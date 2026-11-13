-- A refresh reads every row of the tables its TVIEW reads (every table and
-- every role, not only the plan's).
--
-- The tables are those the chain's views name as they are now, beside the
-- plan's: a view replaced to read another table is probed on it. The roles
-- include a view owner given the view later, and the owner of a declared
-- SECURITY DEFINER function. A parent TVIEW reads its child's tv_* table like
-- any other. A statement that leaves every TVIEW as it was passes, even while
-- one TVIEW is refused already.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_beyond_plan.sql
-- expect-output: row security, every table and every role: PASS

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
SELECT regress_roles('regress_rsb');

-- owner_a owns the tables and the TVIEWs; tb_lbl and tb_extra have row security
-- enabled (owner_a is exempt). owner_b owns v_rec and a SECURITY DEFINER function.
SET ROLE regress_rsb_owner_a;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
CREATE TABLE public.tb_lbl (pk_lbl int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
CREATE TABLE public.tb_extra (fk_rec int NOT NULL, label text NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2');
INSERT INTO public.tb_lbl (pk_lbl, title) VALUES (1, 'l1');
INSERT INTO public.tb_extra VALUES (1, 'x1'), (2, 'x2');
GRANT SELECT ON public.tb_rec, public.tb_lbl, public.tb_extra TO regress_rsb_owner_b;
ALTER TABLE public.tb_lbl ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tb_extra ENABLE ROW LEVEL SECURITY;
CREATE POLICY extra_b ON public.tb_extra FOR SELECT TO regress_rsb_owner_b USING (true);
CREATE VIEW public.v_lbl AS SELECT pk_lbl, id, title FROM public.tb_lbl;
RESET ROLE;
SET ROLE regress_rsb_owner_b;
CREATE VIEW public.v_rec AS SELECT pk_rec, id, title FROM public.tb_rec;
GRANT SELECT ON public.v_rec TO regress_rsb_owner_a;
CREATE FUNCTION public.extra_label(k int) RETURNS text LANGUAGE sql STABLE SECURITY DEFINER
    AS $f$ SELECT string_agg(label, ',' ORDER BY label) FROM public.tb_extra WHERE fk_rec = k $f$;
GRANT EXECUTE ON FUNCTION public.extra_label(int) TO regress_rsb_owner_a;
RESET ROLE;

SET ROLE regress_rsb_owner_a;
SELECT tviews.pg_tviews_create('public.tv_rec', $q$
    SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title) AS data
    FROM public.v_rec r $q$) IS NOT NULL AS created;
SELECT tviews.pg_tviews_create('public.tv_lbl', $q$
    SELECT l.pk_lbl, l.id, jsonb_build_object('title', l.title) AS data
    FROM public.v_lbl l $q$) IS NOT NULL AS created;
RESET ROLE;

-- A view of the chain replaced to read another table.
SELECT refused('regress_rsb_owner_b', $s$
    CREATE OR REPLACE VIEW public.v_rec AS
    SELECT r.pk_rec, r.id, r.title || coalesce((SELECT string_agg(e.label, ',') FROM public.tb_extra e
                                               WHERE e.fk_rec = r.pk_rec), '') AS title
    FROM public.tb_rec r $s$, 'CREATE OR REPLACE VIEW', 'regress_rsb_owner_b', 'public.tb_extra');

-- A view of the chain given to another role.
SELECT refused(NULL, 'ALTER VIEW public.v_lbl OWNER TO regress_rsb_owner_b',
               'ALTER VIEW OWNER TO', 'regress_rsb_owner_b', 'public.tb_lbl');

-- A declared SECURITY DEFINER function reads as its owner.
SELECT refused('regress_rsb_owner_a', $s$
    SELECT tviews.pg_tviews_create_or_replace('public.tv_fn', $q$
        SELECT r.pk_rec AS pk_fn, r.id, jsonb_build_object('extra', public.extra_label(r.pk_rec)) AS data
        FROM public.tb_rec r $q$,
        options => '{"function_reads": {"public.extra_label(integer)": ["public.tb_extra"]}}') $s$,
    'CREATE TVIEW (a SECURITY DEFINER function)', 'regress_rsb_owner_b', 'public.tb_extra');

-- A parent reading its child's tv_* table: forced on the child.
SET ROLE regress_rsb_owner_a;
SELECT tviews.pg_tviews_create('public.tv_lblp', $q$
    SELECT l.pk_lbl AS pk_lblp, l.id, jsonb_build_object('child', c.data) AS data
    FROM public.tb_lbl l JOIN public.tv_lbl c ON c.pk_lbl = l.pk_lbl $q$) IS NOT NULL AS created;
ALTER TABLE public.tv_lbl ENABLE ROW LEVEL SECURITY;
RESET ROLE;
SELECT refused('regress_rsb_owner_a', 'ALTER TABLE public.tv_lbl FORCE ROW LEVEL SECURITY',
               'FORCE on a child tv_* table', 'regress_rsb_owner_a', 'public.tv_lbl');

-- tv_rec refused already (the catalog written directly): statements that change
-- nothing for any TVIEW pass.
UPDATE pg_catalog.pg_class SET relrowsecurity = true WHERE oid = 'public.tb_rec'::regclass;
GRANT regress_rsb_reader TO regress_rsb_writer;
ALTER ROLE regress_rsb_writer CONNECTION LIMIT 5;
SET ROLE regress_rsb_owner_b;
CREATE OR REPLACE VIEW public.v_rec AS SELECT pk_rec, id, title FROM public.tb_rec;
RESET ROLE;
SELECT refused('regress_rsb_owner_a', $s$ UPDATE public.tb_rec SET title = 'r1b' WHERE pk_rec = 1 $s$,
               'a write', 'regress_rsb_owner_b', 'public.tb_rec');
UPDATE pg_catalog.pg_class SET relrowsecurity = false WHERE oid = 'public.tb_rec'::regclass;

-- Exempt: writes refresh, and the TVIEWs are fresh.
SET ROLE regress_rsb_owner_a;
UPDATE public.tb_rec SET title = 'r1c' WHERE pk_rec = 1;
UPDATE public.tb_lbl SET title = 'l1c' WHERE pk_lbl = 1;
RESET ROLE;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the writes');
SELECT owner_fresh('public.tv_lbl', 'pk_lbl', 'the writes');
SELECT owner_fresh('public.tv_lblp', 'pk_lblp', 'the writes');

SELECT 'row security, every table and every role: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP VIEW public.v_rec, public.v_lbl;
DROP FUNCTION public.extra_label(int);
DROP TABLE public.tb_extra, public.tb_lbl, public.tb_rec;
SELECT regress_drop_roles('regress_rsb');
