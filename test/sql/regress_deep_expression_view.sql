-- A definition too deep for the stack fails with an ERROR, not a crash.
--
-- pg_tviews walks a definition's parse tree with PostgreSQL's tree walkers,
-- which check the stack depth. A definition nested beyond max_stack_depth must
-- raise "stack depth limit exceeded" back through pg_tviews' callbacks, and the
-- session must go on.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_deep_expression_view.sql
-- expect-output: deep expression view: clean error

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n int);

-- n + 1 + 1 + … deep enough to pass the parser under the default stack, then
-- analysed under a small one.
SELECT 'SELECT pk_x, id, jsonb_build_object(''n'', ' || repeat('(', 3000) || 'n'
       || repeat(' + 1)', 3000) || ') AS data FROM tb_x' AS deep \gset
SELECT set_config('regress.deep', :'deep', false) IS NOT NULL AS ready;
SET max_stack_depth = '200kB';
DO $$
BEGIN
    PERFORM pg_tviews_create('tv_x', current_setting('regress.deep'));
    RAISE EXCEPTION 'the deep definition was accepted';
EXCEPTION WHEN statement_too_complex THEN
    NULL;
END $$;
RESET max_stack_depth;
SELECT 1 AS session_alive;

\echo 'deep expression view: clean error'
