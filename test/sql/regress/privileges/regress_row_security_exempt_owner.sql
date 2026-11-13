-- A refresh reads every row of the tables its TVIEW reads (an exempt owner is
-- accepted, and the TVIEW stays fresh).
--
-- Policies written for other roles leave a TVIEW alone when the roles it reads
-- its tables as are exempt: here its owner owns the tables (row security
-- enabled, not forced), with policies for readers, one of them reading another
-- table, created before and after the TVIEW. Every write refreshes it as usual,
-- on the row path and on the statement path.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_exempt_owner.sql
-- expect-output: row security for other roles, an exempt owner: PASS

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
SELECT regress_roles('regress_rse');

-- owner_a owns the tables and the TVIEWs, and writes; writer writes too.
SET ROLE regress_rse_owner_a;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL, active boolean NOT NULL DEFAULT true);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_rec int NOT NULL REFERENCES public.tb_rec, body text NOT NULL,
                             active boolean NOT NULL DEFAULT true);
CREATE TABLE public.tb_acl (fk_rec int NOT NULL, reader name NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2'), (3, 'r3');
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (1, 1, 'n1'), (2, 1, 'n2'), (3, 2, 'n3');
GRANT SELECT, UPDATE ON public.tb_rec, public.tb_note TO regress_rse_writer;
GRANT SELECT ON public.tb_rec, public.tb_note, public.tb_acl TO regress_rse_reader;
ALTER TABLE public.tb_rec ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.tb_note ENABLE ROW LEVEL SECURITY;
CREATE POLICY rec_reader ON public.tb_rec FOR SELECT TO regress_rse_reader
    USING (active AND EXISTS (SELECT FROM public.tb_acl a
                              WHERE a.fk_rec = pk_rec AND a.reader = current_user));
CREATE POLICY rec_writer ON public.tb_rec TO regress_rse_writer USING (active);
CREATE POLICY note_writer ON public.tb_note TO regress_rse_writer USING (true);
RESET ROLE;

SET ROLE regress_rse_owner_a;
-- The row path: the table holding the key.
SELECT tviews.pg_tviews_create('public.tv_rec', $q$
    SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title, 'active', r.active) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
-- The statement path: a child collection.
SELECT tviews.pg_tviews_create('public.tv_recn', $q$
    SELECT r.pk_rec AS pk_recn, r.id, jsonb_build_object('title', r.title,
        'notes', (SELECT jsonb_agg(n.body ORDER BY n.pk_note) FROM public.tb_note n
                  WHERE n.fk_rec = r.pk_rec AND n.active)) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
-- A policy added after CREATE, for a reader.
CREATE POLICY note_reader ON public.tb_note FOR SELECT TO regress_rse_reader USING (active);
RESET ROLE;

SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the setup');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the setup');

SET ROLE regress_rse_writer;
UPDATE public.tb_rec SET title = 'r1w' WHERE pk_rec = 1;
UPDATE public.tb_note SET active = false WHERE pk_note = 2;
RESET ROLE;
SET ROLE regress_rse_owner_a;
UPDATE public.tb_rec SET active = false WHERE pk_rec = 1;
UPDATE public.tb_rec SET title = 'r2b' WHERE pk_rec = 2;
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (4, 3, 'n4');
INSERT INTO public.tb_acl VALUES (3, 'regress_rse_reader');
RESET ROLE;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the writes');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the writes');

BEGIN;
SET LOCAL ROLE regress_rse_owner_a;
UPDATE public.tb_rec SET active = true WHERE pk_rec = 1;
UPDATE public.tb_note SET active = true WHERE pk_note = 2;
COMMIT;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'a transaction of writes');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'a transaction of writes');

SELECT 'row security for other roles, an exempt owner: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note, public.tb_rec, public.tb_acl;
SELECT regress_drop_roles('regress_rse');
