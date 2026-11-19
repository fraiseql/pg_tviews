-- Rebuilds run a TVIEW's view as the TVIEW's owner, whoever calls them.
--
-- A view runs the functions it calls as the querying role. A rebuild queries a
-- TVIEW's backing view, so a rebuild run as the caller lets a TVIEW owner run code
-- with the privileges of whoever rebuilds, a superuser after a migration. Like
-- REFRESH MATERIALIZED VIEW, every rebuild runs as the TVIEW's owner, and an
-- explicit refresh of one TVIEW requires owning it.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/privileges/regress_admin_rebuild_as_owner.sql
-- expect-output: admin rebuilds: all as owner

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

DROP ROLE IF EXISTS regress_rebuild_low;
DROP ROLE IF EXISTS regress_rebuild_other;
CREATE ROLE regress_rebuild_low;
CREATE ROLE regress_rebuild_other;
GRANT CREATE ON SCHEMA public TO regress_rebuild_low;

-- spy() names who evaluates the view and in which transaction: each entry
-- point, run in a transaction of its own, must leave the owner's name and a new
-- transaction in tv_thing (a definition may not call a volatile function, nor
-- read current_user itself: its body is not visible).
CREATE TABLE public.seen (s text);

SET ROLE regress_rebuild_low;
CREATE FUNCTION public.spy() RETURNS text LANGUAGE plpgsql STABLE AS $$
BEGIN
    RETURN current_user || ':' || txid_current();
END $$;
CREATE TABLE public.tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO public.tb_thing VALUES (1, DEFAULT, 'x');
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, jsonb_build_object('id', id, 'n', n, 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
RESET ROLE;

-- Raise unless tv_thing was just evaluated again, as its owner.
CREATE FUNCTION ran(entry text) RETURNS void LANGUAGE plpgsql AS $f$
DECLARE now_s text := (SELECT data->>'s' FROM public.tv_thing WHERE pk_thing = 1);
BEGIN
    IF split_part(now_s, ':', 1) <> 'regress_rebuild_low' THEN
        RAISE EXCEPTION '% ran the view of tv_thing as %, not as its owner', entry, now_s;
    END IF;
    IF EXISTS (SELECT 1 FROM public.seen WHERE s = now_s) THEN
        RAISE EXCEPTION '% did not rebuild tv_thing (still %)', entry, now_s;
    END IF;
    INSERT INTO public.seen VALUES (now_s);
END $f$;
-- The value its creation computed.
INSERT INTO public.seen SELECT data->>'s' FROM public.tv_thing;

-- Each entry point, called by the superuser.
SET regress.via = 'refresh_all';
SELECT (pg_tviews_refresh_all()->>'refreshed_count')::int AS refreshed;
SELECT ran('refresh_all');
SET regress.via = 'rebuild_all';
SELECT entity FROM pg_tviews_rebuild_all(false);
SELECT ran('rebuild_all');
SET regress.via = 'refresh';
SELECT pg_tviews_refresh('thing');
SELECT ran('refresh');
-- A crash empties tv_thing and its row in pg_tview_valid.
TRUNCATE public.tv_thing;
DELETE FROM tviews.pg_tview_valid;
SET regress.via = 'rebuild_all_only_empty';
SELECT entity FROM pg_tviews_rebuild_all(true);
SELECT ran('rebuild_all_only_empty');
-- A deploy re-applying the definition: the same columns (rows reconciled in
-- place), then a new column (the table rebuilt).
SET regress.via = 'replace';
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, jsonb_build_object('id', id, 'n', n || '!', 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
SELECT ran('replace');
SET regress.via = 'replace_rebuild';
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, n, jsonb_build_object('id', id, 'n', n, 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
SELECT ran('replace_rebuild');
RESET regress.via;

SELECT count(*) = 7 AS every_entry_point_checked FROM public.seen;

-- An explicit refresh of a TVIEW the caller does not own is refused.
SET ROLE regress_rebuild_other;
DO $$
BEGIN
    PERFORM pg_tviews_refresh('thing');
    RAISE EXCEPTION 'a non-owner refreshed tv_thing';
EXCEPTION WHEN insufficient_privilege THEN
    -- Refused up front, not by the TRUNCATE a rebuild as the caller would hit.
    IF SQLERRM NOT LIKE 'must be owner of TVIEW%' THEN
        RAISE EXCEPTION 'refused for the wrong reason: %', SQLERRM;
    END IF;
END $$;
RESET ROLE;

\echo 'admin rebuilds: all as owner'

DROP OWNED BY regress_rebuild_low, regress_rebuild_other;
DROP ROLE regress_rebuild_low;
DROP ROLE regress_rebuild_other;
