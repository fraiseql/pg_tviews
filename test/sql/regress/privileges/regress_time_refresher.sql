-- Regression test (E1-10): a time refresh that cannot refresh every time-dependent
-- TVIEW says so, and an operator can refresh them all.
--   1. pg_tviews_refresh_time_dependent() and CALL pg_tviews_time_boundary(), run by
--      the owner of one TVIEW, refresh it and name in a WARNING the time-dependent
--      TVIEW they skipped, whose boundary stays unset;
--   2. the operator forms are not PUBLIC's: 42501;
--   3. granted them, a role owning no TVIEW refreshes every time-dependent TVIEW,
--      each as its owner, one transaction each, and both boundaries are recorded.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_time_refresher.sql
-- expect-once: WARNING:  time-dependent TVIEWs not refreshed, owned by other roles: public.tv_theirs
-- expect-output: time_refresher: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
DROP ROLE IF EXISTS regress_ttr_mine;
DROP ROLE IF EXISTS regress_ttr_theirs;
DROP ROLE IF EXISTS regress_ttr_cron;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
CREATE ROLE regress_ttr_mine;
CREATE ROLE regress_ttr_theirs;
CREATE ROLE regress_ttr_cron;
GRANT CREATE, USAGE ON SCHEMA public TO regress_ttr_mine, regress_ttr_theirs;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'time_refresher FAIL: %', what; END IF; END $$;
CREATE FUNCTION boundary(e text) RETURNS timestamptz LANGUAGE sql AS $$
    SELECT time_boundary FROM tviews.registry WHERE entity = e $$;

CREATE TABLE tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      ends_at timestamptz NOT NULL);
INSERT INTO tb_item VALUES (1, DEFAULT, now() + interval '1 day');
GRANT SELECT, TRIGGER ON tb_item TO regress_ttr_mine, regress_ttr_theirs;

SET ROLE regress_ttr_mine;
SELECT tviews.pg_tviews_create('tv_mine', $$
    SELECT pk_item AS pk_mine, id, jsonb_build_object('current', now() < ends_at) AS data
    FROM public.tb_item $$, '{"time_refresh": "external"}');
SET ROLE regress_ttr_theirs;
SELECT tviews.pg_tviews_create('tv_theirs', $$
    SELECT pk_item AS pk_theirs, id, jsonb_build_object('current', now() < ends_at) AS data
    FROM public.tb_item $$, '{"time_refresh": "external"}');
RESET ROLE;

-- 1. The function's WARNING is checked once (expect-once); the procedure's is checked
--    in a separate session, so the two do not count twice.
SET ROLE regress_ttr_mine;
SELECT * FROM tviews.pg_tviews_refresh_time_dependent();
RESET ROLE;
SELECT must(boundary('mine') IS NOT NULL AND boundary('theirs') IS NULL, 'the owner''s refresh');
SELECT must((SELECT count(*) FROM tviews.pg_tview_time_boundary) = 1, 'boundaries recorded');
\setenv PGDATABASE :DBNAME
\! psql -X -q -c 'SET ROLE regress_ttr_mine' -c 'CALL tviews.pg_tviews_time_boundary()' 2>&1 | sed 's/^WARNING:/PROCEDURE SAID:/'
-- expect-output: PROCEDURE SAID:  time-dependent TVIEWs not refreshed, owned by other roles: public.tv_theirs
SELECT must(boundary('theirs') IS NULL, 'the procedure refreshed another owner''s TVIEW');

-- 2.
SET ROLE regress_ttr_cron;
\set ON_ERROR_STOP off
CALL tviews.pg_tviews_time_boundary_all();
-- expect-output: permission denied for procedure pg_tviews_time_boundary_all
SELECT tviews.pg_tviews_refresh_time_dependent_all();
-- expect-output: permission denied for function pg_tviews_refresh_time_dependent_all
\set ON_ERROR_STOP on
RESET ROLE;

-- 3.
GRANT EXECUTE ON PROCEDURE tviews.pg_tviews_time_boundary_all() TO regress_ttr_cron;
GRANT EXECUTE ON FUNCTION tviews.pg_tviews_refresh_time_dependent_all(text) TO regress_ttr_cron;
SELECT boundary('mine') AS mine_before \gset
SET ROLE regress_ttr_cron;
CALL tviews.pg_tviews_time_boundary_all();
RESET ROLE;
SELECT must(boundary('mine') > :'mine_before'::timestamptz AND boundary('theirs') IS NOT NULL,
            'the operator''s refresh');

DROP TABLE tv_mine, tv_theirs;
DROP EXTENSION pg_tviews CASCADE;
REVOKE CREATE, USAGE ON SCHEMA public FROM regress_ttr_mine, regress_ttr_theirs;
DROP TABLE tb_item;
DROP ROLE regress_ttr_mine;
DROP ROLE regress_ttr_theirs;
DROP ROLE regress_ttr_cron;
\echo 'time_refresher: PASS'
