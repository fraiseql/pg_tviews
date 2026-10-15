-- Regression test (#231): the report's ids come from the refresh writes.
--
-- Every journaled change carries the row's public id, returned by the write that
-- made it (upsert, delete, direct patch, fan-out patch, full refresh), so
-- pg_tviews_flush_and_report(include_data => false) reads no TVIEW at all, and the
-- change set can be published at commit, when nothing can be read.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_report_ids_from_writes.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'report ids FAIL: %', what; END IF; END $$;

-- Scans of the TVIEW tables in this transaction.
CREATE FUNCTION tv_scans() RETURNS bigint LANGUAGE sql AS $$
    SELECT sum(seq_scan + coalesce(idx_scan, 0))::bigint FROM pg_stat_xact_user_tables
    WHERE relname IN ('tv_cat', 'tv_item', 'tv_tag') $$;

CREATE TABLE tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_cat int NOT NULL REFERENCES tb_cat, title text NOT NULL);
CREATE INDEX ON tb_item (fk_cat);
-- A TVIEW without an id column: its rows are reported with id null.
CREATE TABLE tb_tag (pk_tag int PRIMARY KEY, label text NOT NULL);
INSERT INTO tb_cat (pk_cat, name) VALUES (1, 'a'), (2, 'b');
INSERT INTO tb_item (pk_item, fk_cat, title) SELECT g, 1 + g % 2, 't' FROM generate_series(1, 6) g;
INSERT INTO tb_tag VALUES (1, 'x'), (2, 'y');
SELECT tviews.pg_tviews_create('tv_cat', $$
    SELECT c.pk_cat, c.id, jsonb_build_object('name', c.name) AS data FROM tb_cat c $$);
SELECT tviews.pg_tviews_create('tv_item', $$
    SELECT i.pk_item, i.id, i.fk_cat,
           jsonb_build_object('title', i.title, 'category', jsonb_build_object('name', c.name)) AS data
    FROM tb_item i JOIN tb_cat c ON c.pk_cat = i.fk_cat $$);
SELECT tviews.pg_tviews_create('tv_tag', $$
    SELECT t.pk_tag, jsonb_build_object('label', t.label) AS data FROM tb_tag t $$);

-- 1. Direct patch, fan-out patch, insert, delete, recompute, a TVIEW without id:
--    the report reads no TVIEW, and its ids are the rows' own.
BEGIN;
UPDATE tb_cat SET name = 'a2' WHERE pk_cat = 1;           -- tv_cat patch, tv_item fan-out
INSERT INTO tb_item VALUES (7, DEFAULT, 2, 'new');        -- insert
DELETE FROM tb_item WHERE pk_item = 6;                     -- delete
UPDATE tb_item SET fk_cat = 1 WHERE pk_item = 3;           -- recompute
UPDATE tb_tag SET label = 'z' WHERE pk_tag = 1;            -- no id column
CREATE TEMP TABLE expected AS
    SELECT 'Cat' AS t, id::text AS id FROM tv_cat WHERE pk_cat = 1
    UNION ALL SELECT 'Item', id::text FROM tv_item WHERE fk_cat = 1 OR pk_item = 7;
SELECT tv_scans() AS before \gset
SELECT tviews.pg_tviews_flush_and_report(include_data => false) AS r \gset
SELECT must(tv_scans() = :before, (tv_scans() - :before) || ' scan(s) of TVIEWs by the report');
SELECT must((SELECT array_agg(x ORDER BY x) FROM (
                 SELECT e->>'__typename' || ':' || coalesce(e->>'id', 'null') AS x
                 FROM jsonb_array_elements((:'r'::jsonb)->'updated') e) r)
            = (SELECT array_agg(x ORDER BY x) FROM (
                 SELECT t || ':' || id AS x FROM expected UNION ALL SELECT 'Tag:null') e),
            'updated: ' || :'r');
SELECT must((SELECT count(*) FROM jsonb_array_elements((:'r'::jsonb)->'deleted') e
             WHERE e->>'__typename' = 'Item' AND e->>'id' IS NOT NULL) = 1, 'deleted: ' || :'r');
COMMIT;

-- 2. With data, the documents are read and the ids are the same.
BEGIN;
UPDATE tb_cat SET name = 'a3' WHERE pk_cat = 1;
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(bool_and(e->>'id' = (SELECT id::text FROM tv_cat WHERE pk_cat = 1)
                     AND e->'data'->>'name' = 'a3'), 'with data: ' || :'r')
FROM jsonb_array_elements((:'r'::jsonb)->'updated') e WHERE e->>'__typename' = 'Cat';
COMMIT;

\echo 'report ids from writes: PASS'
