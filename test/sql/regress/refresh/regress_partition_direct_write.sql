-- Every partition flushes and truncates. A statement that names a partition
-- directly (a leaf, a middle level, a sub-leaf) refreshes the TVIEW when it ends,
-- in autocommit too; TRUNCATE of a partition refreshes it, and TRUNCATE of the root
-- refreshes it once. Partitions created or attached after the TVIEW get the same
-- triggers, a detached one loses them, and the health check sees a missing one.
--
-- Runs in autocommit on purpose (no -1): each statement is its own transaction,
-- and freshness is checked read-only right after each write.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_partition_direct_write.sql
--
-- expect-output: partition direct write: PASS
-- reject-output: not managed by pg_tviews

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- The entity table is partitioned (P1), tb_line is partitioned and local, tb_note
-- is partitioned, sub-partitioned and mapped (read through tb_line).
CREATE TABLE tb_order (pk_order bigint NOT NULL, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       ref text, PRIMARY KEY (pk_order)) PARTITION BY RANGE (pk_order);
CREATE TABLE tb_order_1 PARTITION OF tb_order FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_order_2 PARTITION OF tb_order FOR VALUES FROM (100) TO (200);
CREATE TABLE tb_line (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL,
                      PRIMARY KEY (pk_line)) PARTITION BY RANGE (pk_line);
CREATE TABLE tb_line_1 PARTITION OF tb_line FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_line_2 PARTITION OF tb_line FOR VALUES FROM (100) TO (200);
CREATE TABLE tb_note (pk_note bigint NOT NULL, fk_line bigint NOT NULL, body text,
                      PRIMARY KEY (pk_note)) PARTITION BY RANGE (pk_note);
CREATE TABLE tb_note_1 PARTITION OF tb_note FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_note_2 PARTITION OF tb_note FOR VALUES FROM (100) TO (200)
    PARTITION BY RANGE (pk_note);
CREATE TABLE tb_note_2a PARTITION OF tb_note_2 FOR VALUES FROM (100) TO (150);
CREATE TABLE tb_note_2b PARTITION OF tb_note_2 FOR VALUES FROM (150) TO (200);

INSERT INTO tb_order (pk_order, ref) VALUES (1, 'o1'), (2, 'o2'), (150, 'o150');
INSERT INTO tb_line (pk_line, fk_order, pos) VALUES (1, 1, 1), (2, 2, 1), (120, 150, 1);
INSERT INTO tb_note (pk_note, fk_line, body) VALUES (1, 1, 'n1'), (120, 2, 'n2'), (170, 120, 'n3');

SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object(
               'ref', o.ref,
               'lines', (SELECT jsonb_agg(l.pos ORDER BY l.pk_line) FROM tb_line l
                         WHERE l.fk_order = o.pk_order),
               'notes', (SELECT jsonb_agg(n.body ORDER BY n.body) FROM tb_line l2
                         JOIN tb_note n ON n.fk_line = l2.pk_line WHERE l2.fk_order = o.pk_order)
           ) AS data
    FROM tb_order o $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
               WHERE t.pk_order IS NULL OR v.pk_order IS NULL
                  OR t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'item 2 FAIL: %', label;
    END IF;
END $$;

-- Scans of tb_order_1 in this transaction: each full refresh of tv_order reads it.
CREATE FUNCTION order_scans() RETURNS bigint LANGUAGE sql AS $$
    SELECT COALESCE(seq_scan, 0) + COALESCE(idx_scan, 0)
    FROM pg_stat_xact_user_tables WHERE relid = 'tb_order_1'::regclass $$;

-- ── direct writes, autocommit ───────────────────────────────────────────────
UPDATE tb_note_1 SET body = 'n1 leaf' WHERE pk_note = 1;
SELECT check_fresh('UPDATE a leaf (mapped)');
INSERT INTO tb_line_2 (pk_line, fk_order, pos) VALUES (130, 150, 2);
SELECT check_fresh('INSERT into a leaf (local)');
UPDATE tb_note_2b SET body = 'n3 sub-leaf' WHERE pk_note = 170;
SELECT check_fresh('UPDATE a sub-leaf (mapped)');
UPDATE tb_note_2 SET body = 'n2 middle' WHERE pk_note = 120;
SELECT check_fresh('UPDATE a middle level (mapped)');
DELETE FROM tb_line_1 WHERE pk_line = 2;
SELECT check_fresh('DELETE from a leaf (local)');
UPDATE tb_order_1 SET ref = 'o1 leaf' WHERE pk_order = 1;
SELECT check_fresh('UPDATE a leaf of the entity table');
INSERT INTO tb_order_2 (pk_order, ref) VALUES (160, 'o160');
SELECT check_fresh('INSERT into a leaf of the entity table');

-- ── partitions created or attached later ────────────────────────────────────
CREATE TABLE tb_note_3 PARTITION OF tb_note FOR VALUES FROM (200) TO (300);
INSERT INTO tb_note_3 (pk_note, fk_line, body) VALUES (210, 130, 'n4 new partition');
SELECT check_fresh('INSERT into a partition created later');
CREATE TABLE tb_line_3 (pk_line bigint NOT NULL, fk_order bigint NOT NULL, pos int NOT NULL);
INSERT INTO tb_line_3 (pk_line, fk_order, pos) VALUES (205, 2, 5);   -- rows come with it
ALTER TABLE tb_line ATTACH PARTITION tb_line_3 FOR VALUES FROM (200) TO (300);
SELECT check_fresh('ATTACH a partition that has rows');
INSERT INTO tb_line_3 (pk_line, fk_order, pos) VALUES (210, 1, 7);
SELECT check_fresh('INSERT into an attached partition');
-- From PL/pgSQL, as partition managers do.
DO $$ BEGIN
    EXECUTE 'CREATE TABLE tb_order_3 PARTITION OF tb_order FOR VALUES FROM (200) TO (300)';
END $$;
INSERT INTO tb_order_3 (pk_order, ref) VALUES (250, 'o250');
SELECT check_fresh('INSERT into a partition created from PL/pgSQL');

-- ── TRUNCATE ────────────────────────────────────────────────────────────────
TRUNCATE tb_note_1;
SELECT check_fresh('TRUNCATE a leaf');
INSERT INTO tb_note (pk_note, fk_line, body) VALUES (1, 1, 'n1 again'), (220, 130, 'n5');
DO $$
DECLARE s0 bigint;
DECLARE one_refresh bigint;
DECLARE root bigint;
BEGIN
    s0 := order_scans();
    TRUNCATE tb_note_1;
    one_refresh := order_scans() - s0;
    s0 := order_scans();
    TRUNCATE tb_note;      -- fires the truncate trigger of the root and of each partition
    root := order_scans() - s0;
    IF one_refresh = 0 OR root > one_refresh THEN
        RAISE EXCEPTION 'item 2 FAIL: TRUNCATE of the root refreshed tv_order % times',
            CASE WHEN one_refresh = 0 THEN 'an unknown number of' ELSE (root / one_refresh)::text END;
    END IF;
END $$;
SELECT check_fresh('TRUNCATE the root');

-- A refresh that fails in the truncate trigger aborts the TRUNCATE (N7).
CREATE TABLE tb_kind (pk_kind bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_weight (w int NOT NULL);
INSERT INTO tb_kind (pk_kind, name) VALUES (1, 'k');
INSERT INTO tb_weight VALUES (1);
SET client_min_messages TO ERROR;   -- tb_weight is read uncorrelated: a WARNING at create
SELECT pg_tviews_create_or_replace('tv_kind', $$
    SELECT k.pk_kind, k.id,
           jsonb_build_object('name', k.name,
                              'share', 1 / (SELECT count(*) FROM tb_weight)) AS data
    FROM tb_kind k $$, '{"uncascaded_policy": "warn"}');
SET client_min_messages TO WARNING;
DO $$ BEGIN
    BEGIN
        TRUNCATE tb_weight;
    EXCEPTION WHEN division_by_zero THEN
        NULL;
    END;
    IF NOT EXISTS (SELECT 1 FROM tb_weight) THEN
        RAISE EXCEPTION 'N7 FAIL: TRUNCATE committed although the refresh of tv_kind failed';
    END IF;
END $$;
DROP TABLE tv_kind;

-- ── DETACH removes ours from the detached table ─────────────────────────────
ALTER TABLE tb_line DETACH PARTITION tb_line_3;
SELECT check_fresh('DETACH a partition');
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid = 'tb_line_3'::regclass
               AND tgname LIKE 'trg_tview_%') THEN
        RAISE EXCEPTION 'item 2 FAIL: pg_tviews triggers left on a detached table: %',
            (SELECT string_agg(tgname, ', ') FROM pg_trigger
             WHERE tgrelid = 'tb_line_3'::regclass AND tgname LIKE 'trg_tview_%');
    END IF;
END $$;

-- ── health check ────────────────────────────────────────────────────────────
DO $$ BEGIN
    IF (SELECT status FROM pg_tviews_health_check() WHERE component = 'triggers') <> 'OK' THEN
        RAISE EXCEPTION 'item 2 FAIL: health check on correct partition triggers: %',
            (SELECT message FROM pg_tviews_health_check() WHERE component = 'triggers');
    END IF;
END $$;
DO $$
DECLARE flush_trigger name := (SELECT tgname FROM pg_trigger
                               WHERE tgrelid = 'tb_note_2a'::regclass AND tgname LIKE 'trg_tview_flush_%');
BEGIN
    IF flush_trigger IS NULL THEN
        RAISE EXCEPTION 'item 2 FAIL: tb_note_2a has no flush trigger';
    END IF;
    EXECUTE format('DROP TRIGGER %I ON tb_note_2a', flush_trigger);
END $$;
DO $$ BEGIN
    IF (SELECT status FROM pg_tviews_health_check() WHERE component = 'triggers') = 'OK' THEN
        RAISE EXCEPTION 'item 2 FAIL: health check misses a dropped partition trigger';
    END IF;
END $$;
SELECT count(*) FROM pg_tviews_reregister_all();
DO $$ BEGIN
    IF (SELECT status FROM pg_tviews_health_check() WHERE component = 'triggers') <> 'OK' THEN
        RAISE EXCEPTION 'item 2 FAIL: re-registration did not repair the partition triggers: %',
            (SELECT message FROM pg_tviews_health_check() WHERE component = 'triggers');
    END IF;
END $$;
UPDATE tb_note_2a SET body = 'after repair' WHERE pk_note = 120;
SELECT check_fresh('UPDATE a sub-leaf after the repair');

\echo 'partition direct write: PASS'
