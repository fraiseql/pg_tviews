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

-- Every call of spy() records who ran it, and through which entry point.
CREATE TABLE public.who_ran (u text, via text);
GRANT INSERT ON public.who_ran TO regress_rebuild_low;

SET ROLE regress_rebuild_low;
CREATE FUNCTION public.spy() RETURNS int LANGUAGE plpgsql VOLATILE AS $$
BEGIN
    INSERT INTO public.who_ran VALUES (current_user, current_setting('regress.via', true));
    RETURN 1;
END $$;
CREATE TABLE public.tb_thing (pk_thing int PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(), n text);
INSERT INTO public.tb_thing VALUES (1, DEFAULT, 'x');
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, jsonb_build_object('id', id, 'n', n, 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
RESET ROLE;

-- Each entry point, called by the superuser.
SET regress.via = 'refresh_all';
SELECT (pg_tviews_refresh_all()->>'refreshed_count')::int AS refreshed;
SET regress.via = 'rebuild_all';
SELECT entity FROM pg_tviews_rebuild_all(false);
SET regress.via = 'refresh_all_entities';
SELECT pg_tviews_refresh_all_entities();
SET regress.via = 'refresh';
SELECT pg_tviews_refresh('thing');
-- A crash empties tv_thing and its row in pg_tview_valid.
TRUNCATE public.tv_thing;
DELETE FROM tviews.pg_tview_valid;
SET regress.via = 'rebuild_all_only_empty';
SELECT entity FROM pg_tviews_rebuild_all(true);
TRUNCATE public.tv_thing;
DELETE FROM tviews.pg_tview_valid;
SET regress.via = 'recover_after_crash';
SELECT pg_tviews_recover_after_crash('thing');
-- A deploy re-applying the definition: the same columns (rows reconciled in
-- place), then a new column (the table rebuilt).
SET regress.via = 'replace';
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, jsonb_build_object('id', id, 'n', n || '!', 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
SET regress.via = 'replace_rebuild';
SELECT pg_tviews_create_or_replace('public.tv_thing',
  $q$SELECT pk_thing, id, n, jsonb_build_object('id', id, 'n', n, 's', public.spy()) AS data
     FROM public.tb_thing$q$,
  options => '{"logged": false, "function_reads": {"public.spy()": []}}');
RESET regress.via;

DO $$
DECLARE
    want text[] := ARRAY['refresh_all', 'rebuild_all', 'refresh_all_entities', 'refresh',
                         'rebuild_all_only_empty', 'recover_after_crash', 'replace',
                         'replace_rebuild'];
    entry text;
    users text;
BEGIN
    FOREACH entry IN ARRAY want LOOP
        SELECT string_agg(DISTINCT w.u, ',') INTO users FROM public.who_ran w WHERE w.via = entry;
        IF users IS NULL THEN
            RAISE EXCEPTION '% did not rebuild tv_thing', entry;
        END IF;
        IF users <> 'regress_rebuild_low' THEN
            RAISE EXCEPTION '% ran the view of tv_thing as %, not as its owner', entry, users;
        END IF;
    END LOOP;
END $$;

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
