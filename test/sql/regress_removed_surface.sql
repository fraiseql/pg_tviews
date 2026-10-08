-- Functions removed from the extension's SQL surface stay removed.
--
-- A fresh install must not declare them, and the upgrade scripts drop them, so
-- that upgrade-path CI (which compares an updated catalog with a fresh one) keeps
-- both in step. See docs/DEPRECATION_WARNINGS.md for what replaces each.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress_removed_surface.sql
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
          'pg_tviews_delete'
      );
    IF found IS NOT NULL THEN
        RAISE EXCEPTION 'removed functions still declared: %', found;
    END IF;
END $$;

SELECT 'removed surface: none declared' AS result;
