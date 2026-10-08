-- Functions removed from the extension's SQL surface stay removed.
--
-- A fresh install must not declare them, and the upgrade scripts drop them, so
-- that upgrade-path CI (which compares an updated catalog with a fresh one) keeps
-- both in step. See docs/DEPRECATION_WARNINGS.md for what replaces each.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_removed_surface.sql
-- expect-output: removed surface: none declared

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION pg_tviews;

DO $$
DECLARE
    found text;
BEGIN
    SELECT string_agg(p.oid::regprocedure::text, ', ' ORDER BY p.proname) INTO found
    FROM pg_proc p
    WHERE p.pronamespace = 'tviews'::regnamespace
      AND p.proname IN (
          -- Text-pattern schema analysis (replaced by the query-tree analysis
          -- every TVIEW is registered with).
          'pg_tviews_analyze_select',
          'pg_tviews_infer_types',
          -- Name-guessing manual cascade (a write to the base table does it).
          'pg_tviews_cascade',
          'pg_tviews_insert',
          'pg_tviews_delete',
          -- Legacy metadata and trigger migration (every TVIEW is re-derived by
          -- the update to 0.1.0-beta.27, its plan rebound by the catalog trigger).
          'pg_tviews_rebind_cascade_paths',
          'pg_tviews_migrate_triggers',
          -- Table conversion (CREATE TABLE tv_x AS SELECT or pg_tviews_create).
          'pg_tviews_convert_existing_table',
          'pg_tviews_convert_table'
      );
    IF found IS NOT NULL THEN
        RAISE EXCEPTION 'removed functions still declared: %', found;
    END IF;
END $$;

-- Removed settings: a pg_tviews.* name no longer defined is refused.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_settings WHERE name = 'pg_tviews.metrics_enabled') THEN
        RAISE EXCEPTION 'removed setting still defined: pg_tviews.metrics_enabled';
    END IF;
END $$;

-- A non-TVIEW CREATE TABLE tv_x AS the hook did not intercept is still refused
-- (the event trigger raises it itself now).
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_event_trigger WHERE evtname = 'pg_tviews_ddl_end') THEN
        RAISE EXCEPTION 'the CREATE TABLE AS event trigger is gone';
    END IF;
END $$;

SELECT 'removed surface: none declared' AS result;
