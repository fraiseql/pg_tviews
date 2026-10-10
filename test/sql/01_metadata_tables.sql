-- test/sql/01_metadata_tables.sql
-- Test: Metadata tables exist after extension creation
\set ON_ERROR_STOP on

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- The catalog lives in the extension's own schema, tviews.

    -- Test 1: pg_tview_meta table exists
    SELECT COUNT(*) = 1 AS meta_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'tviews'
      AND table_name = 'pg_tview_meta';

    -- Test 2: pg_tview_helpers table exists
    SELECT COUNT(*) = 1 AS helpers_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'tviews'
      AND table_name = 'pg_tview_helpers';

    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM information_schema.tables
            WHERE table_schema = 'tviews'
              AND table_name IN ('pg_tview_meta', 'pg_tview_helpers', 'pg_tview_audit_log')) <> 3 THEN
            RAISE EXCEPTION 'FAIL: a catalog table is missing from schema tviews';
        END IF;
    END $$;

    -- Test 3: Verify pg_tview_meta schema
    SELECT
        column_name,
        data_type,
        is_nullable
    FROM information_schema.columns
    WHERE table_schema = 'tviews'
      AND table_name = 'pg_tview_meta'
    ORDER BY ordinal_position;

    -- The columns every refresh path reads: a TVIEW's name, its backing view,
    -- its table, its definition and its stored propagation plan.
    DO $$
    DECLARE missing text;
    BEGIN
        SELECT string_agg(want.col, ', ') INTO missing
        FROM (VALUES ('entity', 'text'), ('view_oid', 'regclass'), ('table_oid', 'regclass'),
                     ('definition', 'text'), ('plan', 'jsonb')) AS want(col, typ)
        WHERE NOT EXISTS (
            SELECT 1 FROM information_schema.columns c
            WHERE c.table_schema = 'tviews' AND c.table_name = 'pg_tview_meta'
              AND c.column_name = want.col AND c.data_type = want.typ
              AND c.is_nullable = 'NO');
        IF missing IS NOT NULL THEN
            RAISE EXCEPTION 'FAIL: pg_tview_meta lacks NOT NULL column(s): %', missing;
        END IF;
    END $$;

    -- Test 4: Verify catalog tables are WAL-logged (relpersistence = 'p')
    SELECT relname, relpersistence = 'p' AS is_logged
    FROM pg_class
    WHERE relnamespace = 'tviews'::regnamespace
      AND relname IN ('pg_tview_meta', 'pg_tview_helpers', 'pg_tview_audit_log')
    ORDER BY relname;

    -- An unlogged catalog would be truncated by crash recovery.
    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM pg_class
            WHERE relnamespace = 'tviews'::regnamespace
              AND relname IN ('pg_tview_meta', 'pg_tview_helpers', 'pg_tview_audit_log')
              AND relpersistence = 'p') <> 3 THEN
            RAISE EXCEPTION 'FAIL: a catalog table is not WAL-logged';
        END IF;
    END $$;

ROLLBACK;
