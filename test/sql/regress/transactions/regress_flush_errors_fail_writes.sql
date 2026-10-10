-- A refresh that fails fails the write that queued it, as the refresh runbook
-- says: the statement-level flush trigger used to turn an error the flush
-- returned (rather than one PostgreSQL raised) into a WARNING and let the write
-- commit, leaving the TVIEWs stale. Here the flush returns "propagation depth
-- exceeded": the write fails, and neither the table nor the TVIEWs change.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/transactions/regress_flush_errors_fail_writes.sql
--
-- expect-output: flush errors fail writes: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/assert_fresh.sql

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'flush errors FAIL: %', what; END IF; END $$;
CREATE FUNCTION error_of(stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN EXECUTE stmt; RETURN 'committed';
EXCEPTION WHEN OTHERS THEN RETURN SQLERRM; END $$;

CREATE TABLE tb_user (pk_user bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), name text);
CREATE TABLE tb_post (pk_post bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_user bigint REFERENCES tb_user, title text);
INSERT INTO tb_user (pk_user, name) VALUES (1, 'u');
INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p');
SELECT tviews.pg_tviews_create('tv_user', 'SELECT pk_user, id, jsonb_build_object(''name'', name) AS data FROM tb_user');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user $$);

-- A write to tb_user refreshes tv_user, then tv_post: two passes.
SET pg_tviews.max_propagation_depth = 1;
SELECT error_of('UPDATE tb_user SET name = ''u2'' WHERE pk_user = 1') AS outcome \gset
SELECT must(:'outcome' LIKE '%depth%', 'the write was not refused: ' || :'outcome');
RESET pg_tviews.max_propagation_depth;
SELECT must((SELECT name FROM tb_user) = 'u', 'the refused write changed tb_user');
SELECT assert_fresh('tv_user', 'pk_user', 'after the refused write');
SELECT assert_fresh('tv_post', 'pk_post', 'after the refused write');

-- With the default limit the same write goes through and refreshes both.
UPDATE tb_user SET name = 'u2' WHERE pk_user = 1;
SELECT must((SELECT data -> 'author' ->> 'name' FROM tv_post) = 'u2', 'tv_post not refreshed');

SELECT 'flush errors fail writes: PASS' AS result;
