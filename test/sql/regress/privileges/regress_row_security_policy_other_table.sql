-- A refresh reads every row of the tables its TVIEW reads (whatever a policy
-- reads).
--
-- A policy that reads another table is no special case: while row security
-- applies on the table to a role the TVIEW reads it as, the TVIEW is refused,
-- at CREATE and by ENABLE ROW LEVEL SECURITY afterwards.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_policy_other_table.sql
-- expect-output: row security, a policy reading another table: PASS

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
SELECT regress_roles('regress_rso');

-- owner_b owns the tables and writes them; owner_a owns the TVIEW.
SET ROLE regress_rso_owner_b;
CREATE TABLE public.tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             title text NOT NULL);
CREATE TABLE public.tb_acl (pk_acl int PRIMARY KEY, fk_item int NOT NULL, visible boolean NOT NULL);
INSERT INTO public.tb_item (pk_item, title) VALUES (1, 'i1'), (2, 'i2');
INSERT INTO public.tb_acl VALUES (1, 1, true), (2, 2, true);
GRANT SELECT, TRIGGER ON public.tb_item TO regress_rso_owner_a;
GRANT SELECT ON public.tb_acl TO regress_rso_owner_a;
CREATE POLICY p_acl ON public.tb_item FOR SELECT TO regress_rso_owner_a
    USING (EXISTS (SELECT FROM public.tb_acl a WHERE a.fk_item = pk_item AND a.visible));
ALTER TABLE public.tb_item ENABLE ROW LEVEL SECURITY;
RESET ROLE;

-- At CREATE.
SELECT refused('regress_rso_owner_a', $s$
    SELECT tviews.pg_tviews_create('public.tv_item', $q$
        SELECT i.pk_item, i.id, jsonb_build_object('title', i.title) AS data
        FROM public.tb_item i $q$) $s$, 'CREATE TVIEW', 'regress_rso_owner_a', 'public.tb_item');

-- Afterwards.
SET ROLE regress_rso_owner_b;
ALTER TABLE public.tb_item DISABLE ROW LEVEL SECURITY;
RESET ROLE;
SET ROLE regress_rso_owner_a;
SELECT tviews.pg_tviews_create('public.tv_item', $q$
    SELECT i.pk_item, i.id, jsonb_build_object('title', i.title) AS data
    FROM public.tb_item i $q$) IS NOT NULL AS created;
RESET ROLE;
SELECT refused('regress_rso_owner_b', 'ALTER TABLE public.tb_item ENABLE ROW LEVEL SECURITY',
               'ENABLE ROW LEVEL SECURITY', 'regress_rso_owner_a', 'public.tb_item');

SELECT 'row security, a policy reading another table: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_acl, public.tb_item;
SELECT regress_drop_roles('regress_rso');
