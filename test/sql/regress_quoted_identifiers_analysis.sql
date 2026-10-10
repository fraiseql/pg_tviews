-- A definition is read as PostgreSQL reads it: quoted identifiers, `SELECT *`
-- and unqualified names resolve exactly as in the view itself.
--
-- (a) A column named with an apostrophe is accepted, copied into `data`, and
--     refreshed.
-- (b) `SELECT *` over a view with such a column is written out with the column
--     quoted.
-- (c) `SELECT * FROM tb_y` names public.tb_y's columns, not those of a same-named
--     table in another schema.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_quoted_identifiers_analysis.sql
-- expect-output: quoted identifiers analysis: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'quoted identifiers FAIL: %', what; END IF; END $$;

-- (a)
CREATE TABLE tb_x (pk_x int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), "it's" text);
INSERT INTO tb_x (pk_x, "it's") VALUES (1, 'a'), (2, 'b');
SELECT pg_tviews_create('tv_x', $q$SELECT pk_x, id, x."it's",
    jsonb_build_object('id', id, 'v', x."it's") AS data FROM tb_x x$q$);
UPDATE tb_x SET "it's" = 'a2' WHERE pk_x = 1;
SELECT assert_fresh('tv_x', 'pk_x', '(a) an apostrophe in a column name');

-- (b)
CREATE TABLE tb_w (pk_w int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), "it's" text);
INSERT INTO tb_w (pk_w, "it's") VALUES (1, 'w');
CREATE VIEW v_w AS SELECT pk_w, id, "it's", jsonb_build_object('id', id, 'v', "it's") AS data FROM tb_w;
SELECT pg_tviews_create('tv_w', 'SELECT * FROM v_w');
SELECT must(definition LIKE '%"it''s"%', format('(b) the column is quoted: %s', definition))
FROM tviews.pg_tview_meta WHERE entity = 'w';
UPDATE tb_w SET "it's" = 'w2';
SELECT assert_fresh('tv_w', 'pk_w', '(b) SELECT * with an apostrophe column');

-- (c)
CREATE SCHEMA aaa;
CREATE TABLE aaa.tb_y (pk_y int PRIMARY KEY, id uuid, data jsonb, planted text);
CREATE TABLE tb_y (pk_y int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), data jsonb);
INSERT INTO tb_y (pk_y, data) VALUES (1, '{"k": 1}');
SELECT pg_tviews_create('tv_y', 'SELECT * FROM tb_y');
SELECT must(definition NOT LIKE '%planted%', format('(c) another schema steered SELECT *: %s', definition))
FROM tviews.pg_tview_meta WHERE entity = 'y';
UPDATE tb_y SET data = '{"k": 2}';
SELECT assert_fresh('tv_y', 'pk_y', '(c) SELECT * of an unqualified table');

SELECT 'quoted identifiers analysis: PASS' AS result;
