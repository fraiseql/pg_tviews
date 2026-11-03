-- A TVIEW name carrying SQL is refused, never executed: pg_tviews_create,
-- pg_tviews_drop and pg_tviews_refresh validate the name (22023) before any
-- statement is built from it, and the table the payload names survives.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_name_injection.sql
-- expect-output: name_injection: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'name injection FAIL: %', what; END IF; END $$;
-- 'SQLSTATE message' of the error a statement raises, or 'ok'.
CREATE FUNCTION outcome(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'ok';
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ' ' || SQLERRM; END $$;

CREATE TABLE tb_x (pk_x int PRIMARY KEY);
CREATE TABLE y (n int);

SELECT must(outcome(stmt) LIKE '22023 %contains invalid characters%', stmt || ': ' || outcome(stmt))
FROM unnest(ARRAY[
    $$SELECT pg_tviews_create('tv_x; DROP TABLE tb_x; --', 'SELECT 1')$$,
    $$SELECT pg_tviews_create('tv_x''; DROP TABLE tb_x; --', 'SELECT 1')$$,
    $$SELECT pg_tviews_drop('tv_x''; DROP TABLE y; --')$$,
    $$SELECT pg_tviews_refresh('x; DROP TABLE y; --')$$
]) AS stmt;

SELECT must(to_regclass('tb_x') IS NOT NULL AND to_regclass('y') IS NOT NULL,
            'a table named in an injected name was dropped');

\echo 'name_injection: PASS'
