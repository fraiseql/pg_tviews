-- A refresh reads every row of the tables its TVIEW reads (row security for the
-- owner of a view of the chain).
--
-- A definition reads a table through a view that is not security_invoker as
-- that view's owner. A TVIEW is refused while row security applies on the table
-- to that role, at CREATE and by ENABLE ROW LEVEL SECURITY afterwards, though
-- none applies to the TVIEW's owner.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_view_owner.sql
-- expect-output: row security for a view owner of the chain: PASS

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
SELECT regress_roles('regress_rsv');

-- owner_a owns the tables and the TVIEWs: no row security applies to it.
-- owner_b owns the views the definitions read the tables through.
SET ROLE regress_rsv_owner_a;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL, active boolean NOT NULL DEFAULT true);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2');
INSERT INTO public.tb_note (pk_note, title) VALUES (1, 'n1');
GRANT SELECT ON public.tb_rec, public.tb_note TO regress_rsv_owner_b;
ALTER TABLE public.tb_rec ENABLE ROW LEVEL SECURITY;
CREATE POLICY rec_active ON public.tb_rec FOR SELECT TO regress_rsv_owner_b USING (active);
RESET ROLE;

SET ROLE regress_rsv_owner_b;
CREATE VIEW public.v_rec AS SELECT pk_rec, id, title FROM public.tb_rec;
CREATE VIEW public.v_note AS SELECT pk_note, id, title FROM public.tb_note;
GRANT SELECT ON public.v_rec, public.v_note TO regress_rsv_owner_a;
RESET ROLE;

-- At CREATE.
SELECT refused('regress_rsv_owner_a', $s$
    SELECT tviews.pg_tviews_create('public.tv_rec', $q$
        SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title) AS data
        FROM public.v_rec r $q$) $s$, 'CREATE TVIEW', 'regress_rsv_owner_b', 'public.tb_rec');

-- Afterwards.
SET ROLE regress_rsv_owner_a;
SELECT tviews.pg_tviews_create('public.tv_note', $q$
    SELECT n.pk_note, n.id, jsonb_build_object('title', n.title) AS data
    FROM public.v_note n $q$) IS NOT NULL AS created;
RESET ROLE;
SELECT refused('regress_rsv_owner_a', 'ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY',
               'ENABLE ROW LEVEL SECURITY', 'regress_rsv_owner_b', 'public.tb_note');

SELECT 'row security for a view owner of the chain: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP VIEW public.v_note, public.v_rec;
DROP TABLE public.tb_note, public.tb_rec;
SELECT regress_drop_roles('regress_rsv');
