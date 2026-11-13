-- A refresh reads every row of the tables its TVIEW reads (a change no statement
-- the extension follows makes).
--
-- When row security comes to apply on a table a TVIEW reads, to a role it reads
-- that table as, by a change the extension does not see (here the catalog
-- written directly), the next write to a table the TVIEW reads raises, on the
-- row path and on the statement path, and so does every refresh: the flush, a
-- rebuild. Nothing is refreshed until the TVIEW's roles are exempt again.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_row_security_unseen_change.sql
-- expect-output: row security, a change the extension does not see: PASS

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
SELECT regress_roles('regress_rsu');

-- owner_b owns the tables and writes them; owner_a owns the TVIEWs.
SET ROLE regress_rsu_owner_b;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text NOT NULL);
CREATE TABLE public.tb_note (pk_note int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_rec int NOT NULL REFERENCES public.tb_rec, body text NOT NULL);
INSERT INTO public.tb_rec (pk_rec, title) VALUES (1, 'r1'), (2, 'r2');
INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (1, 1, 'n1'), (2, 2, 'n2');
GRANT SELECT, TRIGGER ON public.tb_rec, public.tb_note TO regress_rsu_owner_a;
RESET ROLE;

SET ROLE regress_rsu_owner_a;
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

-- Row security enabled on both tables in the catalog itself.
UPDATE pg_catalog.pg_class SET relrowsecurity = true
 WHERE oid IN ('public.tb_rec'::regclass, 'public.tb_note'::regclass);

SELECT refused('regress_rsu_owner_b', $s$ UPDATE public.tb_rec SET title = 'r1b' WHERE pk_rec = 1 $s$,
               'a write on the row path', 'regress_rsu_owner_a', NULL);
SELECT refused('regress_rsu_owner_b', $s$ UPDATE public.tb_note SET body = 'n1b' WHERE pk_note = 1 $s$,
               'a write on the statement path', 'regress_rsu_owner_a', NULL);
SELECT refused('regress_rsu_owner_b',
               $s$ INSERT INTO public.tb_note (pk_note, fk_rec, body) VALUES (3, 1, 'n3') $s$,
               'an INSERT', 'regress_rsu_owner_a', NULL);
SELECT refused('regress_rsu_owner_a', $s$ SELECT tviews.pg_tviews_refresh('rec') $s$,
               'a rebuild', 'regress_rsu_owner_a', 'public.tb_rec');

-- Exempt again: writes refresh, and the TVIEWs are fresh.
UPDATE pg_catalog.pg_class SET relrowsecurity = false
 WHERE oid IN ('public.tb_rec'::regclass, 'public.tb_note'::regclass);
SET ROLE regress_rsu_owner_b;
UPDATE public.tb_rec SET title = 'r1b' WHERE pk_rec = 1;
UPDATE public.tb_note SET body = 'n1b' WHERE pk_note = 1;
RESET ROLE;
SELECT owner_fresh('public.tv_rec', 'pk_rec', 'the writes');
SELECT owner_fresh('public.tv_recn', 'pk_recn', 'the writes');

SELECT 'row security, a change the extension does not see: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_note, public.tb_rec;
SELECT regress_drop_roles('regress_rsu');
