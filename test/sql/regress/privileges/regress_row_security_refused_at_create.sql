-- A refresh reads every row of the tables its TVIEW reads (row security for the
-- TVIEW's owner, before CREATE).
--
-- A TVIEW is refused while row security applies on a table it reads to its
-- owner: a table it does not own with row security enabled, or one it owns that
-- is forced. The error names the table and the role. A re-register of a TVIEW
-- created before the table was set so is refused the same way.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_refused_at_create.sql
-- expect-output: row security for the owner, before CREATE: PASS

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
SELECT regress_roles('regress_rsc');

-- owner_b owns tb_rec and tb_note; owner_a owns tb_own.
SET ROLE regress_rsc_owner_b;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL, active boolean NOT NULL DEFAULT true);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_rec int NOT NULL REFERENCES public.tb_rec, body text NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2');
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (1, 1, 'n1'), (2, 2, 'n2');
GRANT SELECT, TRIGGER ON public.tb_rec, public.tb_note TO regress_rsc_owner_a;
ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY;
CREATE POLICY note_all ON public.tb_note FOR SELECT TO regress_rsc_owner_a USING (true);
RESET ROLE;
SET ROLE regress_rsc_owner_a;
CREATE TABLE public.tb_own (pk_own int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
INSERT INTO public.tb_own (pk_own, title) VALUES (1, 'o1');
ALTER TABLE public.tb_own ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tb_own FORCE ROW LEVEL SECURITY;
CREATE POLICY own_all ON public.tb_own FOR SELECT USING (true);
RESET ROLE;

-- A child collection read from a table under row security for the owner.
SELECT refused('regress_rsc_owner_a', $s$
    SELECT tviews.pg_tviews_create('public.tv_recn', $q$
        SELECT r.pk_rec AS pk_recn, r.id, jsonb_build_object('title', r.title,
            'notes', (SELECT jsonb_agg(n.body ORDER BY n.pk_note) FROM public.tb_note n
                      WHERE n.fk_rec = r.pk_rec)) AS data
        FROM public.tb_rec r $q$) $s$, 'CREATE TVIEW (a table it does not own)',
    'regress_rsc_owner_a', 'public.tb_note');

-- Its own table, forced.
SELECT refused('regress_rsc_owner_a', $s$
    SELECT tviews.pg_tviews_create('public.tv_own', $q$
        SELECT o.pk_own, o.id, jsonb_build_object('title', o.title) AS data
        FROM public.tb_own o $q$) $s$, 'CREATE TVIEW (its own table, forced)',
    'regress_rsc_owner_a', 'public.tb_own');

-- Re-register: a TVIEW created first, the table then set so behind the
-- extension's back (no statement the extension follows does it).
SET ROLE regress_rsc_owner_a;
SELECT tviews.pg_tviews_create('public.tv_rec', $q$
    SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
RESET ROLE;
UPDATE pg_catalog.pg_class SET relrowsecurity = true WHERE oid = 'public.tb_rec'::regclass;
SELECT refused('regress_rsc_owner_a', $s$ SELECT tviews.pg_tviews_reregister('rec') $s$,
               're-register', 'regress_rsc_owner_a', 'public.tb_rec');
UPDATE pg_catalog.pg_class SET relrowsecurity = false WHERE oid = 'public.tb_rec'::regclass;

SELECT 'row security for the owner, before CREATE: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note, public.tb_rec, public.tb_own;
SELECT regress_drop_roles('regress_rsc');
