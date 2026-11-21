-- Regression test (E1-13): an ERROR raised inside a backend function pg_tviews calls
-- directly (datumIsEqual, from the row trigger's comparison of OLD and NEW) unwinds
-- through pg_tviews' frames: what they opened is closed, and the session's next
-- statement behaves as if the error never happened.
--
-- Called unguarded, the ERROR's longjmp skipped every Rust frame in between: the
-- trigger's catalog search_path stayed recorded as open, so pg_tviews took every
-- later statement of the backend for its own work and left a function's own writes
-- unrefreshed when it read them back.
--
-- The ERROR needs corrupt data, so the test-only GUC pg_tviews.test_ffi_fault makes
-- the comparison pass an invalid type length (test-hooks builds).
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/triggers/regress_ffi_error_unwinds.sql
-- expect-output: ffi_error_unwinds: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

SELECT current_setting('pg_tviews.test_ffi_fault', true) IS NULL AS no_hook \gset
\if :no_hook
\echo 'SKIP: pg_tviews is not built with test-hooks (no pg_tviews.test_ffi_fault)'
\quit
\endif

CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text NOT NULL);
INSERT INTO tb_post VALUES (1, DEFAULT, 'one');
CREATE TABLE tv_post AS
    SELECT pk_post, id, jsonb_build_object('title', title) AS data FROM tb_post;

-- A mutation function reading back its own write: pg_tviews refreshes tv_post before
-- the read, inside the statement.
CREATE FUNCTION save_post(t text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET title = t WHERE pk_post = 1;
    RETURN (SELECT data->>'title' FROM tv_post WHERE pk_post = 1);
END $$;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'ffi_error_unwinds FAIL: %', what; END IF; END $$;

SELECT must(save_post('two') = 'two', 'read back before the fault: ' || save_post('two'));

-- The comparison raises; the write is rolled back.
DO $$
BEGIN
    SET LOCAL pg_tviews.test_ffi_fault = on;
    UPDATE tb_post SET title = 'faulted' WHERE pk_post = 1;
    RAISE EXCEPTION 'ffi_error_unwinds FAIL: the injected fault raised nothing';
EXCEPTION WHEN internal_error THEN
    NULL;
END $$;
SELECT must(current_setting('pg_tviews.test_ffi_fault') = 'off', 'the fault is still on');
SELECT must((SELECT title FROM tb_post) = 'two', 'the faulted write was kept');

-- The session goes on as before: the function's own write is refreshed before it reads.
SELECT must(save_post('three') = 'three',
            'read back after the fault: ' || coalesce(save_post('four'), 'NULL'));
SELECT must((SELECT data->>'title' FROM tv_post) = (SELECT title FROM tb_post),
            'tv_post stale after the fault');

\echo 'ffi_error_unwinds: PASS'
