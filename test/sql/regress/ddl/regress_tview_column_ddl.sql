-- Regression test for issue #208:
--   "Renaming a tv_* column is accepted, then every write to its base tables fails."
--
-- A TVIEW's columns are its definition's: the stored refresh names them. Renaming,
-- dropping or retyping a column of a TVIEW table directly left the refresh naming a
-- column that no longer exists (or no longer fits), and every later write to a base
-- table failed.
--
-- Correct behaviour: RENAME COLUMN and DROP COLUMN on a TVIEW table are refused
-- with 42809 (wrong_object_type) and a hint naming pg_tviews_create_or_replace,
-- and so is ALTER COLUMN TYPE to a type the backing view's type does not convert
-- to on assignment. The TVIEW keeps refreshing. A type the view's converts to is
-- accepted (a TVIEW may keep it until create_or_replace retypes it). The same
-- statements on a plain tv_* table that is no TVIEW, and on base tables, still run.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/ddl/regress_tview_column_ddl.sql
-- expect-output: tview column ddl: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'tview column ddl FAIL: %', what; END IF; END $$;
-- 'SQLSTATE: message | hint' of the error a statement raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE hint text;
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS hint = PG_EXCEPTION_HINT;
    RETURN SQLSTATE || ': ' || SQLERRM || ' | ' || hint;
END $$;
CREATE FUNCTION expect_refused(stmt text, what text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE got text := error_of(stmt);
BEGIN
    PERFORM must(got LIKE '42809: %pg_tviews_create_or_replace%',
                 format('%s: expected a 42809 refusal naming pg_tviews_create_or_replace, got %s',
                        what, coalesce(got, 'no error')));
END $$;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'ada');
SELECT pg_tviews_create('tv_user', $$
    SELECT pk_user, id, name, jsonb_build_object('name', name) AS data FROM tb_user $$);

-- The column statements, alone and in a multi-command ALTER TABLE, schema-qualified too.
SELECT expect_refused($$ALTER TABLE tv_user RENAME COLUMN name TO display_name$$, 'RENAME COLUMN');
SELECT expect_refused($$ALTER TABLE public.tv_user RENAME name TO display_name$$, 'RENAME (qualified)');
SELECT expect_refused($$ALTER TABLE tv_user DROP COLUMN name$$, 'DROP COLUMN');
SELECT expect_refused($$ALTER TABLE tv_user ALTER COLUMN name TYPE integer USING 0$$, 'ALTER COLUMN TYPE integer');
SELECT expect_refused($$ALTER TABLE tv_user ALTER COLUMN data TYPE text$$, 'ALTER COLUMN data TYPE text');
SELECT expect_refused($$ALTER TABLE tv_user SET (fillfactor = 90), DROP COLUMN name$$, 'DROP COLUMN among other commands');

-- Nothing changed, and writes keep refreshing the TVIEW.
SELECT must(EXISTS (SELECT FROM pg_attribute WHERE attrelid = 'tv_user'::regclass AND attname = 'name'
                    AND NOT attisdropped AND atttypid = 'text'::regtype), 'tv_user.name changed');
UPDATE tb_user SET name = 'grace' WHERE pk_user = 1;
INSERT INTO tb_user (pk_user, name) VALUES (2, 'linus');
SELECT must((SELECT name FROM tv_user WHERE pk_user = 1) = 'grace', 'update did not refresh');
SELECT must((SELECT count(*) FROM tv_user) = 2, 'insert did not refresh');
SELECT assert_fresh('tv_user', 'pk_user', 'writes after the refused DDL');

-- Other ALTER TABLE forms on a TVIEW table still run, and so does a type the
-- refresh writes through.
ALTER TABLE tv_user SET (fillfactor = 90);
ALTER TABLE tv_user ALTER COLUMN data SET STORAGE MAIN;
ALTER TABLE tv_user ALTER COLUMN name TYPE varchar;
UPDATE tb_user SET name = 'ada l' WHERE pk_user = 1;
SELECT must((SELECT name FROM tv_user WHERE pk_user = 1) = 'ada l', 'update after a compatible retype');

-- A tv_* table that is no TVIEW, and the base table, are left alone.
CREATE TABLE tv_plain (a int, b int);
ALTER TABLE tv_plain RENAME COLUMN a TO c;
ALTER TABLE tv_plain DROP COLUMN b;
ALTER TABLE tv_plain ALTER COLUMN c TYPE bigint;
ALTER TABLE tb_user RENAME COLUMN name TO full_name;
UPDATE tb_user SET full_name = 'grace h' WHERE pk_user = 1;
SELECT must((SELECT name FROM tv_user WHERE pk_user = 1) = 'grace h', 'base-table rename stopped refreshes');

\echo 'tview column ddl: PASS'
