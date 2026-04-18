-- test/sql/01_metadata_tables.sql
-- Test: Metadata tables exist after extension creation
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test 1: pg_tview_meta table exists
    SELECT COUNT(*) = 1 AS meta_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'public'
      AND table_name = 'pg_tview_meta';

    -- Test 2: pg_tview_helpers table exists
    SELECT COUNT(*) = 1 AS helpers_table_exists
    FROM information_schema.tables
    WHERE table_schema = 'public'
      AND table_name = 'pg_tview_helpers';

    -- Test 3: Verify pg_tview_meta schema
    SELECT
        column_name,
        data_type,
        is_nullable
    FROM information_schema.columns
    WHERE table_name = 'pg_tview_meta'
    ORDER BY ordinal_position;

    -- Test 4: Verify catalog tables are WAL-logged (relpersistence = 'p')
    SELECT relname, relpersistence = 'p' AS is_logged
    FROM pg_class
    WHERE relname IN ('pg_tview_meta', 'pg_tview_helpers', 'pg_tview_audit_log')
    ORDER BY relname;

ROLLBACK;