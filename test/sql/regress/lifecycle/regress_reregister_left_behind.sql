-- A TVIEW an update could not derive again keeps needs_reregister, and the
-- health check names it, with why when this release's checks know: a role
-- reading for it that may no longer read a table it reads, or row security on
-- such a table. Until it is fixed and re-registered, writes to its tables
-- refresh it without skipping or patching and fail where its definition no
-- longer holds (42501, 42P16); once fixed and re-registered, it is no longer
-- named.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/lifecycle/regress_reregister_left_behind.sql
--
-- expect-output: TVIEWs left to re-register are named: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse
DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
\ir ../../lib/assert_fresh.sql
\ir ../../lib/row_security.sql
SELECT regress_roles('regress_rlb');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;
CREATE FUNCTION reregister_health() RETURNS text LANGUAGE sql AS $$
    SELECT status || ' ' || message FROM tviews.pg_tviews_check_health()
    WHERE component = 'reregister' $$;
-- The SQLSTATE `stmt` fails with as `who`, or 'none'.
CREATE FUNCTION fails_with(who text, stmt text) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    BEGIN
        EXECUTE format('SET LOCAL ROLE %I', who);
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        RETURN SQLSTATE;
    END;
    RETURN 'none';
END $$;

-- owner_b owns the tables and writes them; owner_a owns the TVIEWs.
SET ROLE regress_rlb_owner_b;
CREATE TABLE public.tb_lost (pk_lost int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text);
CREATE TABLE public.tb_rls (pk_rls int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text);
CREATE TABLE public.tb_ok (pk_ok int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           title text);
INSERT INTO public.tb_lost (pk_lost, title) VALUES (1, 'l1');
INSERT INTO public.tb_rls (pk_rls, title) VALUES (1, 'r1');
INSERT INTO public.tb_ok (pk_ok, title) VALUES (1, 'o1');
GRANT SELECT, TRIGGER ON public.tb_lost, public.tb_rls, public.tb_ok TO regress_rlb_owner_a;
RESET ROLE;

SET ROLE regress_rlb_owner_a;
SELECT tviews.pg_tviews_create('public.tv_lost', $q$
    SELECT t.pk_lost, t.id, jsonb_build_object('title', t.title) AS data FROM public.tb_lost t $q$)
    IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_rls', $q$
    SELECT t.pk_rls, t.id, jsonb_build_object('title', t.title) AS data FROM public.tb_rls t $q$)
    IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_ok', $q$
    SELECT t.pk_ok, t.id, jsonb_build_object('title', t.title) AS data FROM public.tb_ok t $q$)
    IS NOT NULL AS created;
RESET ROLE;
SELECT must(reregister_health() LIKE 'OK %', 'health before: ' || reregister_health());

-- What an update from an earlier release leaves: the three plans are that
-- release's, tv_lost's owner may no longer read its table, and row security
-- applies to tv_rls's owner on its table (enabled where no check of this
-- release saw it: the catalog written directly).
UPDATE tviews.pg_tview_meta SET needs_reregister = true;
REVOKE SELECT ON public.tb_lost FROM regress_rlb_owner_a;
UPDATE pg_catalog.pg_class SET relrowsecurity = true WHERE oid = 'public.tb_rls'::regclass;

SELECT must(reregister_health() LIKE 'WARNING 3 TVIEWs left to re-register:%',
            'health does not count the TVIEWs left: ' || reregister_health());
SELECT must(reregister_health() LIKE '%tv_lost: regress_rlb_owner_a lacks SELECT on public.tb_lost%',
            'health does not say why tv_lost is left: ' || reregister_health());
SELECT must(reregister_health() LIKE
                '%tv_rls: row security applies to role regress_rlb_owner_a on table public.tb_rls%',
            'health does not say why tv_rls is left: ' || reregister_health());
SELECT must(reregister_health() LIKE '%tv_ok;%',
            'health does not name tv_ok: ' || reregister_health());

-- The writes: tv_lost's refresh reads as its owner and fails, tv_rls is refused,
-- tv_ok is refreshed in full and stays fresh.
-- The refresh runs at the end of the writing statement: read its SQLSTATE in psql.
\set ON_ERROR_STOP off
SET ROLE regress_rlb_owner_b;
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_lost SET title = 'l1b';
SELECT :'LAST_ERROR_SQLSTATE' AS lost_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_rls SET title = 'r1b';
SELECT :'LAST_ERROR_SQLSTATE' AS rls_state \gset
RESET ROLE;
\set ON_ERROR_STOP on
SELECT must(:'lost_state' = '42501', 'a write to tb_lost did not fail with 42501: ' || :'lost_state');
SELECT must(:'rls_state' = '42P16', 'a write to tb_rls did not fail with 42P16: ' || :'rls_state');
SELECT must(data->>'title' = 'l1', 'tv_lost changed') FROM public.tv_lost;
SET ROLE regress_rlb_owner_b;
UPDATE public.tb_ok SET title = 'o1b';
RESET ROLE;
SELECT assert_fresh('public.tv_ok', 'pk_ok', 'a write while left to re-register');

-- Re-registration refuses each until it is fixed.
SELECT must(fails_with('regress_rlb_owner_a', $s$ SELECT tviews.pg_tviews_reregister('public.tv_lost') $s$)
            = '42501', 'tv_lost re-registered without the privilege');
SELECT must(fails_with('regress_rlb_owner_a', $s$ SELECT tviews.pg_tviews_reregister('public.tv_rls') $s$)
            = '42P16', 'tv_rls re-registered under row security');

-- Fixed and re-registered: no longer named, and writes refresh them.
GRANT SELECT ON public.tb_lost TO regress_rlb_owner_a;
UPDATE pg_catalog.pg_class SET relrowsecurity = false WHERE oid = 'public.tb_rls'::regclass;
SET ROLE regress_rlb_owner_a;
SELECT tviews.pg_tviews_reregister('public.tv_lost') IS NOT NULL AS reregistered;
SELECT tviews.pg_tviews_reregister('public.tv_rls') IS NOT NULL AS reregistered;
SELECT tviews.pg_tviews_reregister('public.tv_ok') IS NOT NULL AS reregistered;
RESET ROLE;
SELECT must(reregister_health() LIKE 'OK %', 'health after the fixes: ' || reregister_health());
SET ROLE regress_rlb_owner_b;
UPDATE public.tb_lost SET title = 'l1c';
UPDATE public.tb_rls SET title = 'r1c';
RESET ROLE;
SELECT assert_fresh('public.tv_lost', 'pk_lost', 'a write after the fix');
SELECT assert_fresh('public.tv_rls', 'pk_rls', 'a write after the fix');

SELECT 'TVIEWs left to re-register are named: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_lost, public.tb_rls, public.tb_ok;
SELECT regress_drop_roles('regress_rlb');
