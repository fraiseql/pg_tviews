-- The extension's catalog tables: where they live, the columns every refresh
-- path reads, and their persistence.
--
-- pg_tview_meta and pg_tview_audit_log live in the extension's schema, tviews;
-- the unused pg_tview_helpers table is gone. pg_tview_meta carries a TVIEW's
-- name, backing view, table, definition and stored propagation plan, all NOT
-- NULL. Both tables are WAL-logged: an unlogged catalog would be truncated by
-- crash recovery.
--
--   psql -v ON_ERROR_STOP=1 -f test/sql/regress/catalog/regress_catalog_tables.sql
--
-- expect-output: catalog tables: PASS

\set ON_ERROR_STOP on
SET client_min_messages TO WARNING;

DROP EXTENSION IF EXISTS pg_tviews CASCADE;
DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

CREATE FUNCTION must(ok boolean, what text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN IF ok IS NOT TRUE THEN RAISE EXCEPTION 'catalog tables FAIL: %', what; END IF; END $$;

-- 1. Both catalog tables are in schema tviews; no helpers table.
SELECT must((SELECT count(*) FROM information_schema.tables
             WHERE table_schema = 'tviews'
               AND table_name IN ('pg_tview_meta', 'pg_tview_audit_log')) = 2,
            'a catalog table is missing from schema tviews');
SELECT must(NOT EXISTS (SELECT 1 FROM information_schema.tables
                        WHERE table_schema = 'tviews' AND table_name = 'pg_tview_helpers'),
            'pg_tview_helpers still exists');

-- 2. The columns every refresh path reads, typed and NOT NULL.
SELECT must(missing IS NULL, 'pg_tview_meta lacks NOT NULL column(s): ' || missing)
FROM (SELECT string_agg(want.col, ', ') AS missing
      FROM (VALUES ('entity', 'text'), ('view_oid', 'regclass'), ('table_oid', 'regclass'),
                   ('definition', 'text'), ('plan', 'jsonb')) AS want(col, typ)
      WHERE NOT EXISTS (
          SELECT 1 FROM information_schema.columns c
          WHERE c.table_schema = 'tviews' AND c.table_name = 'pg_tview_meta'
            AND c.column_name = want.col AND c.data_type = want.typ
            AND c.is_nullable = 'NO')) m;

-- 3. Both are WAL-logged.
SELECT must((SELECT count(*) FROM pg_class
             WHERE relnamespace = 'tviews'::regnamespace
               AND relname IN ('pg_tview_meta', 'pg_tview_audit_log')
               AND relpersistence = 'p') = 2,
            'a catalog table is not WAL-logged');

\echo 'catalog tables: PASS'
