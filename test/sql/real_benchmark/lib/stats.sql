-- Physical-cost snapshot helpers for the real benchmark.
--
--   bench.snapshot(label, ops)   record per-relation + cluster WAL/IO counters
--   bench.delta(a, b)            per-relation deltas between two snapshots
--   bench.physical               bench.delta over every consecutive snapshot pair
--
-- PG15+ publishes table stats lazily, so callers must issue
--   SELECT pg_stat_force_next_flush();
-- as a SEPARATE statement right before bench.snapshot(): the pending counters are
-- flushed when the backend goes idle between the two statements.
--
-- `ops` on the closing snapshot is the number of timed operations in the step
-- (e.g. refreshes); bench.physical divides by it for per-op figures.
--
-- Idempotent: safe to \i more than once. Scratch tables are UNLOGGED so the
-- bookkeeping itself adds no WAL to the measurement.

SET client_min_messages TO WARNING;

CREATE SCHEMA IF NOT EXISTS bench;

-- pg_visibility is contrib; the all-visible fraction degrades to NULL without it.
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS pg_visibility;
EXCEPTION WHEN OTHERS THEN
    RAISE NOTICE 'pg_visibility unavailable: all-visible fraction will be NULL';
END $$;

CREATE UNLOGGED TABLE IF NOT EXISTS bench.snaps (
    seq      int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label    text NOT NULL UNIQUE,
    ops      int,
    taken_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    -- cluster-wide WAL (pg_stat_wal)
    wal_records      bigint,
    wal_fpi          bigint,
    wal_bytes        numeric,
    wal_buffers_full bigint,
    -- client-backend relation IO (pg_stat_io)
    io_hits    bigint,
    io_reads   bigint,
    io_writes  bigint,
    io_extends bigint
);

CREATE UNLOGGED TABLE IF NOT EXISTS bench.rel_snaps (
    seq            int NOT NULL REFERENCES bench.snaps (seq) ON DELETE CASCADE,
    relid          oid NOT NULL,
    relname        text NOT NULL,
    relkind        "char" NOT NULL,
    relpersistence "char" NOT NULL,
    seq_scan          bigint,
    idx_scan          bigint,
    n_tup_ins         bigint,
    n_tup_upd         bigint,
    n_tup_hot_upd     bigint,
    n_tup_newpage_upd bigint,
    n_tup_del         bigint,
    n_live_tup        bigint,
    n_dead_tup        bigint,
    heap_blks_hit   bigint,
    heap_blks_read  bigint,
    idx_blks_hit    bigint,
    idx_blks_read   bigint,
    toast_blks_hit  bigint,
    toast_blks_read bigint,
    heap_bytes  bigint,
    index_bytes bigint,
    toast_bytes bigint,
    heap_pages         bigint,
    all_visible_pages  bigint,
    PRIMARY KEY (seq, relid)
);

CREATE OR REPLACE FUNCTION bench.snapshot(p_label text, p_ops int DEFAULT NULL)
RETURNS int
LANGUAGE plpgsql AS $$
DECLARE
    v_seq int;
    v_has_vm boolean := EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_visibility');
BEGIN
    -- Drop any cached stats snapshot so this transaction sees the flushed values.
    PERFORM pg_stat_clear_snapshot();

    INSERT INTO bench.snaps (label, ops, wal_records, wal_fpi, wal_bytes, wal_buffers_full,
                             io_hits, io_reads, io_writes, io_extends)
    SELECT p_label, p_ops, w.wal_records, w.wal_fpi, w.wal_bytes, w.wal_buffers_full,
           io.hits, io.reads, io.writes, io.extends
    FROM pg_stat_wal w,
         LATERAL (SELECT sum(hits)::bigint AS hits, sum(reads)::bigint AS reads,
                         sum(writes)::bigint AS writes, sum(extends)::bigint AS extends
                  FROM pg_stat_io
                  WHERE backend_type = 'client backend' AND object = 'relation') io
    RETURNING seq INTO v_seq;

    INSERT INTO bench.rel_snaps
    SELECT v_seq, c.oid, c.relname, c.relkind, c.relpersistence,
           s.seq_scan, s.idx_scan, s.n_tup_ins, s.n_tup_upd, s.n_tup_hot_upd,
           s.n_tup_newpage_upd, s.n_tup_del, s.n_live_tup, s.n_dead_tup,
           io.heap_blks_hit, io.heap_blks_read, io.idx_blks_hit, io.idx_blks_read,
           io.toast_blks_hit, io.toast_blks_read,
           pg_relation_size(c.oid),
           pg_indexes_size(c.oid),
           CASE WHEN c.reltoastrelid <> 0 THEN pg_total_relation_size(c.reltoastrelid) ELSE 0 END,
           pg_relation_size(c.oid) / current_setting('block_size')::int,
           NULL
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_stat_all_tables s ON s.relid = c.oid
    JOIN pg_statio_all_tables io ON io.relid = c.oid
    WHERE c.relkind IN ('r', 'm')
      AND n.nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'bench');

    IF v_has_vm THEN
        UPDATE bench.rel_snaps r
        SET all_visible_pages = (SELECT all_visible FROM pg_visibility_map_summary(r.relid::regclass))
        WHERE r.seq = v_seq;
    END IF;

    RETURN v_seq;
END $$;

-- Zero this database's table counters and the cluster WAL/IO counters. Call at
-- the start of a scenario, before its first snapshot (deltas stay valid either
-- way; the reset keeps absolute counters readable when inspecting by hand).
CREATE OR REPLACE FUNCTION bench.reset()
RETURNS void
LANGUAGE plpgsql AS $$
DECLARE r record;
BEGIN
    FOR r IN SELECT relid FROM pg_stat_user_tables WHERE schemaname <> 'bench' LOOP
        PERFORM pg_stat_reset_single_table_counters(r.relid);
    END LOOP;
    PERFORM pg_stat_reset_shared('wal');
    PERFORM pg_stat_reset_shared('io');
    TRUNCATE bench.snaps CASCADE;
END $$;

CREATE OR REPLACE FUNCTION bench.delta(p_from text, p_to text)
RETURNS TABLE (
    relname text, relpersistence "char", ops int,
    seq_scan bigint, idx_scan bigint,
    n_tup_ins bigint, n_tup_upd bigint, n_tup_hot_upd bigint, n_tup_newpage_upd bigint,
    n_tup_del bigint, n_dead_tup_after bigint, n_live_tup_after bigint,
    heap_blks_hit bigint, heap_blks_read bigint, idx_blks_hit bigint, idx_blks_read bigint,
    toast_blks_hit bigint, toast_blks_read bigint,
    heap_bytes_before bigint, heap_bytes_after bigint,
    index_bytes_before bigint, index_bytes_after bigint,
    toast_bytes_before bigint, toast_bytes_after bigint,
    all_visible_frac_after numeric,
    wal_records bigint, wal_fpi bigint, wal_bytes numeric, wal_buffers_full bigint,
    io_hits bigint, io_reads bigint, io_writes bigint, io_extends bigint,
    elapsed_ms numeric
)
LANGUAGE sql STABLE AS $$
    SELECT b.relname, b.relpersistence, sb.ops,
           b.seq_scan - a.seq_scan, b.idx_scan - a.idx_scan,
           b.n_tup_ins - a.n_tup_ins, b.n_tup_upd - a.n_tup_upd,
           b.n_tup_hot_upd - a.n_tup_hot_upd, b.n_tup_newpage_upd - a.n_tup_newpage_upd,
           b.n_tup_del - a.n_tup_del, b.n_dead_tup, b.n_live_tup,
           b.heap_blks_hit - a.heap_blks_hit, b.heap_blks_read - a.heap_blks_read,
           b.idx_blks_hit - a.idx_blks_hit, b.idx_blks_read - a.idx_blks_read,
           b.toast_blks_hit - a.toast_blks_hit, b.toast_blks_read - a.toast_blks_read,
           a.heap_bytes, b.heap_bytes, a.index_bytes, b.index_bytes,
           a.toast_bytes, b.toast_bytes,
           round(b.all_visible_pages::numeric / NULLIF(b.heap_pages, 0), 4),
           sb.wal_records - sa.wal_records, sb.wal_fpi - sa.wal_fpi,
           sb.wal_bytes - sa.wal_bytes, sb.wal_buffers_full - sa.wal_buffers_full,
           sb.io_hits - sa.io_hits, sb.io_reads - sa.io_reads,
           sb.io_writes - sa.io_writes, sb.io_extends - sa.io_extends,
           round(extract(epoch FROM sb.taken_at - sa.taken_at)::numeric * 1000, 1)
    FROM bench.snaps sa
    JOIN bench.snaps sb ON sb.label = p_to
    JOIN bench.rel_snaps b ON b.seq = sb.seq
    -- A relation created during the step has no "before" row: count from zero.
    LEFT JOIN LATERAL (
        SELECT * FROM bench.rel_snaps r WHERE r.seq = sa.seq AND r.relid = b.relid
        UNION ALL
        SELECT sa.seq, b.relid, b.relname, b.relkind, b.relpersistence,
               0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        WHERE NOT EXISTS (SELECT 1 FROM bench.rel_snaps r WHERE r.seq = sa.seq AND r.relid = b.relid)
    ) a ON true
    WHERE sa.label = p_from
$$;

-- Every step (a snapshot and its predecessor) with derived ratios. WAL/IO
-- columns are cluster-wide for the step and so repeat on each relation row.
CREATE OR REPLACE VIEW bench.physical AS
SELECT s.seq,
       s.label AS step,
       d.*,
       round(100.0 * d.n_tup_hot_upd / NULLIF(d.n_tup_upd, 0), 1) AS hot_pct,
       round(d.wal_bytes / NULLIF(d.ops, 0)) AS wal_bytes_per_op,
       round((d.heap_bytes_after + d.index_bytes_after + d.toast_bytes_after)::numeric
             / NULLIF(d.n_live_tup_after, 0)) AS bytes_per_row
FROM bench.snaps s
JOIN LATERAL (SELECT p.label FROM bench.snaps p WHERE p.seq < s.seq ORDER BY p.seq DESC LIMIT 1) prev
     ON true
CROSS JOIN LATERAL bench.delta(prev.label, s.label) d;

RESET client_min_messages;
