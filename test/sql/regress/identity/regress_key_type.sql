-- Regression test for issue #209:
--   "A uuid pk_<entity> fails pg_tviews_create with a raw 42804 instead of a refusal."
--
-- pg_tviews keys a TVIEW's rows on pk_<entity> as an integer: the TVIEW table's key
-- column is BIGINT, and the queue, journal and propagation plan carry keys as
-- 64-bit integers. A definition whose pk_<entity> was a uuid created the backing
-- view and table, then failed filling them with PostgreSQL's 42804.
--
-- Correct behaviour: such a definition is refused before any object is created,
-- with 42804 (datatype_mismatch), a message naming the column, the type found and
-- the rule, and a hint to keep the uuid in `id`. smallint, integer, bigint and a
-- domain over them still work.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/identity/regress_key_type.sql
-- expect-output: key type: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'key type FAIL: %', what; END IF; END $$;
-- 'SQLSTATE: message | hint' of the error a statement raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
DECLARE hint text;
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN
    GET STACKED DIAGNOSTICS hint = PG_EXCEPTION_HINT;
    RETURN SQLSTATE || ': ' || SQLERRM || ' | ' || hint;
END $$;

CREATE TABLE tb_doc (pk_doc uuid PRIMARY KEY DEFAULT gen_random_uuid(),
                     id uuid NOT NULL DEFAULT gen_random_uuid(), title text);
INSERT INTO tb_doc (title) VALUES ('a');

-- uuid key: refused up front, from pg_tviews_create and from CTAS.
DO $$
DECLARE got text := error_of($q$SELECT pg_tviews_create('tv_doc', 'SELECT pk_doc, id, jsonb_build_object(''title'', title) AS data FROM tb_doc')$q$);
BEGIN
    PERFORM must(got LIKE '42804: %pk_doc is uuid: %integer key%| %id column%',
                 format('uuid key: expected 42804 naming pk_doc and uuid with a hint, got %s', coalesce(got, 'no error')));
END $$;
DO $$
DECLARE got text := error_of($q$CREATE TABLE tv_doc AS SELECT pk_doc, id, jsonb_build_object('title', title) AS data FROM tb_doc$q$);
BEGIN
    PERFORM must(got LIKE '42804: %pk_doc is uuid: %integer key%', format('uuid key via CTAS: got %s', coalesce(got, 'no error')));
END $$;
SELECT must(to_regclass('tv_doc') IS NULL, 'tv_doc was created');
SELECT must(NOT EXISTS (SELECT FROM pg_class WHERE relname LIKE '%tv_doc'), 'a backing view was left behind');
SELECT must(NOT EXISTS (SELECT FROM pg_tview_meta WHERE entity = 'doc'), 'doc was registered');

-- text and numeric keys are refused too.
SELECT must(error_of($q$SELECT pg_tviews_create('tv_doc', 'SELECT title AS pk_doc, id, jsonb_build_object(''t'', title) AS data FROM tb_doc')$q$)
            LIKE '42804: %pk_doc is text: %integer key%', 'text key not refused');
SELECT must(error_of($q$SELECT pg_tviews_create('tv_doc', 'SELECT 1.5::numeric AS pk_doc, id, jsonb_build_object(''t'', title) AS data FROM tb_doc')$q$)
            LIKE '42804: %pk_doc is numeric: %integer key%', 'numeric key not refused');

-- Integer keys of every width, and a domain over one, still work.
CREATE DOMAIN user_key AS bigint CHECK (VALUE > 0);
CREATE TABLE tb_small (pk_small smallint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
CREATE TABLE tb_int (pk_int integer PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
CREATE TABLE tb_dom (pk_dom user_key PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO tb_small (pk_small, n) VALUES (1, 'a');
INSERT INTO tb_int (pk_int, n) VALUES (1, 'a');
INSERT INTO tb_dom (pk_dom, n) VALUES (1, 'a');
SELECT pg_tviews_create('tv_small', $$SELECT pk_small, id, jsonb_build_object('n', n) AS data FROM tb_small$$);
SELECT pg_tviews_create('tv_int', $$SELECT pk_int, id, jsonb_build_object('n', n) AS data FROM tb_int$$);
SELECT pg_tviews_create('tv_dom', $$SELECT pk_dom, id, jsonb_build_object('n', n) AS data FROM tb_dom$$);
UPDATE tb_small SET n = 'b';
UPDATE tb_int SET n = 'b';
UPDATE tb_dom SET n = 'b';
SELECT assert_fresh('tv_small', 'pk_small', 'an update');
SELECT assert_fresh('tv_int', 'pk_int', 'an update');
SELECT assert_fresh('tv_dom', 'pk_dom', 'an update');

-- Replacing a definition with a uuid key is refused the same way, and the TVIEW stays.
SELECT must(error_of($q$SELECT pg_tviews_create_or_replace('tv_int', 'SELECT id AS pk_int, id, jsonb_build_object(''n'', n) AS data FROM tb_int')$q$)
            LIKE '42804: %pk_int is uuid: %integer key%', 'create_or_replace with a uuid key not refused');
SELECT must(error_of($q$SELECT pg_tviews_create_or_replace('tv_doc', 'SELECT pk_doc, id, jsonb_build_object(''t'', title) AS data FROM tb_doc')$q$)
            LIKE '42804: %pk_doc is uuid: %integer key%', 'create_or_replace of a new uuid-keyed TVIEW not refused');
UPDATE tb_int SET n = 'c';
SELECT assert_fresh('tv_int', 'pk_int', 'an update');

\echo 'key type: PASS'
