-- test/sql/00_extension_loading.sql
-- Test: Extension can be created
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Verify extension exists
    SELECT COUNT(*) = 1 AS extension_loaded
    FROM pg_extension
    WHERE extname = 'pg_tviews';

    -- Expected: t (true)
ROLLBACK;