-- Regression test (#240): the report reads a TVIEW's changed rows by key.
--
-- pg_tviews_flush_and_report(include_data => true) reads the fresh documents of the
-- rows it reports. It compared `pk_<entity>::text = ANY($1)`, an expression no
-- index serves, so every report scanned each reported TVIEW in full: its cost grew
-- with the TVIEWs, not with the rows the transaction changed.
--
-- Correct behaviour: one changed row of a large TVIEW is read through the
-- pk_<entity> index; the report reads no row of the TVIEW by a sequential scan.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_report_reads_by_key.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'report reads by key FAIL: %', what; END IF; END $$;

CREATE TABLE tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text NOT NULL);
CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user int NOT NULL REFERENCES tb_user, title text NOT NULL);
CREATE INDEX ON tb_post (fk_user);
INSERT INTO tb_user (pk_user, name) SELECT g, 'u' || g FROM generate_series(1, 500) g;
INSERT INTO tb_post (pk_post, fk_user, title)
SELECT g, 1 + g % 500, 't' || g FROM generate_series(1, 5000) g;
SELECT tviews.pg_tviews_create('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM tb_user u $$);
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title, 'author', jsonb_build_object('name', u.name)) AS data
    FROM tb_post p JOIN tb_user u ON u.pk_user = p.fk_user $$);
ANALYZE tv_user, tv_post;

-- Rows of tv_user and tv_post read by sequential scans, and their index scans, in
-- this transaction.
CREATE FUNCTION seq_rows() RETURNS bigint LANGUAGE sql AS $$
    SELECT sum(seq_tup_read)::bigint FROM pg_stat_xact_user_tables
    WHERE relname IN ('tv_user', 'tv_post') $$;
CREATE FUNCTION idx_scans() RETURNS bigint LANGUAGE sql AS $$
    SELECT sum(coalesce(idx_scan, 0))::bigint FROM pg_stat_xact_user_tables
    WHERE relname IN ('tv_user', 'tv_post') $$;

-- The refresh runs at the end of each statement: the counters taken after it cover
-- the report alone.

-- 1. One post changed: its document is read by key.
BEGIN;
UPDATE tb_post SET title = 'changed again' WHERE pk_post = 42;
SELECT seq_rows() AS seq_before, idx_scans() AS idx_before \gset
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(seq_rows() = :seq_before,
            (seq_rows() - :seq_before) || ' row(s) of the TVIEWs read by seq scan');
SELECT must(idx_scans() > :idx_before, 'the report read no row by index');
SELECT must((SELECT e->'data'->>'title' FROM jsonb_array_elements((:'r'::jsonb)->'updated') e
             WHERE e->>'__typename' = 'Post') = 'changed again', 'data: ' || :'r');
COMMIT;

-- 2. A user changed, which fans out to ten posts: two TVIEWs, read by key.
BEGIN;
UPDATE tb_user SET name = 'renamed again' WHERE pk_user = 7;
SELECT seq_rows() AS seq_before, idx_scans() AS idx_before \gset
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(seq_rows() = :seq_before,
            (seq_rows() - :seq_before) || ' row(s) of the TVIEWs read by seq scan');
SELECT must(idx_scans() > :idx_before, 'the fan-out report read no row by index');
SELECT must((SELECT count(*) FROM jsonb_array_elements((:'r'::jsonb)->'updated') e
             WHERE e->'data'->'author'->>'name' = 'renamed again') = 10, 'fan-out: ' || :'r');
SELECT must((SELECT count(*) FROM jsonb_array_elements((:'r'::jsonb)->'updated') e
             WHERE e->>'__typename' = 'User' AND e->'data'->>'name' = 'renamed again') = 1,
            'user: ' || :'r');
COMMIT;

-- 3. A TVIEW named by another key (DISTINCT ON, ADR 0169): its primary key is on the
--    DISTINCT ON column, and the report reads its rows by pk_<entity>, which gets an
--    index of its own.
CREATE TABLE tb_line (pk_line int PRIMARY KEY, fk_post int NOT NULL REFERENCES tb_post,
                      amount int NOT NULL);
INSERT INTO tb_line SELECT g, 1 + g % 5000, g FROM generate_series(1, 10000) g;
SELECT tviews.pg_tviews_create('tv_latest', $$
    SELECT DISTINCT ON (l.fk_post) l.fk_post, l.pk_line AS pk_latest,
           jsonb_build_object('amount', l.amount) AS data
    FROM tb_line l ORDER BY l.fk_post, l.pk_line DESC $$);
ANALYZE tv_latest;
CREATE FUNCTION latest_seq_rows() RETURNS bigint LANGUAGE sql AS $$
    SELECT seq_tup_read FROM pg_stat_xact_user_tables WHERE relname = 'tv_latest' $$;
BEGIN;
INSERT INTO tb_line VALUES (10001, 42, 7);
SELECT latest_seq_rows() AS seq_before \gset
SELECT tviews.pg_tviews_flush_and_report() AS r \gset
SELECT must(latest_seq_rows() = :seq_before,
            (latest_seq_rows() - :seq_before) || ' row(s) of tv_latest read by seq scan');
SELECT must((SELECT count(*) FROM jsonb_array_elements((:'r'::jsonb)->'updated') e
             WHERE e->>'__typename' = 'Latest' AND (e->'data'->>'amount')::int = 7) = 1,
            'latest: ' || :'r');
COMMIT;

\echo 'report reads by key: PASS'
