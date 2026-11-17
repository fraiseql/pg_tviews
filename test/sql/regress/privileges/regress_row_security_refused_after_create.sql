-- A refresh reads every row of the tables its TVIEW reads (statements after
-- CREATE).
--
-- Every statement that would make row security apply on a table a TVIEW reads
-- to a role it reads that table as is refused: ENABLE ROW LEVEL SECURITY, FORCE
-- ROW LEVEL SECURITY, an owner change of the table, BYPASSRLS taken away, a
-- membership revoked. Statements that leave the TVIEW's roles exempt pass.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_refused_after_create.sql
-- expect-output: row security, statements after CREATE: PASS

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
SELECT regress_roles('regress_rsa');

-- owner_a owns tb_rec and the TVIEWs; owner_b owns tb_note.
SET ROLE regress_rsa_owner_a;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2');
GRANT SELECT ON public.tb_rec TO regress_rsa_owner_b;
RESET ROLE;
SET ROLE regress_rsa_owner_b;
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_rec int NOT NULL, body text NOT NULL);
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (1, 1, 'n1'), (2, 2, 'n2');
GRANT SELECT, TRIGGER ON public.tb_note TO regress_rsa_owner_a;
RESET ROLE;

SET ROLE regress_rsa_owner_a;
SELECT tviews.pg_tviews_create('public.tv_recn', $q$
    SELECT r.pk_rec AS pk_recn, r.id, jsonb_build_object('title', r.title,
        'notes', (SELECT jsonb_agg(n.body ORDER BY n.pk_note) FROM public.tb_note n
                  WHERE n.fk_rec = r.pk_rec)) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
RESET ROLE;

-- ENABLE on a table the owner does not own.
SELECT refused('regress_rsa_owner_b', 'ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY',
               'ENABLE ROW LEVEL SECURITY', 'regress_rsa_owner_a', 'public.tb_note');

-- ENABLE on its own table passes (the owner is exempt); FORCE does not.
SET ROLE regress_rsa_owner_a;
ALTER TABLE public.tb_rec ENABLE ROW LEVEL SECURITY;
CREATE POLICY rec_none ON public.tb_rec FOR SELECT TO regress_rsa_reader USING (false);
RESET ROLE;
SELECT refused('regress_rsa_owner_a', 'ALTER TABLE public.tb_rec FORCE ROW LEVEL SECURITY',
               'FORCE ROW LEVEL SECURITY', 'regress_rsa_owner_a', 'public.tb_rec');

-- The table given to another role, its row security enabled.
SELECT refused(NULL, 'ALTER TABLE public.tb_rec OWNER TO regress_rsa_owner_b',
               'an owner change of the table', 'regress_rsa_owner_a', 'public.tb_rec');

-- Exempt through membership in the table's owner: the membership revoked.
GRANT regress_rsa_owner_b TO regress_rsa_owner_a;
SET ROLE regress_rsa_owner_b;
ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY;
RESET ROLE;
SELECT refused(NULL, 'REVOKE regress_rsa_owner_b FROM regress_rsa_owner_a',
               'a membership revoked', 'regress_rsa_owner_a', 'public.tb_note');

-- Exempt through BYPASSRLS: taken away.
ALTER ROLE regress_rsa_owner_a BYPASSRLS;
REVOKE regress_rsa_owner_b FROM regress_rsa_owner_a;
SELECT refused(NULL, 'ALTER ROLE regress_rsa_owner_a NOBYPASSRLS',
               'NOBYPASSRLS', 'regress_rsa_owner_a', 'public.tb_note');

-- Still exempt, still fresh.
SET ROLE regress_rsa_owner_b;
UPDATE public.tb_note SET body = 'n1b' WHERE pk_note = 1;
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (3, 2, 'n3');
RESET ROLE;
SET ROLE regress_rsa_owner_a;
UPDATE public.tb_rec SET title = 'r2b' WHERE pk_rec = 2;
RESET ROLE;
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the writes');

SELECT 'row security, statements after CREATE: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note, public.tb_rec;
ALTER ROLE regress_rsa_owner_a NOBYPASSRLS;
SELECT regress_drop_roles('regress_rsa');
