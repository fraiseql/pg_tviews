-- A definition nested deeper than max_stack_depth allows fails with
-- 54001 stack depth limit exceeded and leaves nothing behind; one within it is
-- created. The recursive walks over a definition's tree check the stack depth
-- as PostgreSQL's own walkers do.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/lineage/regress_deep_definition.sql
-- expect-output: deep_definition: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'deep_definition FAIL: %', what; END IF; END $$;

CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), k int);
CREATE TABLE tb_b (pk_b int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_a int, x int);
INSERT INTO tb_a (pk_a, k) VALUES (1, 301);
INSERT INTO tb_b (pk_b, fk_a, x) VALUES (1, 1, 1);

-- A join condition and an output nested `depth` levels deep.
CREATE FUNCTION deep(depth int) RETURNS text LANGUAGE sql AS $$
    SELECT 'SELECT b.pk_b, b.id, jsonb_build_object(''x'', ' || repeat('(1 + ', depth) || 'b.x'
           || repeat(')', depth) || ') AS data FROM tb_b b JOIN tb_a a ON a.pk_a = b.fk_a AND a.k = '
           || repeat('(1 + ', depth) || 'b.x' || repeat(')', depth) $$;

SELECT tviews.pg_tviews_create('tv_b', deep(300));
SELECT must((SELECT data->>'x' FROM tv_b) = '301', 'the 300-level TVIEW has the wrong row');

SELECT tviews.pg_tviews_drop('tv_b');
DO $$
BEGIN
    PERFORM tviews.pg_tviews_create('tv_b', deep(3000));
    RAISE EXCEPTION 'deep_definition FAIL: a 3000-level definition was accepted';
EXCEPTION WHEN statement_too_complex THEN
    NULL;
END $$;
SELECT must(to_regclass('tv_b') IS NULL, 'the refused TVIEW left its table');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.pg_tview_meta WHERE entity = 'b'),
            'the refused TVIEW left its catalog row');

SELECT 'deep_definition: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
