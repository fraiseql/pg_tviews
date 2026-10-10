-- test/sql/00_extension_loading.sql
-- Test: Extension can be created
\set ON_ERROR_STOP on

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Verify extension exists
    SELECT COUNT(*) = 1 AS extension_loaded
    FROM pg_extension
    WHERE extname = 'pg_tviews';

    DO $$ BEGIN
        IF (SELECT COUNT(*) FROM pg_extension WHERE extname = 'pg_tviews') <> 1 THEN
            RAISE EXCEPTION 'FAIL: pg_tviews not in pg_extension after CREATE EXTENSION';
        END IF;
    END $$;
ROLLBACK;
