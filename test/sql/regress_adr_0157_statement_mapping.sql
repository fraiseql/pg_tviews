-- ADR 0157, execution: a `mapped` base table carries statement-level triggers that
-- map the statement's changed rows (its transition tables) to TVIEW keys with one
-- query. Every way of writing to the table refreshes the TVIEW: UPDATE, INSERT,
-- DELETE, INSERT … ON CONFLICT, MERGE, a writable CTE touching two of its tables,
-- writes routed through a partitioned table, TRUNCATE. An UPDATE that changes no
-- column the TVIEW reads maps nothing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_adr_0157_statement_mapping.sql
--
-- expect-output: ADR 0157 statement mapping: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_order (pk_order bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), ref text);
CREATE TABLE tb_sku   (pk_sku bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       name text, cost int NOT NULL DEFAULT 0);
CREATE TABLE tb_line  (pk_line bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_order bigint NOT NULL, fk_sku bigint, pos int NOT NULL);
-- Partitioned, read through tb_line.
CREATE TABLE tb_note  (pk_note bigint NOT NULL, fk_line bigint NOT NULL, body text,
                       PRIMARY KEY (pk_note)) PARTITION BY RANGE (pk_note);
CREATE TABLE tb_note_1 PARTITION OF tb_note FOR VALUES FROM (0) TO (100);
CREATE TABLE tb_note_2 PARTITION OF tb_note FOR VALUES FROM (100) TO (200);

INSERT INTO tb_order (pk_order, ref) SELECT g, 'o' || g FROM generate_series(1, 4) g;
INSERT INTO tb_sku (pk_sku, name) SELECT g, 's' || g FROM generate_series(1, 3) g;
INSERT INTO tb_line (pk_line, fk_order, fk_sku, pos)
    SELECT g, 1 + g % 4, 1 + g % 3, g FROM generate_series(1, 12) g;
INSERT INTO tb_note (pk_note, fk_line, body) VALUES (1, 1, 'n1'), (150, 2, 'n2');

SELECT pg_tviews_create('tv_order', $$
    SELECT o.pk_order, o.id,
           jsonb_build_object(
               'ref', o.ref,
               'skus', (SELECT jsonb_agg(s.name ORDER BY s.name) FROM tb_line l
                        JOIN tb_sku s ON s.pk_sku = l.fk_sku WHERE l.fk_order = o.pk_order),
               'notes', (SELECT jsonb_agg(n.body ORDER BY n.body) FROM tb_line l2
                         JOIN tb_note n ON n.fk_line = l2.pk_line WHERE l2.fk_order = o.pk_order)
           ) AS data
    FROM tb_order o $$);

CREATE FUNCTION check_fresh(label text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM tv_order t FULL JOIN tviews.public__tv_order v USING (pk_order)
               WHERE t.data IS DISTINCT FROM v.data) THEN
        RAISE EXCEPTION 'FAIL ADR 0157 statement mapping [%]: tv_order diverges from tviews.public__tv_order', label;
    END IF;
END $$;

-- The layout: tb_sku and tb_note map through a query, tb_line is local.
DO $$ BEGIN
    IF (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order')
       <> '{"tb_order": "local", "tb_line": "local", "tb_sku": "mapped", "tb_note": "mapped"}' THEN
        RAISE EXCEPTION 'FAIL: unexpected cascade kinds %',
            (SELECT cascade_kinds FROM tviews.registry WHERE entity = 'order');
    END IF;
    IF (SELECT count(*) FROM pg_trigger WHERE tgrelid = 'tb_sku'::regclass AND tgname LIKE 'trg_tview_delta_%') <> 3 THEN
        RAISE EXCEPTION 'FAIL: tb_sku should carry three delta triggers';
    END IF;
END $$;

-- ── two hops: one row, then every row ──────────────────────────────────────
UPDATE tb_sku SET name = 'S2' WHERE pk_sku = 2;
SELECT check_fresh('UPDATE one sku');
UPDATE tb_sku SET name = upper(name) || '!';
SELECT check_fresh('UPDATE every sku');
INSERT INTO tb_sku (pk_sku, name) VALUES (4, 'new');
UPDATE tb_line SET fk_sku = 4 WHERE pk_line = 1;
SELECT check_fresh('INSERT sku, point a line at it');
DELETE FROM tb_line WHERE fk_sku = 4;
DELETE FROM tb_sku WHERE pk_sku = 4;
SELECT check_fresh('DELETE');

-- ── INSERT … ON CONFLICT and MERGE (both events) ───────────────────────────
INSERT INTO tb_sku (pk_sku, name) VALUES (1, 'upserted'), (5, 'fresh')
    ON CONFLICT (pk_sku) DO UPDATE SET name = excluded.name;
SELECT check_fresh('ON CONFLICT');
MERGE INTO tb_sku t USING (VALUES (2, 'merged'), (6, 'inserted')) s(pk, name) ON t.pk_sku = s.pk
    WHEN MATCHED THEN UPDATE SET name = s.name
    WHEN NOT MATCHED THEN INSERT (pk_sku, name) VALUES (s.pk, s.name);
SELECT check_fresh('MERGE');

-- ── a writable CTE: a sku renamed and a line moved to another order ─────────
WITH renamed AS (UPDATE tb_sku SET name = 'cte' WHERE pk_sku = 3 RETURNING pk_sku)
UPDATE tb_line SET fk_order = 4 WHERE pk_line = 2;
SELECT check_fresh('writable CTE');

-- ── an explicit transaction ─────────────────────────────────────────────────
BEGIN;
UPDATE tb_sku SET name = 'tx1' WHERE pk_sku = 1;
UPDATE tb_sku SET name = 'tx2' WHERE pk_sku = 2;
COMMIT;
SELECT check_fresh('explicit transaction');

-- ── a partitioned table: writes routed through the root, and to a partition ─
INSERT INTO tb_note (pk_note, fk_line, body) VALUES (2, 3, 'routed'), (151, 4, 'routed 2');
SELECT check_fresh('INSERT routed to partitions');
UPDATE tb_note SET pk_note = 160 WHERE pk_note = 1;   -- moves to the other partition
SELECT check_fresh('UPDATE across partitions');
BEGIN;
UPDATE tb_note_2 SET body = 'direct' WHERE pk_note = 150;
COMMIT;
SELECT check_fresh('UPDATE of a partition');

-- ── an UPDATE of a column the TVIEW does not read maps nothing ──────────────
DO $$
DECLARE before bigint := (tviews.pg_tviews_queue_stats()->>'total_refreshes')::bigint;
BEGIN
    UPDATE tb_sku SET cost = cost + 1;
    IF (tviews.pg_tviews_queue_stats()->>'total_refreshes')::bigint <> before THEN
        RAISE EXCEPTION 'FAIL [column-aware]: an UPDATE of tb_sku.cost refreshed % rows',
            (tviews.pg_tviews_queue_stats()->>'total_refreshes')::bigint - before;
    END IF;
END $$;
SELECT check_fresh('column-aware skip');

-- ── TRUNCATE ────────────────────────────────────────────────────────────────
TRUNCATE tb_note;
SELECT check_fresh('TRUNCATE a mapped table');
TRUNCATE tb_line;
SELECT check_fresh('TRUNCATE a local table');

\echo 'ADR 0157 statement mapping: PASS'
