-- Regression test: the SELECT of a CREATE TABLE tv_* AS statement is cut from the
-- statement's text as PostgreSQL's lexer reads it. Comments (with quotes or the
-- table's name in them), quoted and non-ASCII names, schema qualification, IF NOT
-- EXISTS, dollar-quoted literals and a batch of several statements each keep the
-- SELECT as written; WITH NO DATA is still refused, and a CTAS PostgreSQL
-- rejects keeps its SQLSTATE.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_ctas_text.sql

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'CTAS TEXT FAIL: %', what; END IF; END $$;
-- The stored definition of `entity`, or NULL.
CREATE FUNCTION definition_of(entity text) RETURNS text LANGUAGE sql AS $$
    SELECT m.definition FROM tviews.pg_tview_meta m WHERE m.entity = $1 $$;

CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_x (pk_x, name) VALUES (1, 'a');

-- 1. A trailing semicolon is not part of the SELECT.
CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x;
SELECT must(definition_of('x') = $q$SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x$q$,
            'plain: ' || definition_of('x'));
DROP TABLE tv_x;

-- 2. Lower case, IF NOT EXISTS, a schema, a line break after AS.
create table if not exists public.tv_x as
  select pk_x, id, jsonb_build_object('n', name) as data from tb_x;
SELECT must(definition_of('x') = $q$select pk_x, id, jsonb_build_object('n', name) as data from tb_x$q$,
            'lower case: ' || definition_of('x'));
DROP TABLE tv_x;

-- 3. The table's name and AS in a comment before the statement and in a literal.
/* tv_x as */ CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('n', 'tv_x as y') AS data FROM tb_x;
SELECT must(definition_of('x') = $q$SELECT pk_x, id, jsonb_build_object('n', 'tv_x as y') AS data FROM tb_x$q$,
            'comment before: ' || definition_of('x'));
DROP TABLE tv_x;

-- 4. A comment with an apostrophe right after AS, and one between the name and AS:
--    the SELECT starts at its first token.
--    (sent through EXECUTE: psql drops a -- comment from the text it sends).
DO $$ BEGIN
    EXECUTE E'CREATE TABLE tv_x /* as select 1 */ AS -- don''t\nSELECT pk_x, id, jsonb_build_object(''n'', name) AS data FROM tb_x';
END $$;
SELECT must(definition_of('x') = $q$SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x$q$,
            'comment after AS: ' || definition_of('x'));
DROP TABLE tv_x;

-- 5. Quoted names, and a non-ASCII one.
CREATE TABLE "public"."tv_x" AS SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x;
SELECT must(definition_of('x') LIKE 'SELECT pk_x, id,%', 'quoted: ' || definition_of('x'));
DROP TABLE tv_x;
CREATE TABLE tb_café (pk_café int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid());
INSERT INTO tb_café (pk_café) VALUES (1);
CREATE TABLE tv_café AS SELECT pk_café, id, jsonb_build_object('é', 'ü') AS "data" FROM tb_café;
SELECT must(definition_of('café') = $q$SELECT pk_café, id, jsonb_build_object('é', 'ü') AS "data" FROM tb_café$q$,
            'non-ASCII: ' || definition_of('café'));
DROP TABLE tv_café;

-- 6. A dollar-quoted literal holding AS and a semicolon.
CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('n', $d$tv_x AS; x$d$) AS data FROM tb_x;
SELECT must(definition_of('x') = $q$SELECT pk_x, id, jsonb_build_object('n', $d$tv_x AS; x$d$) AS data FROM tb_x$q$,
            'dollar quotes: ' || definition_of('x'));
DROP TABLE tv_x;

-- 7. One statement of a batch: only its own text.
SELECT 1 \; CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x \; SELECT 2;
SELECT must(definition_of('x') = $q$SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x$q$,
            'batch: ' || definition_of('x'));
DROP TABLE tv_x;

-- 8. WITH NO DATA is refused: a TVIEW is always populated.
DO $$
BEGIN
    BEGIN
        EXECUTE $s$CREATE TABLE tv_x AS SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x WITH NO DATA$s$;
        RAISE EXCEPTION 'CTAS TEXT FAIL: WITH NO DATA was accepted';
    EXCEPTION WHEN feature_not_supported THEN
        NULL;
    END;
END $$;
SELECT must(to_regclass('tv_x') IS NULL, 'WITH NO DATA left a table');

-- 9. A CTAS PostgreSQL itself rejects keeps its SQLSTATE through the hook.
DO $$
BEGIN
    BEGIN
        EXECUTE 'CREATE TABLE tv_x AS SELECT pk_x, id, data FROM no_such_table';
        RAISE EXCEPTION 'CTAS TEXT FAIL: a missing relation was accepted';
    EXCEPTION WHEN undefined_table THEN
        NULL;
    END;
    BEGIN
        EXECUTE $s$CREATE TABLE no_such_schema.tv_x AS SELECT pk_x, id, jsonb_build_object('n', name) AS data FROM tb_x$s$;
        RAISE EXCEPTION 'CTAS TEXT FAIL: a missing schema was accepted';
    EXCEPTION WHEN invalid_schema_name THEN
        NULL;
    END;
END $$;

\echo 'PASS: regress_ctas_text'
