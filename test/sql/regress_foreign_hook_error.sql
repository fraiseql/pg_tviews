-- An error raised under another extension's executor hook leaves no stale
-- statement frame behind.
--
-- pg_tviews keeps a stack of running queries from its ExecutorRun/ExecutorFinish
-- hooks, and defers a statement's flush while a writing query encloses it. When
-- another library's hook runs first (shared_preload_libraries =
-- 'pg_stat_statements,pg_tviews'), an error inside a writing query must still pop
-- that query's frame. Otherwise, after the error is caught by an EXCEPTION
-- block, every later flush in the session is deferred to a statement that no
-- longer exists.
--
-- Meaningful when another executor hook is preloaded (the CI job that preloads
-- pg_stat_statements first); it passes trivially without one.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_foreign_hook_error.sql
-- expect-output: foreign hook error: next statement fresh

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO tb_x SELECT g, gen_random_uuid(), 'a' FROM generate_series(1, 3) g;
SELECT pg_tviews_create('tv_x',
  $q$SELECT pk_x, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_x$q$);

-- A write to a tracked table that fails while it runs (division by zero at
-- execution, not at planning), caught by the EXCEPTION block.
CREATE FUNCTION failing_write() RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        UPDATE tb_x SET n = (1 / (pk_x - pk_x))::text;
    EXCEPTION WHEN division_by_zero THEN
        RETURN 'caught';
    END;
    RETURN 'not raised';
END $$;

SELECT failing_write();

-- Top level: no frame encloses this UPDATE, so it flushes at its end.
UPDATE tb_x SET n = 'after the caught error';
SELECT assert_fresh('tv_x', 'pk_x', 'an UPDATE following an error caught under the executor');

-- The same from inside a writing statement: the error is caught in a function
-- that the UPDATE's SET list calls.
CREATE FUNCTION fail_then(v text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    PERFORM failing_write();
    RETURN v;
END $$;
UPDATE tb_x SET n = fail_then('inside a writing statement') WHERE pk_x = 1;
UPDATE tb_x SET n = 'next statement';
SELECT assert_fresh('tv_x', 'pk_x', 'the statement after a writing statement that caught an error');

\echo 'foreign hook error: next statement fresh'
