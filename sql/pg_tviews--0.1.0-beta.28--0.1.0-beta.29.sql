-- pg_tviews 0.1.0-beta.28 → 0.1.0-beta.29
--
-- Pending upgrade script: a pull request that changes the extension SQL adds its
-- statements here (docs/development/extension-versioning.md). Released scripts are
-- never edited.

-- #236: the validity of UNLOGGED TVIEWs is keyed by entity and dumped as extension
-- configuration, so a logical restore that carries an UNLOGGED TVIEW's rows also
-- carries their validity. Existing rows are converted.
ALTER TABLE @extschema@.pg_tview_valid RENAME TO pg_tview_valid_by_oid;
ALTER INDEX @extschema@.pg_tview_valid_pkey RENAME TO pg_tview_valid_by_oid_pkey;
CREATE UNLOGGED TABLE @extschema@.pg_tview_valid (
    entity TEXT NOT NULL PRIMARY KEY
);
COMMENT ON TABLE @extschema@.pg_tview_valid IS
    'Internal: UNLOGGED TVIEWs whose rows can be trusted; may change in any release';
GRANT SELECT ON @extschema@.pg_tview_valid TO PUBLIC;
INSERT INTO @extschema@.pg_tview_valid
SELECT m.entity
FROM @extschema@.pg_tview_valid_by_oid v
JOIN @extschema@.pg_tview_meta m ON m.table_oid::pg_catalog.oid = v.table_oid;
DROP TABLE @extschema@.pg_tview_valid_by_oid;
SELECT pg_catalog.pg_extension_config_dump('@extschema@.pg_tview_valid', '');

CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_catalog_revision()
RETURNS integer
LANGUAGE sql IMMUTABLE PARALLEL SAFE
AS 'SELECT 7';

-- #236: pg_tviews_rebuild_all reports the rows each rebuild inserted
-- (`rows_inserted`, was `rows`, the table's size). A result column's name is part
-- of the function's type: dropped and created again, with the EXECUTE grants an
-- operator gave kept.
DO $$
DECLARE
    grantees text[];
    grantee text;
BEGIN
    SELECT pg_catalog.array_agg(DISTINCT pg_catalog.quote_ident(r.rolname)) INTO grantees
    FROM pg_catalog.pg_proc p,
         LATERAL pg_catalog.aclexplode(p.proacl) a
         JOIN pg_catalog.pg_roles r ON r.oid = a.grantee
    WHERE p.oid = '@extschema@.pg_tviews_rebuild_all(boolean)'::pg_catalog.regprocedure
      AND a.privilege_type = 'EXECUTE' AND a.grantee <> p.proowner;
    DROP FUNCTION @extschema@.pg_tviews_rebuild_all(boolean);
    CREATE FUNCTION @extschema@."pg_tviews_rebuild_all"(
        "only_empty" bool DEFAULT true /* bool */
    ) RETURNS TABLE (
        "entity" TEXT,  /* alloc::string::String */
        "rows_inserted" bigint  /* i64 */
    )
    STRICT
    LANGUAGE c /* Rust */
    AS 'MODULE_PATHNAME', 'pg_tviews_rebuild_all_wrapper';
    REVOKE EXECUTE ON FUNCTION @extschema@.pg_tviews_rebuild_all(BOOLEAN) FROM PUBLIC;
    FOREACH grantee IN ARRAY coalesce(grantees, '{}') LOOP
        EXECUTE pg_catalog.format('GRANT EXECUTE ON FUNCTION @extschema@.pg_tviews_rebuild_all(BOOLEAN) TO %s', grantee);
    END LOOP;
END $$;

-- #233: pg_tviews_profile() projects each fan-out's cost per write (p99_ms_per_write,
-- p99_wal_bytes_per_write in `fanout`) and says it in the fan-out warning.
CREATE OR REPLACE FUNCTION @extschema@.pg_tviews_profile(
    tview       TEXT   DEFAULT NULL,
    fanout_warn BIGINT DEFAULT 1000)
RETURNS TABLE (
    entity                      TEXT,
    schema                      TEXT,
    name                        TEXT,
    persistence                 TEXT,
    replica_readable            BOOLEAN,
    rows_estimate               BIGINT,
    heap_bytes                  BIGINT,
    index_bytes                 BIGINT,
    toast_bytes                 BIGINT,
    avg_row_width               INTEGER,
    data_avg_width              INTEGER,
    fillfactor                  INTEGER,
    n_tup_upd                   BIGINT,
    n_tup_hot_upd               BIGINT,
    hot_ratio                   DOUBLE PRECISION,
    n_dead_tup                  BIGINT,
    last_vacuum                 TIMESTAMPTZ,
    last_autovacuum             TIMESTAMPTZ,
    all_visible_fraction        DOUBLE PRECISION,
    unused_indexes              TEXT[],
    missing_propagation_indexes TEXT[],
    fanout                      JSONB,
    warnings                    TEXT[])
LANGUAGE plpgsql
STABLE
SET search_path = pg_catalog, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    r            RECORD;
    f            RECORD;
    col          TEXT;
    idx          TEXT;
    vis_schema   NAME;
    all_visible  BIGINT;
    stats_reset  TIMESTAMPTZ := (SELECT d.stats_reset FROM pg_stat_database d
                                 WHERE d.datname = current_database());
    chosen       TEXT;
    qualified    TEXT;
    doc_bytes    FLOAT8;
    -- The fan-out cost model (ADR 0233), measured 2026-10-13 on the stock
    -- PostgreSQL 18.4 release package: a write costs a fixed part plus a part per
    -- embedding row it patches, and writes WAL per row in proportion to the
    -- document. Documents over toasted_doc bytes are stored out of line and
    -- rewritten whole.
    fixed_ms       CONSTANT FLOAT8 := 0.15;
    inline_row_ms  CONSTANT FLOAT8 := 0.0028;
    toasted_row_ms CONSTANT FLOAT8 := 0.021;
    inline_wal     CONSTANT FLOAT8 := 0.7;
    toasted_wal    CONSTANT FLOAT8 := 1.2;
    toasted_doc    CONSTANT FLOAT8 := 2000;
BEGIN
    -- The TVIEW named as every function names one (ADR 0211).
    IF tview IS NOT NULL THEN
        chosen := @extschema@.pg_tviews_entity_of(tview);
    END IF;

    vis_schema := (SELECT n.nspname FROM pg_extension e
                   JOIN pg_namespace n ON n.oid = e.extnamespace
                   WHERE e.extname = 'pg_visibility');

    FOR r IN
        SELECT m.entity AS ent, c.oid AS rel, n.nspname AS nsp, c.relname AS tbl,
               c.relpersistence AS pers, c.reltuples, c.relpages, c.reltoastrelid,
               c.reloptions,
               -- The columns its rows are looked up by: those holding an
               -- embedded TVIEW's key, and those a fan-out patch writes through.
               ARRAY(SELECT jsonb_array_elements_text(e->'lookups')
                     FROM jsonb_array_elements(m.plan->'embeds') e
                     UNION
                     SELECT t->'fanout'->>'lookup_col'
                     FROM jsonb_array_elements(m.plan->'tables') t
                     WHERE t ? 'fanout') AS lookups
        FROM @extschema@.pg_tview_meta m
        JOIN pg_class c ON c.oid = m.table_oid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE chosen IS NULL OR m.entity = chosen
        ORDER BY m.entity
    LOOP
        entity           := r.ent;
        schema           := r.nsp;
        name             := r.tbl;
        qualified        := quote_ident(r.nsp) || '.' || quote_ident(r.tbl);
        persistence      := CASE r.pers WHEN 'u' THEN 'unlogged' ELSE 'logged' END;
        replica_readable := r.pers = 'p';
        rows_estimate    := CASE WHEN r.reltuples < 0 THEN NULL ELSE r.reltuples::BIGINT END;
        heap_bytes       := pg_relation_size(r.rel);
        index_bytes      := pg_indexes_size(r.rel);
        toast_bytes      := CASE WHEN r.reltoastrelid = 0 THEN 0
                                 ELSE pg_relation_size(r.reltoastrelid) END;

        SELECT sum(s.avg_width)::INTEGER, max(s.avg_width) FILTER (WHERE s.attname = 'data')
          INTO avg_row_width, data_avg_width
          FROM pg_stats s WHERE s.schemaname = r.nsp AND s.tablename = r.tbl;

        fillfactor := coalesce((SELECT split_part(o, '=', 2)::INTEGER
                                FROM unnest(r.reloptions) o WHERE o LIKE 'fillfactor=%'), 100);

        SELECT s.n_tup_upd, s.n_tup_hot_upd, s.n_dead_tup, s.last_vacuum, s.last_autovacuum
          INTO n_tup_upd, n_tup_hot_upd, n_dead_tup, last_vacuum, last_autovacuum
          FROM pg_stat_all_tables s WHERE s.relid = r.rel;
        hot_ratio := CASE WHEN n_tup_upd > 0 THEN n_tup_hot_upd::FLOAT8 / n_tup_upd END;

        -- The visibility map of an UNLOGGED table cannot be read during recovery.
        all_visible_fraction := NULL;
        IF vis_schema IS NOT NULL AND r.relpages > 0
           AND NOT (pg_is_in_recovery() AND r.pers = 'u') THEN
            EXECUTE format('SELECT all_visible FROM %I.pg_visibility_map_summary($1)', vis_schema)
               INTO all_visible USING r.rel::REGCLASS;
            all_visible_fraction := all_visible::FLOAT8 / r.relpages;
        END IF;

        -- Never-scanned indexes, except the primary key, unique indexes and the
        -- propagation indexes cascades need (leading with a lookup column).
        unused_indexes := ARRAY(
            SELECT quote_ident(si.indexrelname) FROM pg_index i
            JOIN pg_stat_all_indexes si ON si.indexrelid = i.indexrelid
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
            WHERE i.indrelid = r.rel AND NOT i.indisprimary AND NOT i.indisunique
              AND si.idx_scan = 0 AND a.attname <> ALL (r.lookups)
            ORDER BY 1);

        -- Lookup columns that no index leads with.
        missing_propagation_indexes := ARRAY(
            SELECT a.attname::TEXT FROM pg_attribute a
            WHERE a.attrelid = r.rel AND a.attnum > 0 AND NOT a.attisdropped
              AND a.attname = ANY (r.lookups)
              AND NOT EXISTS (SELECT 1 FROM pg_index i
                              WHERE i.indrelid = r.rel AND i.indkey[0] = a.attnum)
            ORDER BY 1);

        -- Estimated rows per key of each of those columns, from the planner statistics:
        -- p50 = rows / distinct keys, max = top MCV, p99 = the MCV at the 1% rank
        -- (or the average of the non-MCV keys when the MCV list is shorter).
        fanout := (
            SELECT jsonb_object_agg(x.attname, jsonb_build_object(
                       'p50', round(r.reltuples / x.nd),
                       'p99', round(CASE
                           WHEN x.k <= coalesce(array_length(x.mcf, 1), 0)
                               THEN x.mcf[x.k] * r.reltuples
                           WHEN x.nd > coalesce(array_length(x.mcf, 1), 0)
                               THEN (1 - coalesce((SELECT sum(v) FROM unnest(x.mcf) v), 0))
                                    * r.reltuples / (x.nd - coalesce(array_length(x.mcf, 1), 0))
                           ELSE r.reltuples / x.nd END),
                       'max', round(coalesce(x.mcf[1], 1 / x.nd) * r.reltuples)))
            FROM (SELECT s.attname,
                         s.most_common_freqs AS mcf,
                         greatest(CASE WHEN s.n_distinct < 0 THEN -s.n_distinct * r.reltuples
                                       ELSE s.n_distinct END, 1) AS nd,
                         greatest(floor(greatest(CASE WHEN s.n_distinct < 0
                                                      THEN -s.n_distinct * r.reltuples
                                                      ELSE s.n_distinct END, 1) * 0.01)::INTEGER, 1) AS k
                  FROM pg_stats s
                  WHERE s.schemaname = r.nsp AND s.tablename = r.tbl
                    AND s.attname = ANY (r.lookups) AND r.reltuples > 0) x);

        -- What one parent change costs at that p99, by the model: its documents
        -- are the table's bytes per row.
        doc_bytes := (heap_bytes + toast_bytes) / greatest(r.reltuples, 1);
        fanout := (
            SELECT jsonb_object_agg(e.key, e.value || jsonb_build_object(
                       'p99_ms_per_write', round((fixed_ms + (e.value->>'p99')::FLOAT8 *
                           CASE WHEN doc_bytes > toasted_doc THEN toasted_row_ms
                                ELSE inline_row_ms END)::NUMERIC, 2),
                       'p99_wal_bytes_per_write', round((e.value->>'p99')::FLOAT8 * doc_bytes *
                           CASE WHEN doc_bytes > toasted_doc THEN toasted_wal
                                ELSE inline_wal END)))
            FROM jsonb_each(fanout) e);

        warnings := ARRAY[]::TEXT[];
        FOREACH col IN ARRAY missing_propagation_indexes LOOP
            warnings := warnings || format(
                '%s has no index: a cascade into %s scans the whole table. Run pg_tviews_ensure_propagation_indexes(%L)',
                col, qualified, r.ent);
        END LOOP;
        IF n_tup_upd > 1000 AND hot_ratio < 0.5 THEN
            FOR idx IN
                SELECT DISTINCT quote_ident(ic.relname) FROM pg_index i
                JOIN pg_class ic ON ic.oid = i.indexrelid
                JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey)
                WHERE i.indrelid = r.rel AND a.attname IN ('data', 'updated_at')
                ORDER BY 1
            LOOP
                warnings := warnings || format(
                    'HOT ratio %s%%: index %s on a column every refresh changes prevents HOT updates',
                    round(hot_ratio * 100), idx);
            END LOOP;
        END IF;
        FOR idx IN
            SELECT quote_ident(si.indexrelname) FROM pg_index i
            JOIN pg_class ic ON ic.oid = i.indexrelid
            JOIN pg_am am ON am.oid = ic.relam
            JOIN pg_stat_all_indexes si ON si.indexrelid = i.indexrelid
            JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
            WHERE i.indrelid = r.rel AND am.amname = 'gin' AND a.attname = 'data'
              AND si.idx_scan = 0
            ORDER BY 1
        LOOP
            warnings := warnings || format(
                'GIN index %s on data never scanned since statistics reset (%s)',
                idx, coalesce(stats_reset::TEXT, 'never'));
        END LOOP;
        IF fillfactor = 100 AND n_tup_upd > 0 AND n_tup_upd > coalesce(rows_estimate, 0) THEN
            warnings := warnings ||
                'fillfactor 100 on a frequently updated TVIEW: refreshed rows cannot stay on their page (option fillfactor, default 85)'::TEXT;
        END IF;
        IF toast_bytes > 0 AND toast_bytes > 0.3 * (heap_bytes + toast_bytes) THEN
            warnings := warnings || format(
                '%s%% of the table is TOAST: each refresh rewrites whole data documents (see docs/adr/0094-large-document-refresh.md)',
                round(100.0 * toast_bytes / (heap_bytes + toast_bytes)));
        END IF;
        FOR f IN SELECT key, (value->>'p99')::BIGINT AS p99, value->>'p99_ms_per_write' AS ms,
                        (value->>'p99_wal_bytes_per_write')::BIGINT AS wal
                 FROM jsonb_each(fanout) ORDER BY key LOOP
            IF f.p99 > fanout_warn THEN
                warnings := warnings || format(
                    'p99 fan-out through %s is about %s rows per key: one parent change refreshes that many rows, about %s ms and %s of WAL per write (see docs/user-guides/architects.md#fan-out)',
                    f.key, f.p99, f.ms, pg_size_pretty(f.wal));
            END IF;
        END LOOP;
        IF r.pers = 'u' THEN
            warnings := warnings ||
                'UNLOGGED: not readable on hot standbys, empty after promotion or a crash restart (option logged)'::TEXT;
        END IF;
        IF rows_estimate > 0 AND n_dead_tup > 0.2 * rows_estimate THEN
            warnings := warnings || format(
                '%s%% dead tuples: autovacuum is behind (last autovacuum %s)',
                round(100.0 * n_dead_tup / rows_estimate),
                coalesce(last_autovacuum::TEXT, 'never'));
        END IF;

        RETURN NEXT;
    END LOOP;
END;
$$;

-- #230: pg_tviews_flush_and_report(types) reports only the requested types. An
-- argument added to a function is a new function: dropped and created again.
DROP FUNCTION @extschema@.pg_tviews_flush_and_report(INT, bool, bool);
CREATE  FUNCTION @extschema@."pg_tviews_flush_and_report"(
	"max_entities" INT DEFAULT 500, /* i32 */
	"include_data" bool DEFAULT true, /* bool */
	"reset" bool DEFAULT true, /* bool */
	"types" TEXT[] DEFAULT NULL /* core::option::Option<alloc::vec::Vec<alloc::string::String>> */
) RETURNS jsonb /* core::result::Result<pgrx::datum::json::JsonB, pgrx_pg_sys::submodules::panic::ErrorReport> */
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'pg_tviews_flush_and_report_wrapper';

-- #228: registration derives fan-out patches for columns copied two joins away.
-- Existing TVIEWs keep refreshing with their plans until re-registered.
UPDATE @extschema@.pg_tview_meta SET needs_reregister = true;
