-- Every pg_tviews error reaches the client with its documented SQLSTATE, a
-- one-line message, and the same code whichever entry point raised it.
--
-- Clients catch errors by SQLSTATE (`WHEN undefined_object`, `WHEN sqlstate
-- '42P17'`), so this file asserts codes, not message text. CTAS and
-- pg_tviews_create report the same failure with the same code. A definition that
-- would make TVIEWs read each other in a cycle is refused when it is registered,
-- and the TVIEWs it would have broken keep refreshing.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/operations/regress_error_codes.sql
-- expect-output: error codes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'error codes FAIL: %', what; END IF; END $$;
-- 'SQLSTATE: message' of the error a statement raises, or NULL.
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN NULL;
EXCEPTION WHEN OTHERS THEN RETURN SQLSTATE || ': ' || SQLERRM; END $$;
-- Fails unless `stmt` raises `code` with a single-line message.
CREATE FUNCTION expect_code(stmt text, code text, what text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    got text := error_of(stmt);
BEGIN
    PERFORM must(got LIKE code || ': %', format('%s: expected %s, got %s', what, code, coalesce(got, 'no error')));
    PERFORM must(position(E'\n' IN got) = 0, format('%s: multi-line message %L', what, got));
END $$;

CREATE TABLE tb_a (pk_a int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO tb_a SELECT g, gen_random_uuid(), 'x' FROM generate_series(1, 5) g;
SELECT pg_tviews_create('tv_a', $$SELECT pk_a, id, jsonb_build_object('id', id, 'n', n) AS data FROM tb_a$$);

-- 42704 undefined_object: no such TVIEW, whichever function names it.
SELECT expect_code($$SELECT pg_tviews_refresh('nope')$$, '42704', 'refresh unknown');
SELECT expect_code($$SELECT pg_tviews_drop('tv_nope')$$, '42704', 'drop unknown');
SELECT expect_code($$SELECT pg_tviews_reregister('tv_nope')$$, '42704', 'reregister unknown');
SELECT expect_code($$SELECT pg_tviews_set_logged('nope', true)$$, '42704', 'set_logged unknown');

-- 42P07 duplicate_table: from pg_tviews_create and from CTAS alike.
SELECT expect_code($$SELECT pg_tviews_create('tv_a', 'SELECT pk_a, id, jsonb_build_object(''id'', id) AS data FROM tb_a')$$,
                   '42P07', 'create existing');
SELECT expect_code($$CREATE TABLE tv_a AS SELECT pk_a, id, jsonb_build_object('id', id) AS data FROM tb_a$$,
                   '42P07', 'CTAS existing');

-- 42601 syntax_error: not one SELECT; 42703 undefined_column: a column that does
-- not exist, from either entry point (PostgreSQL analyzes CTAS before pg_tviews).
SELECT expect_code($$SELECT pg_tviews_create('tv_b',
    'SELECT pk_a AS pk_b, id, jsonb_build_object(''id'', id) AS data FROM tb_a; CREATE TABLE smuggled (x int)')$$,
                   '42601', 'two statements');
SELECT must(to_regclass('smuggled') IS NULL, 'a second statement in a definition ran');
SELECT expect_code($$SELECT pg_tviews_create('tv_b', 'SELEC pk_a FROM tb_a')$$, '42601', 'misspelled');
SELECT expect_code($$SELECT pg_tviews_create('tv_b', 'SELECT nope AS pk_b FROM tb_a')$$, '42703', 'create unknown column');
SELECT expect_code($$CREATE TABLE tv_b AS SELECT nope AS pk_b FROM tb_a$$, '42703', 'CTAS unknown column');

-- 0A000 feature_not_supported: a definition no write can ever refresh.
SELECT expect_code($$SELECT pg_tviews_create('tv_c', 'SELECT 1::bigint AS pk_c, gen_random_uuid() AS id, ''{}''::jsonb AS data')$$,
                   '0A000', 'never refreshed');

-- 55000 object_not_in_prerequisite_state: resume without suspend.
SELECT expect_code($$SELECT pg_tviews_resume_triggers()$$, '55000', 'resume not suspended');

-- 54000 program_limit_exceeded: the refresh queue is full.
SET pg_tviews.max_queue_size = 2;
SELECT expect_code($$UPDATE tb_a SET n = 'y'$$, '54000', 'queue full');
RESET pg_tviews.max_queue_size;
SELECT assert_fresh('tv_a', 'pk_a', 'after a refused write');

-- 42P17 invalid_object_definition: tv_b reads tv_a; tv_a may not read tv_b back.
CREATE TABLE tb_b (pk_b int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), fk_a int NOT NULL REFERENCES tb_a);
INSERT INTO tb_b (pk_b, fk_a) SELECT g, g FROM generate_series(1, 5) g;
SELECT pg_tviews_create('tv_b', $$SELECT b.pk_b, b.id, jsonb_build_object('id', b.id, 'a', a.data) AS data
                                  FROM tb_b b JOIN tv_a a ON a.pk_a = b.fk_a$$);
SELECT expect_code($$SELECT pg_tviews_create_or_replace('tv_a',
    'SELECT t.pk_a, t.id, jsonb_build_object(''id'', t.id, ''b'', b.data) AS data FROM tb_a t LEFT JOIN tv_b b ON b.pk_b = t.pk_a')$$,
                   '42P17', 'cycle');
UPDATE tb_a SET n = 'after the refused cycle' WHERE pk_a = 1;
SELECT assert_fresh('tv_a', 'pk_a', 'tv_a after the refused cycle');
SELECT assert_fresh('tv_b', 'pk_b', 'tv_b after the refused cycle');

-- 42501 insufficient_privilege: refreshing a TVIEW the role does not own.
DROP ROLE IF EXISTS regress_error_codes_stranger;
CREATE ROLE regress_error_codes_stranger;
GRANT EXECUTE ON FUNCTION must(boolean, text), error_of(text), expect_code(text, text, text) TO PUBLIC;
SET ROLE regress_error_codes_stranger;
SELECT expect_code($$SELECT tviews.pg_tviews_refresh('a')$$, '42501', 'refresh not owner');
RESET ROLE;
DROP ROLE regress_error_codes_stranger;

-- Diagnostics name an unknown TVIEW instead of returning nothing.
SELECT expect_code($$SELECT * FROM tviews.pg_tviews_show_cascade_path('nope')$$, '42704',
                   'show_cascade_path unknown');
SELECT expect_code($$SELECT tviews.pg_tviews_mapping_query('nope', 'pg_class'::regclass)$$,
                   '42704', 'mapping_query unknown');
SELECT expect_code($$SELECT * FROM tviews.pg_tviews_read_set_queries('nope', 'pg_class'::regclass)$$,
                   '42704', 'read_set_queries unknown');

SELECT 'error codes: PASS' AS result;
