-- No column skip while row security applies to the TVIEW's owner (a policy created after the create).
--
-- While row security applies to the TVIEW's owner on a table it reads, a write
-- to that table refreshes the rows it reaches whatever columns it changed, on
-- the row path and on the statement path; here a policy is added after the TVIEW was created, on tables whose row security was already enabled.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_owner_rls_policy_after_create.sql
-- expect-output: owner row security, a policy created after the create: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
\ir ../../lib/assert_fresh.sql
SELECT regress_roles('regress_orp');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;
CREATE FUNCTION stopgap(reason text) RETURNS bigint LANGUAGE sql AS $$
    SELECT CASE WHEN tviews.pg_tviews_show_queue() ? 'patch_stats'
                THEN coalesce((tviews.pg_tviews_show_queue()->'patch_stats'->>('stopgap:' || reason))::bigint, 0)
           END $$;
-- tv_<e> against its backing view read as the TVIEW's owner.
CREATE FUNCTION owner_fresh(tv regclass, key text, label text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE diff text;
BEGIN
    EXECUTE format('SET LOCAL ROLE %I', (SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid = tv));
    diff := fresh_diff(tv, key);
    RESET ROLE;
    IF diff IS NOT NULL THEN
        RAISE EXCEPTION 'STALE after %: %', label, diff;
    END IF;
END $$;
GRANT EXECUTE ON FUNCTION fresh_diff(regclass, text) TO PUBLIC;

-- owner_b owns the tables and writes them; owner_a owns the TVIEWs.
SET ROLE regress_orp_owner_b;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL, active boolean NOT NULL DEFAULT true);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_rec int NOT NULL REFERENCES public.tb_rec, body text NOT NULL,
                             active boolean NOT NULL DEFAULT true);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2'), (3, 'r3');
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (1, 1, 'n1'), (2, 1, 'n2'), (3, 2, 'n3');
GRANT SELECT, TRIGGER ON public.tb_rec, public.tb_note TO regress_orp_owner_a;
ALTER TABLE public.tb_rec ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY;
CREATE POLICY rec_all ON public.tb_rec FOR SELECT TO regress_orp_owner_a USING (true);
CREATE POLICY note_all ON public.tb_note FOR SELECT TO regress_orp_owner_a USING (true);
RESET ROLE;

SET ROLE regress_orp_owner_a;
-- The row path: the table holding the key.
SELECT tviews.pg_tviews_create('public.tv_rec', $q$
    SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
-- The statement path: a child collection.
SELECT tviews.pg_tviews_create('public.tv_recn', $q$
    SELECT r.pk_rec AS pk_recn, r.id, jsonb_build_object('title', r.title,
        'notes', (SELECT jsonb_agg(n.body ORDER BY n.pk_note) FROM public.tb_note n
                  WHERE n.fk_rec = r.pk_rec)) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
RESET ROLE;
SET ROLE regress_orp_owner_b;
CREATE POLICY rec_active ON public.tb_rec AS RESTRICTIVE FOR SELECT TO regress_orp_owner_a USING (active);
CREATE POLICY note_active ON public.tb_note AS RESTRICTIVE FOR SELECT TO regress_orp_owner_a USING (active);
RESET ROLE;

SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the setup');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the setup');

-- A write, then its reverse.
SET ROLE regress_orp_owner_b;
UPDATE public.tb_rec SET active = false WHERE pk_rec = 1;
UPDATE public.tb_note SET active = false WHERE pk_note = 2;
RESET ROLE;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the UPDATE (tb_rec)');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the UPDATE (tb_note)');

SET ROLE regress_orp_owner_b;
UPDATE public.tb_rec SET active = true WHERE pk_rec = 1;
UPDATE public.tb_note SET active = true WHERE pk_note = 2;
RESET ROLE;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the reverse UPDATE (tb_rec)');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the reverse UPDATE (tb_note)');

-- In one transaction, with the flush at its end.
BEGIN;
SET LOCAL ROLE regress_orp_owner_b;
UPDATE public.tb_rec SET active = false WHERE pk_rec = 3;
UPDATE public.tb_note SET active = false WHERE pk_note = 3;
COMMIT;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'a transaction of such UPDATEs (tb_rec)');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'a transaction of such UPDATEs (tb_note)');

SELECT must(stopgap('rls') IS NULL OR stopgap('rls') > 0, 'no stopgap:rls counted');

SELECT 'owner row security, a policy created after the create: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note, public.tb_rec;
DROP FUNCTION owner_fresh(regclass, text, text);
DROP FUNCTION must(boolean, text);
SELECT regress_drop_roles('regress_orp');
