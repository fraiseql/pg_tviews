-- DROP TABLE of a TVIEW and a plain table, from a function called twice.
--
-- plpgsql caches the plan of each statement it runs: the second call gets the
-- same, read-only, parse tree. pg_tviews drops the TVIEW itself and hands the
-- rest of the list to PostgreSQL; it must not edit the cached tree, or the
-- second call no longer sees the TVIEW in it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_drop_in_cached_plan.sql
-- expect-output: drop in cached plan: both calls dropped both

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);
CREATE FUNCTION make_both() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_tviews_create('tv_a',
      'SELECT pk_a, id, jsonb_build_object(''n'', n) AS data FROM tb_a');
    CREATE TABLE plain_t (x int);
END $$;
CREATE FUNCTION drop_both() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TABLE tv_a, plain_t;
END $$;

SELECT make_both();
SELECT drop_both();
SELECT make_both();
SELECT drop_both();

DO $$ BEGIN
    IF to_regclass('tv_a') IS NOT NULL OR to_regclass('plain_t') IS NOT NULL THEN
        RAISE EXCEPTION 'the second call left %', concat_ws(', ', to_regclass('tv_a'), to_regclass('plain_t'));
    END IF;
    IF EXISTS (SELECT 1 FROM tviews.pg_tview_meta) THEN
        RAISE EXCEPTION 'tv_a is still registered';
    END IF;
END $$;

\echo 'drop in cached plan: both calls dropped both'
