-- Issue #278: test builds can log what a refresh upsert failing with 21000
-- conflicted with.
--
-- `ON CONFLICT DO UPDATE command cannot affect row a second time` was seen once
-- under the concurrency fuzzer and never reproduced. With pg_tviews.test_diag_21000
-- the upsert runs in a subtransaction; on 21000 a WARNING DIAG21000 names the keys,
-- the source's duplicate keys and the conflicting rows, and the error is raised
-- again with its SQLSTATE. pg_tviews.test_duplicate_source forces the error here:
-- every refresh source returns each row twice. Both exist in test-hooks builds only.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/refresh/regress_diag_21000.sql
-- expect-output: DIAG21000 tv=public.tv_post
-- expect-output: source_duplicates=[3 x2]
-- expect-once: DIAG21000
-- expect-output: diag 21000: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

SELECT current_setting('pg_tviews.test_diag_21000', true) IS NOT NULL AS hooks \gset
\if :hooks
\else
\echo 'SKIP: pg_tviews.test_diag_21000 needs a test-hooks build'
\quit
\endif

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'diag 21000 FAIL: %', what; END IF; END $$;

CREATE TABLE tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      title text NOT NULL);
INSERT INTO tb_post (pk_post, title) VALUES (1, 'a');
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, jsonb_build_object('title', p.title) AS data FROM tb_post p $$);

-- The refresh runs at the end of the writing statement: each write is its own
-- statement, its SQLSTATE read from psql.
\set ON_ERROR_STOP off
-- Without the diagnostic: the forced duplicate fails 21000, logging nothing.
SET pg_tviews.test_duplicate_source = on;
INSERT INTO tb_post (pk_post, title) VALUES (2, 'b');
SELECT must(:'LAST_ERROR_SQLSTATE' = '21000', 'a doubled source failed ' || :'LAST_ERROR_SQLSTATE');
\set LAST_ERROR_SQLSTATE 00000
-- With it: the same 21000, after one DIAG21000 warning.
SET pg_tviews.test_diag_21000 = on;
INSERT INTO tb_post (pk_post, title) VALUES (3, 'c');
SELECT must(:'LAST_ERROR_SQLSTATE' = '21000', 'with the diagnostic, failed ' || :'LAST_ERROR_SQLSTATE');
\set ON_ERROR_STOP on

-- A refresh that succeeds commits its upsert through the subtransaction.
RESET pg_tviews.test_duplicate_source;
INSERT INTO tb_post (pk_post, title) VALUES (4, 'd');
SELECT must((SELECT data->>'title' FROM tv_post WHERE pk_post = 4) = 'd',
            'an upsert run under the diagnostic was lost');
RESET pg_tviews.test_diag_21000;

DROP EXTENSION pg_tviews CASCADE;
\echo 'diag 21000: PASS'
