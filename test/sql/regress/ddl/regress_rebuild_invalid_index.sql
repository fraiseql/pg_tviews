-- A rebuilt replace refuses, before it drops anything, while a user index on the
-- TVIEW's table is invalid (#219).
--
-- A failed CREATE UNIQUE INDEX CONCURRENTLY leaves an invalid index behind. A
-- rebuild would re-create it, valid, on the empty table, then fail the fill on the
-- duplicates the index could not hold, naming only the violated constraint.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_rebuild_invalid_index.sql
-- expect-output: rebuild_invalid_index: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'rebuild_invalid_index FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ': ' || SQLERRM; END $$;

CREATE TABLE tb_item (pk_item bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      name text);
INSERT INTO tb_item (pk_item, name) VALUES (1, 'same'), (2, 'same'), (3, 'other');
SELECT tviews.pg_tviews_create_or_replace('public.tv_item', $$
    SELECT pk_item, id, name, jsonb_build_object('name', name) AS data FROM tb_item $$);

-- Fails on the duplicates, leaving item_name_key invalid.
\set ON_ERROR_STOP off
CREATE UNIQUE INDEX CONCURRENTLY item_name_key ON tv_item (name);
\set ON_ERROR_STOP on
SELECT must((SELECT NOT indisvalid FROM pg_index WHERE indexrelid = 'item_name_key'::regclass),
            'item_name_key should be invalid');

SELECT COALESCE(error_of($x$SELECT tviews.pg_tviews_create_or_replace('public.tv_item', $$
    SELECT pk_item, id, name, upper(name) AS shout,
           jsonb_build_object('name', name) AS data FROM tb_item $$)$x$), 'no error') AS err \gset
SELECT must(:'err' LIKE '55000: %item_name_key%', 'rebuild over an invalid index: ' || :'err');
SELECT must((SELECT count(*) FROM tv_item) = 3, 'rows lost by the refused rebuild');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = 'tv_item'::regclass
                                                  AND attname = 'shout'), 'refused rebuild ran');

-- Once the index is dropped, the rebuild runs and the TVIEW keeps refreshing.
DROP INDEX item_name_key;
SELECT must(tviews.pg_tviews_create_or_replace('public.tv_item', $$
    SELECT pk_item, id, name, upper(name) AS shout,
           jsonb_build_object('name', name) AS data FROM tb_item $$) = 'rebuilt', 'rebuild');
UPDATE tb_item SET name = 'new' WHERE pk_item = 3;
SELECT assert_fresh('tv_item', 'pk_item', 'after rebuild');

DROP EXTENSION pg_tviews CASCADE;

SELECT 'rebuild_invalid_index: PASS';
