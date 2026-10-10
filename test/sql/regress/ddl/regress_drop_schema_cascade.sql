-- Regression test for issue #186: a TVIEW's table dropped with its schema
-- (DROP SCHEMA … CASCADE) or with its owner's objects (DROP OWNED BY) takes its
-- backing view in tviews along, even when the base tables live elsewhere, so the
-- TVIEW can be created again under the same name.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_drop_schema_cascade.sql
--
-- expect-output: issue #186 drop schema cascade: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#186 FAIL: %', what; END IF; END $$;
CREATE FUNCTION views_left(prefix text) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM pg_class
    WHERE relnamespace = 'tviews'::regnamespace AND relname LIKE prefix || '\_\_%' $$;

CREATE TABLE tb_a (pk_a bigint PRIMARY KEY, id uuid DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_b (pk_b bigint PRIMARY KEY, id uuid DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_a (pk_a, name) VALUES (1, 'a');
INSERT INTO tb_b (pk_b, name) VALUES (1, 'b');
CREATE SCHEMA s;
CREATE SCHEMA keep;
SELECT tviews.pg_tviews_create('s.tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
SELECT tviews.pg_tviews_create('keep.tv_b', 'SELECT pk_b, id, name FROM public.tb_b');
-- Something reading the backing view goes with it, as it went with the schema
-- before backing views moved to tviews.
CREATE VIEW public.reads_backing_view AS SELECT * FROM tviews.s__tv_a;

-- 1. The issue: the schema goes, the backing view goes with it.
DROP SCHEMA s CASCADE;
SELECT must(views_left('s') = 0, 'DROP SCHEMA s CASCADE left tviews.s__tv_a');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.registry WHERE schema = 's'), 'registration left');
SELECT must(to_regclass('public.reads_backing_view') IS NULL,
            'a view reading the backing view survived the cascade');
SELECT must(views_left('keep') = 1, 'the TVIEW of another schema lost its backing view');

-- 2. The same TVIEW again, and it refreshes.
CREATE SCHEMA s;
SELECT tviews.pg_tviews_create('s.tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
UPDATE tb_a SET name = 'a2' WHERE pk_a = 1;
SELECT assert_fresh('s.tv_a', 'pk_a', 're-created after DROP SCHEMA CASCADE');
SELECT must((SELECT name FROM s.tv_a WHERE pk_a = 1) = 'a2', 're-created TVIEW not refreshed');
UPDATE tb_b SET name = 'b2' WHERE pk_b = 1;
SELECT must((SELECT name FROM keep.tv_b WHERE pk_b = 1) = 'b2', 'the other TVIEW stopped refreshing');

-- 3. DROP OWNED BY: the TVIEW's owner's objects go, the backing view too.
DROP ROLE IF EXISTS r186;
CREATE ROLE r186;
GRANT USAGE, CREATE ON SCHEMA s TO r186;
GRANT SELECT, TRIGGER ON tb_a TO r186;
SELECT tviews.pg_tviews_drop('s.tv_a');
SET ROLE r186;
SELECT tviews.pg_tviews_create('s.tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
RESET ROLE;
SELECT must(views_left('s') = 1, 'no backing view for the role''s TVIEW');
DROP OWNED BY r186;
SELECT must(views_left('s') = 0, 'DROP OWNED BY left tviews.s__tv_a');
SELECT must(NOT EXISTS (SELECT 1 FROM tviews.registry WHERE schema = 's'), 'registration left (OWNED)');
SELECT tviews.pg_tviews_create('s.tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
DROP ROLE r186;

SELECT 'issue #186 drop schema cascade: PASS' AS result;
