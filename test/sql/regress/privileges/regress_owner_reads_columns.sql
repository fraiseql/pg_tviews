-- A copy stands for the owner's read only while the owner may read every column
-- the plan reads of every table it reads (E1-04).
--
-- A write copied into a TVIEW (a direct patch, a fan-out, a parent patched from
-- its child) stands for a read as the TVIEW's owner. It is taken only while that
-- owner holds SELECT on each column the plan reads of each table, or on every
-- column of a table whose columns the plan does not know; otherwise the row is
-- recomputed as the owner and PostgreSQL decides. The health check and
-- re-registration name what the owner lacks, column by column. SELECT on the
-- columns read, and on no other, is enough for every copy.
--
--   PGTZ=UTC psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_owner_reads_columns.sql
-- expect-output: owner reads, column by column: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;
\set VERBOSITY terse

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;
\ir ../../lib/who.sql
\ir ../../lib/assert_fresh.sql
SELECT regress_roles('regress_orc');

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'FAIL: %', what; END IF; END $$;
CREATE FUNCTION owner_reads_health() RETURNS text LANGUAGE sql AS $$
    SELECT status || ' ' || message FROM tviews.pg_tviews_check_health()
    WHERE component = 'owner_reads' $$;
-- The work one write did: its counters, read in its own transaction around it.
CREATE TABLE seen (q jsonb);
CREATE FUNCTION mark() RETURNS void LANGUAGE sql AS $$
    DELETE FROM seen; INSERT INTO seen SELECT tviews.pg_tviews_show_queue() $$;
CREATE FUNCTION recomputes() RETURNS int LANGUAGE sql AS $$
    SELECT (n->>'view_recomputes')::int - (s.q->>'view_recomputes')::int
    FROM seen s, tviews.pg_tviews_show_queue() AS now(n) $$;
GRANT ALL ON seen TO PUBLIC;

-- owner_b owns the tables and writes them; owner_a's TVIEWs read them, with
-- SELECT on the columns they read and nothing more.
SET ROLE regress_orc_owner_b;
CREATE TABLE public.tb_rec (pk_rec int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            title text, note text, extra text);
CREATE TABLE public.tb_cat (pk_cat int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                            label text, extra text);
CREATE TABLE public.tb_item (pk_item int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_cat int NOT NULL REFERENCES public.tb_cat, name text);
CREATE TABLE public.tb_user (pk_user int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             name text);
CREATE TABLE public.tb_post (pk_post int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                             fk_user int NOT NULL REFERENCES public.tb_user, title text);
CREATE TABLE public.tb_whole (pk_whole int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                              title text, extra text);
INSERT INTO public.tb_rec (pk_rec, title, note) VALUES (1, 'r1', 'n1');
INSERT INTO public.tb_cat (pk_cat, label) VALUES (1, 'c1');
INSERT INTO public.tb_item (pk_item, fk_cat, name) VALUES (1, 1, 'i1'), (2, 1, 'i2');
INSERT INTO public.tb_user (pk_user, name) VALUES (1, 'u1');
INSERT INTO public.tb_post (pk_post, fk_user, title) VALUES (1, 1, 'p1');
INSERT INTO public.tb_whole (pk_whole, title) VALUES (1, 'w1');
GRANT SELECT (pk_rec, id, title, note), TRIGGER ON public.tb_rec TO regress_orc_owner_a;
-- Creating a TVIEW reads a mapped table's every column: tb_cat's are narrowed after.
GRANT SELECT, TRIGGER ON public.tb_cat TO regress_orc_owner_a;
GRANT SELECT, TRIGGER ON public.tb_item, public.tb_post TO regress_orc_owner_a;
GRANT SELECT (pk_whole, id, title, extra), TRIGGER ON public.tb_whole TO regress_orc_owner_a;
-- tv_user is owner_b's; owner_a's tv_post embeds its documents.
SELECT tviews.pg_tviews_create('public.tv_user', $q$
    SELECT u.pk_user, u.id, jsonb_build_object('name', u.name) AS data FROM public.tb_user u $q$)
    IS NOT NULL AS created;
GRANT SELECT (pk_user, id, data), TRIGGER ON public.tv_user TO regress_orc_owner_a;
RESET ROLE;

SET ROLE regress_orc_owner_a;
-- A direct patch: title and note are copied.
SELECT tviews.pg_tviews_create('public.tv_rec', $q$
    SELECT r.pk_rec, r.id, jsonb_build_object('title', r.title, 'note', r.note) AS data
    FROM public.tb_rec r $q$) IS NOT NULL AS created;
-- A fan-out: label is copied into every item of the category.
SELECT tviews.pg_tviews_create('public.tv_item', $q$
    SELECT i.pk_item, i.id, i.fk_cat, jsonb_build_object('name', i.name, 'cat', c.label) AS data
    FROM public.tb_item i JOIN public.tb_cat c ON c.pk_cat = i.fk_cat $q$) IS NOT NULL AS created;
-- A parent patched from its child's document.
SELECT tviews.pg_tviews_create('public.tv_post', $q$
    SELECT p.pk_post, p.id, p.fk_user, jsonb_build_object('title', p.title, 'author', u.data) AS data
    FROM public.tb_post p JOIN public.tv_user u ON u.pk_user = p.fk_user $q$) IS NOT NULL AS created;
-- A table read whole: its columns are not known.
SELECT tviews.pg_tviews_create('public.tv_whole', $q$
    SELECT w.pk_whole, w.id, jsonb_build_object('title', w.title, 'row', to_jsonb(w)) AS data
    FROM public.tb_whole w $q$) IS NOT NULL AS created;
RESET ROLE;
SET ROLE regress_orc_owner_b;
REVOKE SELECT ON public.tb_cat FROM regress_orc_owner_a;
GRANT SELECT (pk_cat, label) ON public.tb_cat TO regress_orc_owner_a;
RESET ROLE;

-- 1. Holding SELECT on exactly the columns read, every copy is taken.
SET ROLE regress_orc_owner_b;
BEGIN; SELECT mark(); UPDATE public.tb_rec SET note = 'n1b';
SELECT must(recomputes() = 0, 'a direct patch the owner may read was recomputed'); COMMIT;
BEGIN; SELECT mark(); UPDATE public.tb_cat SET label = 'c1b';
SELECT must(recomputes() = 0, 'a fan-out the owner may read was recomputed'); COMMIT;
BEGIN; SELECT mark(); UPDATE public.tb_user SET name = 'u1b';
SELECT must(recomputes() = 0, 'a parent patch the owner may read was recomputed'); COMMIT;
RESET ROLE;

-- 2. The owner loses SELECT on one column each TVIEW reads: every write that
--    would copy it fails as the owner's read does (42501), and the TVIEWs keep
--    what the owner last read. The refresh runs at the end of the writing
--    statement: read its SQLSTATE in psql.
SET ROLE regress_orc_owner_b;
REVOKE SELECT (note) ON public.tb_rec FROM regress_orc_owner_a;
REVOKE SELECT (label) ON public.tb_cat FROM regress_orc_owner_a;
REVOKE SELECT (data) ON public.tv_user FROM regress_orc_owner_a;
RESET ROLE;
REVOKE SELECT (extra) ON public.tb_whole FROM regress_orc_owner_a;
SET ROLE regress_orc_owner_b;
\set ON_ERROR_STOP off
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_rec SET note = 'n1c';
SELECT :'LAST_ERROR_SQLSTATE' AS direct_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_cat SET label = 'c1c';
SELECT :'LAST_ERROR_SQLSTATE' AS fanout_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_user SET name = 'u1c';
SELECT :'LAST_ERROR_SQLSTATE' AS parent_state \gset
\set LAST_ERROR_SQLSTATE none
UPDATE public.tb_whole SET title = 'w1c';
SELECT :'LAST_ERROR_SQLSTATE' AS whole_state \gset
\set ON_ERROR_STOP on
RESET ROLE;
SELECT must(:'direct_state' = '42501', 'a direct patch over a column the owner lost: ' || :'direct_state');
SELECT must(:'fanout_state' = '42501', 'a fan-out over a column the owner lost: ' || :'fanout_state');
SELECT must(:'parent_state' = '42501', 'a parent patch over a column the owner lost: ' || :'parent_state');
SELECT must(:'whole_state' = '42501', 'a write to a table read whole: ' || :'whole_state');
SELECT must(data->>'note' = 'n1b', 'tv_rec changed: ' || data::text) FROM public.tv_rec;
SELECT must(data->>'cat' = 'c1b', 'tv_item changed: ' || data::text) FROM public.tv_item WHERE pk_item = 1;
SELECT must(data->'author'->>'name' = 'u1b', 'tv_post changed: ' || data::text) FROM public.tv_post;

-- 3. The health check names each column the owner lacks, and so does
--    re-registration.
SELECT must(owner_reads_health() LIKE 'ERROR %'
            AND owner_reads_health() LIKE '%regress_orc_owner_a lacks SELECT (note) on public.tb_rec%'
            AND owner_reads_health() LIKE '%regress_orc_owner_a lacks SELECT (label) on public.tb_cat%'
            AND owner_reads_health() LIKE '%regress_orc_owner_a lacks SELECT (data) on public.tv_user%'
            AND owner_reads_health() LIKE '%regress_orc_owner_a lacks SELECT on every column of public.tb_whole%',
            'health: ' || owner_reads_health());
DO $$
DECLARE
    state text;
    message text;
BEGIN
    BEGIN
        PERFORM tviews.pg_tviews_reregister('public.tv_rec');
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS state = RETURNED_SQLSTATE, message = MESSAGE_TEXT;
    END;
    PERFORM must(state = '42501' AND message LIKE '%lacks SELECT (note) on public.tb_rec%',
                 format('re-registering tv_rec: %s %s', state, message));
END $$;

-- 4. Granted again: copies are taken, and the TVIEWs follow.
SET ROLE regress_orc_owner_b;
GRANT SELECT (note) ON public.tb_rec TO regress_orc_owner_a;
GRANT SELECT (label) ON public.tb_cat TO regress_orc_owner_a;
GRANT SELECT (data) ON public.tv_user TO regress_orc_owner_a;
RESET ROLE;
GRANT SELECT (extra) ON public.tb_whole TO regress_orc_owner_a;
SET ROLE regress_orc_owner_b;
BEGIN; SELECT mark(); UPDATE public.tb_rec SET note = 'n1d';
SELECT must(recomputes() = 0, 'a direct patch was recomputed after the grant'); COMMIT;
UPDATE public.tb_cat SET label = 'c1d';
UPDATE public.tb_user SET name = 'u1d';
UPDATE public.tb_whole SET title = 'w1d';
RESET ROLE;
SELECT must(owner_reads_health() LIKE 'OK %', 'health after the grants: ' || owner_reads_health());
SELECT assert_fresh('public.tv_rec', 'pk_rec', 'the writes after the grants');
SELECT assert_fresh('public.tv_item', 'pk_item', 'the writes after the grants');
SELECT assert_fresh('public.tv_post', 'pk_post', 'the writes after the grants');
SELECT assert_fresh('public.tv_whole', 'pk_whole', 'the writes after the grants');

SELECT 'owner reads, column by column: PASS' AS result;

DROP EXTENSION pg_tviews CASCADE;
DROP TABLE public.tb_post, public.tb_user, public.tb_item, public.tb_cat, public.tb_rec,
           public.tb_whole, public.seen;
SELECT regress_drop_roles('regress_orc');
