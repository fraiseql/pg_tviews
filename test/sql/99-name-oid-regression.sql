-- Regression test for issue #004: name-type OID mismatch
-- This test reproduces the OID mismatch bug in SPI queries that read 'name' type columns
-- without explicit ::text casts. The bug causes refresh operations to fail silently,
-- leaving TVIEWs unpopulated after INSERTs that should trigger refreshes.

\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'Regression Test: Name-Type OID Mismatch'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Create test table and view
CREATE UNLOGGED TABLE tb_nametest (
    pk_nametest BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label TEXT NOT NULL
);
CREATE VIEW v_nametest AS SELECT pk_nametest, label FROM tb_nametest;

-- Register as TVIEW
SELECT pg_tviews_register('nametest', 'v_nametest', 'tb_nametest', 'pk_nametest');

-- Insert data (this should trigger refresh)
INSERT INTO tb_nametest (label) VALUES ('hello');

-- Verify TVIEW is populated (this will fail due to the bug)
DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_nametest;

    IF row_count != 1 THEN
        RAISE EXCEPTION 'TVIEW should have 1 row after INSERT, but has % rows. This indicates the refresh failed due to name-type OID mismatch.', row_count;
    END IF;

    RAISE NOTICE '✓ TVIEW correctly populated after INSERT';
END $$;

-- Clean up
SELECT pg_tviews_drop('nametest');

\echo '=========================================='
\echo 'Regression test completed (should fail until fixes applied)'
\echo '=========================================='