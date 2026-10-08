-- Regression test for issue #199: DROP EXTENSION pg_tviews CASCADE takes the
-- TVIEWs' backing views along. The tv_* tables stay, as plain tables holding
-- their rows, and re-creating the extension and the same TVIEW works.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_drop_extension.sql
--
-- expect-output: issue #199 drop extension: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION '#199 FAIL: %', what; END IF; END $$;
-- Relations left in the extension's schema (whatever it is called).
CREATE FUNCTION left_in_tviews() RETURNS text LANGUAGE sql AS $$
    SELECT coalesce(string_agg(c.relname || '(' || c.relkind::text || ')', ', ' ORDER BY c.relname), '')
    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'tviews' AND c.relkind IN ('v', 'r', 'm') $$;

CREATE TABLE tb_a (pk_a bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_a VALUES (1, default, 'a1'), (2, default, 'a2');
SELECT tviews.pg_tviews_create('tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
CREATE SCHEMA s;
CREATE TABLE s.tb_c (pk_c bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO s.tb_c VALUES (1, default, 'c1');
SELECT tviews.pg_tviews_create('s.tv_c', 'SELECT pk_c, id, name FROM s.tb_c');

-- 1. The issue: the extension goes, with the backing views.
DROP EXTENSION pg_tviews CASCADE;
SELECT must(left_in_tviews() = '', 'left in tviews after DROP EXTENSION: ' || left_in_tviews());
SELECT must((SELECT relkind FROM pg_class WHERE oid = 'public.tv_a'::regclass) = 'r', 'tv_a kept as a table');
SELECT must((SELECT count(*) FROM public.tv_a) = 2, 'tv_a kept its rows');
SELECT must((SELECT count(*) FROM s.tv_c) = 1, 's.tv_c kept its rows');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_trigger WHERE NOT tgisinternal AND tgrelid = 'tb_a'::regclass),
            'triggers left on tb_a');

-- 2. Re-create the extension and the same TVIEWs.
CREATE EXTENSION pg_tviews;
DROP TABLE public.tv_a, s.tv_c;
SELECT tviews.pg_tviews_create('tv_a', 'SELECT pk_a, id, name FROM public.tb_a');
SELECT tviews.pg_tviews_create('s.tv_c', 'SELECT pk_c, id, name FROM s.tb_c');
UPDATE tb_a SET name = 'a1b' WHERE pk_a = 1;
SELECT must((SELECT name FROM tv_a WHERE pk_a = 1) = 'a1b', 'the re-created TVIEW refreshes');

-- 3. DROP EXTENSION … RESTRICT with TVIEWs registered fails and changes nothing.
DO $$
BEGIN
    DROP EXTENSION pg_tviews;
    RAISE EXCEPTION '#199 FAIL: DROP EXTENSION RESTRICT succeeded';
EXCEPTION WHEN dependent_objects_still_exist THEN NULL;
END $$;
SELECT must(left_in_tviews() LIKE '%public__tv_a(v)%', 'RESTRICT failure lost the backing view: ' || left_in_tviews());

-- 4. Backstop (decision D2): a view left at a backing name by a drop that the
-- library did not see (not preloaded) is reclaimed by pg_tviews_create.
CREATE TABLE tb_b (pk_b bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_b VALUES (1, default, 'b1');
CREATE VIEW tviews.public__tv_b AS SELECT 1 AS planted;
SELECT tviews.pg_tviews_create('tv_b', 'SELECT pk_b, id, name FROM public.tb_b');
SELECT must(NOT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = 'tviews.public__tv_b'::regclass
                                                     AND attname = 'planted'),
            'the planted view was kept');
UPDATE tb_b SET name = 'b2';
SELECT must((SELECT name FROM tv_b) = 'b2', 'tv_b refreshes');

SELECT 'issue #199 drop extension: PASS' AS result;
