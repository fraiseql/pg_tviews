-- Issue #267: a refresh of a UNION TVIEW reads its backing view once.
--
-- A key names one row (ADR 0216): a UNION view returning several rows for one key
-- is refused. The refusal is decided from the rows the upsert reads, which then
-- writes none; a second read of the view, which cost as much as the upsert's own
-- whenever a branch's filter or joins decide which rows exist, is gone.
--
-- `shown()` counts the rows each read of the view filters (track_functions; the
-- counts a transaction reads are its own once the backend's were flushed),
-- compared with the same writes through the first branch alone.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_union_read_once.sql
-- expect-output: union_read_once: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'union_read_once FAIL: %', what; END IF; END $$;

CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
CREATE TABLE tb_comment (pk_comment bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_post bigint NOT NULL REFERENCES tb_post, body text);
INSERT INTO tb_post (pk_post, title) SELECT g, 'p' || g FROM generate_series(1, 20) g;
INSERT INTO tb_comment (pk_comment, fk_post, body) SELECT g, 1 + g % 20, 'c' || g FROM generate_series(1, 40) g;
CREATE FUNCTION shown(title text) RETURNS boolean LANGUAGE plpgsql IMMUTABLE AS
$$ BEGIN RETURN title <> ''; END $$;
CREATE FUNCTION calls() RETURNS bigint LANGUAGE sql AS
$$ SELECT pg_catalog.pg_stat_get_xact_function_calls('shown'::regproc) $$;

-- The baseline: the first branch alone, a TVIEW with no set operation.
SELECT tviews.pg_tviews_create('tv_feed', $$
    SELECT p.pk_post * 2 AS pk_feed, p.id, jsonb_build_object('title', p.title) AS data
    FROM tb_post p WHERE public.shown(p.title) $$);
SET track_functions = 'pl';
UPDATE tb_post SET title = title || '!' WHERE pk_post = 3;
SELECT pg_stat_force_next_flush();
BEGIN;
UPDATE tb_post SET title = title || '!' WHERE pk_post = 3;
SELECT calls() AS one_key \gset
COMMIT;
SELECT pg_stat_force_next_flush();
BEGIN;
UPDATE tb_post SET title = title || '!' WHERE pk_post <= 3;
SELECT calls() AS three_keys \gset
COMMIT;
SELECT tviews.pg_tviews_drop('tv_feed');

-- The same rows and a second branch, keys disjoint by construction (posts even,
-- comments odd): the second branch filters no row for a post's key.
SELECT tviews.pg_tviews_create('tv_feed', $$
    SELECT p.pk_post * 2 AS pk_feed, p.id, jsonb_build_object('title', p.title) AS data
    FROM tb_post p WHERE public.shown(p.title)
    UNION ALL
    SELECT c.pk_comment * 2 + 1 AS pk_feed, c.id, jsonb_build_object('body', c.body) AS data
    FROM tb_comment c WHERE public.shown(c.body) $$);
UPDATE tb_post SET title = title || '!' WHERE pk_post = 3;
SELECT pg_stat_force_next_flush();
BEGIN;
UPDATE tb_post SET title = title || '!' WHERE pk_post = 3;
SELECT must(calls() = :one_key,
            format('a write refreshing one key filtered %s rows, %s without a set operation',
                   calls(), :one_key));
COMMIT;
SELECT assert_fresh('tv_feed', 'pk_feed', 'one key');
SELECT pg_stat_force_next_flush();
BEGIN;
UPDATE tb_post SET title = title || '!' WHERE pk_post <= 3;
SELECT must(calls() = :three_keys,
            format('a write refreshing 3 keys filtered %s rows, %s without a set operation',
                   calls(), :three_keys));
COMMIT;
SELECT assert_fresh('tv_feed', 'pk_feed', 'several keys');

\echo 'union_read_once: PASS'
