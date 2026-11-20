-- Row security on a TVIEW's own table (D6).
--
-- 1. ENABLE ROW LEVEL SECURITY with a reader policy: the owner is exempt, writes
--    refresh the TVIEW as usual, and the reader reads its rows only.
-- 2. FORCE ROW LEVEL SECURITY without a policy letting the owner read and write
--    every row: the ALTER runs (pg_restore orders it before CREATE POLICY), the
--    health check reports it, and every refresh is refused with 55000, even from
--    a session with row_security = off. An owner policy USING (true) WITH CHECK
--    (true) lifts the refusal; a restrictive policy applying to the owner does not.
-- 3. A columns-changing pg_tviews_create_or_replace() rebuilds the table and puts
--    its row security and policies back; a policy naming a dropped column fails it.
-- 4. The health check warns about a TVIEW others read without row security while a
--    table it reads filters its readers, and about an embedded table whose policies
--    differ from the TVIEW's.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_tv_row_security.sql
-- expect-output: row security on tv_*: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO ERROR;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
\ir ../../lib/assert_fresh.sql
\ir ../../lib/row_security.sql
SELECT regress_roles('regress_trs');

CREATE FUNCTION public.must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF;
END $$;

-- Raise unless `stmt`, run as `who`, fails with 55000 naming the owner's row security.
CREATE FUNCTION public.owner_filtered(who text, stmt text, what text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE state text; msg text;
BEGIN
    BEGIN
        EXECUTE format('SET LOCAL ROLE %I', who);
        EXECUTE stmt;
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, msg = MESSAGE_TEXT;
        IF state <> '55000' OR msg NOT LIKE '%row security applies to regress_trs_owner_a, '
                                            'the owner of public.tv_contract%' THEN
            RAISE EXCEPTION 'FAIL: % failed with % : %', what, state, msg;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: % was accepted', what;
END $$;
GRANT EXECUTE ON FUNCTION public.owner_filtered(text, text, text) TO PUBLIC;

CREATE FUNCTION public.tv_policies() RETURNS TABLE (status text, message text)
LANGUAGE sql AS $$
    SELECT status, message FROM tviews.pg_tviews_check_health() WHERE component = 'tv_policies'
$$;

SET ROLE regress_trs_owner_a;
CREATE TABLE public.tb_contract (pk_contract int PRIMARY KEY,
                                 id uuid NOT NULL DEFAULT gen_random_uuid(),
                                 tenant_id int NOT NULL, name text NOT NULL);
CREATE TABLE public.tb_site (pk_site int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_contract int NOT NULL REFERENCES public.tb_contract,
                             label text NOT NULL);
INSERT INTO public.tb_contract (pk_contract, tenant_id, name)
    VALUES (1, 10, 'c1'), (2, 10, 'c2'), (3, 20, 'c3');
INSERT INTO public.tb_site (pk_site, fk_contract, label) VALUES (1, 1, 's1'), (2, 3, 's2');
SELECT tviews.pg_tviews_create('public.tv_contract', $q$
    SELECT c.pk_contract, c.id, c.tenant_id,
           jsonb_build_object('name', c.name,
               'sites', (SELECT jsonb_agg(s.label ORDER BY s.pk_site) FROM public.tb_site s
                         WHERE s.fk_contract = c.pk_contract)) AS data
    FROM public.tb_contract c $q$) IS NOT NULL AS created;
GRANT SELECT ON public.tv_contract TO regress_trs_reader;
GRANT SELECT, UPDATE ON public.tb_contract TO regress_trs_writer;

-- 1. ENABLE, a reader policy: the owner is exempt.
ALTER TABLE public.tv_contract ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant ON public.tv_contract FOR SELECT TO regress_trs_reader
    USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::int);
UPDATE public.tb_contract SET name = 'c1b' WHERE pk_contract = 1;
INSERT INTO public.tb_site (pk_site, fk_contract, label) VALUES (3, 2, 's3');
RESET ROLE;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'ENABLE with a reader policy');
SET ROLE regress_trs_writer;
UPDATE public.tb_contract SET name = 'c3w' WHERE pk_contract = 3;
RESET ROLE;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'a writer''s write');
BEGIN;
SET LOCAL ROLE regress_trs_reader;
SET LOCAL app.tenant_id = '10';
SELECT must(count(*) = 2, 'the reader reads its tenant''s rows only')
FROM public.tv_contract;
COMMIT;
SELECT must(status = 'OK', format('tv_policies with ENABLE: %s %s', status, message))
FROM tv_policies();

-- 2. FORCE, with no policy for the owner: the ALTER runs, maintenance is refused.
SET ROLE regress_trs_owner_a;
ALTER TABLE public.tv_contract FORCE ROW LEVEL SECURITY;
RESET ROLE;
SELECT must(status = 'ERROR' AND message LIKE '%public.tv_contract (regress_trs_owner_a: no policy%',
            format('tv_policies under FORCE: %s %s', status, message))
FROM tv_policies();
SELECT owner_filtered('regress_trs_owner_a',
    $$UPDATE public.tb_contract SET name = 'c1c' WHERE pk_contract = 1$$, 'the owner''s write');
SELECT owner_filtered('regress_trs_writer',
    $$UPDATE public.tb_contract SET name = 'c3x' WHERE pk_contract = 3$$, 'a writer''s write');
SELECT owner_filtered('regress_trs_owner_a',
    $$INSERT INTO public.tb_site (pk_site, fk_contract, label) VALUES (4, 1, 's4')$$,
    'a child''s insert');
SELECT owner_filtered('regress_trs_owner_a',
    $$SELECT tviews.pg_tviews_refresh('public.tv_contract')$$, 'a full refresh');
SELECT owner_filtered('regress_trs_writer',
    $$SET LOCAL row_security = off; UPDATE public.tb_contract SET name = 'c3y' WHERE pk_contract = 3$$,
    'a write with row_security off');

-- A restrictive policy applying to the owner keeps the refusal.
SET ROLE regress_trs_owner_a;
CREATE POLICY tviews_owner ON public.tv_contract FOR ALL TO regress_trs_owner_a
    USING (true) WITH CHECK (true);
CREATE POLICY no_tenant_30 ON public.tv_contract AS RESTRICTIVE FOR ALL TO PUBLIC
    USING (tenant_id <> 30);
RESET ROLE;
SELECT owner_filtered('regress_trs_owner_a',
    $$UPDATE public.tb_contract SET name = 'c1c' WHERE pk_contract = 1$$,
    'a write under a restrictive policy');
SET ROLE regress_trs_owner_a;
DROP POLICY no_tenant_30 ON public.tv_contract;
-- The owner's policy lifts it: refreshes run under FORCE.
UPDATE public.tb_contract SET name = 'c1c' WHERE pk_contract = 1;
INSERT INTO public.tb_site (pk_site, fk_contract, label) VALUES (4, 1, 's4');
SELECT tviews.pg_tviews_refresh('public.tv_contract') IS NOT NULL AS refreshed;
RESET ROLE;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'FORCE with an owner policy');
BEGIN;
SET LOCAL ROLE regress_trs_writer;
SET LOCAL row_security = off;
UPDATE public.tb_contract SET name = 'c3z' WHERE pk_contract = 3;
COMMIT;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'a write with row_security off');
-- A writer's tenant setting does not reach the owner's frame (E1-01): the
-- maintenance of another tenant's row reads what the owner reads.
BEGIN;
SET LOCAL ROLE regress_trs_writer;
SET LOCAL app.tenant_id = '10';
UPDATE public.tb_contract SET name = 'c3t' WHERE pk_contract = 3;
COMMIT;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'a write under a writer''s tenant setting');
SELECT must(status = 'OK', format('tv_policies with an owner policy: %s %s', status, message))
FROM tv_policies();

-- 3. A columns-changing replace rebuilds the table and keeps its row security.
SET ROLE regress_trs_owner_a;
SELECT tviews.pg_tviews_create_or_replace('public.tv_contract', $q$
    SELECT c.pk_contract, c.id, c.tenant_id, c.pk_contract % 2 = 0 AS even,
           jsonb_build_object('name', c.name,
               'sites', (SELECT jsonb_agg(s.label ORDER BY s.pk_site) FROM public.tb_site s
                         WHERE s.fk_contract = c.pk_contract)) AS data
    FROM public.tb_contract c $q$) AS replaced;
RESET ROLE;
SELECT must(relrowsecurity AND relforcerowsecurity, 'the rebuild lost ENABLE or FORCE')
FROM pg_class WHERE oid = 'public.tv_contract'::regclass;
SELECT must(array_agg(polname ORDER BY polname) = '{tenant,tviews_owner}',
            format('the rebuild''s policies: %s', array_agg(polname ORDER BY polname)))
FROM pg_policy WHERE polrelid = 'public.tv_contract'::regclass;
SELECT must(pg_get_expr(polqual, polrelid) LIKE '%app.tenant_id%'
            AND polroles = ARRAY['regress_trs_reader'::regrole::oid] AND polcmd = 'r',
            'the rebuild changed the tenant policy')
FROM pg_policy WHERE polrelid = 'public.tv_contract'::regclass AND polname = 'tenant';
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'the rebuild');
BEGIN;
SET LOCAL ROLE regress_trs_reader;
SET LOCAL app.tenant_id = '20';
SELECT must(count(*) = 1, 'the reader after the rebuild') FROM public.tv_contract;
COMMIT;

-- A policy naming a column the new definition drops fails the change, naming it.
SET ROLE regress_trs_owner_a;
CREATE POLICY even_only ON public.tv_contract FOR SELECT TO regress_trs_stranger USING (even);
DO $$
DECLARE msg text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_create_or_replace('public.tv_contract', $q$
            SELECT c.pk_contract, c.id, c.tenant_id,
                   jsonb_build_object('name', c.name) AS data
            FROM public.tb_contract c $q$);
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS msg = MESSAGE_TEXT;
        IF msg NOT LIKE '%policy even_only on tv_contract cannot be re-created%' THEN
            RAISE EXCEPTION 'FAIL: the replace failed with another error: %', msg;
        END IF;
        RETURN;
    END;
    RAISE EXCEPTION 'FAIL: a replace dropping a policy''s column was accepted';
END $$;
DROP POLICY even_only ON public.tv_contract;
RESET ROLE;
SELECT owner_fresh('public.tv_contract', 'pk_contract', 'the refused replace');

-- 4. Warnings. An embedded table filtering its readers by other policies.
SET ROLE regress_trs_owner_a;
ALTER TABLE public.tb_site ENABLE ROW LEVEL SECURITY;
CREATE POLICY site_reader ON public.tb_site FOR SELECT TO regress_trs_reader
    USING (label <> 's2');
RESET ROLE;
SELECT must(status = 'WARNING' AND message LIKE '%public.tv_contract embeds rows of public.tb_site%',
            format('tv_policies, a narrower child: %s %s', status, message))
FROM tv_policies();
-- A TVIEW others read without row security, over a table filtering its readers.
SET ROLE regress_trs_owner_a;
ALTER TABLE public.tv_contract DISABLE ROW LEVEL SECURITY;
RESET ROLE;
SELECT must(status = 'WARNING'
            AND message LIKE '%public.tv_contract has no row security while public.tb_site, which it reads%',
            format('tv_policies, no row security: %s %s', status, message))
FROM tv_policies();
SET ROLE regress_trs_owner_a;
REVOKE SELECT ON public.tv_contract FROM regress_trs_reader;
DROP POLICY site_reader ON public.tb_site;
ALTER TABLE public.tb_site DISABLE ROW LEVEL SECURITY;
RESET ROLE;
SELECT must(status = 'OK', format('tv_policies at the end: %s %s', status, message))
FROM tv_policies();

SELECT 'row security on tv_*: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_site, public.tb_contract;
SELECT regress_drop_roles('regress_trs');
