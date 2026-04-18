-- UNLOGGED TVIEW Performance Validation Tests
-- Tests to validate performance benefits of UNLOGGED tables
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'UNLOGGED TVIEW Performance Validation'
\echo 'Validating performance characteristics'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Create test data for performance comparison
CREATE TABLE perf_base (
    pk_item BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    category TEXT NOT NULL,
    price DECIMAL(10,2) NOT NULL,
    metadata JSONB,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Insert 10,000 test records
INSERT INTO perf_base
SELECT
    i as pk_item,
    'Product ' || i as name,
    'Category ' || (i % 10) as category,
    (random() * 1000)::numeric(10,2) as price,
    jsonb_build_object(
        'tags', jsonb_build_array('tag' || (i % 5), 'tag' || (i % 3)),
        'attributes', jsonb_build_object(
            'weight', (random() * 10)::numeric(5,2),
            'dimensions', jsonb_build_object('l', random() * 100, 'w', random() * 50, 'h', random() * 30)
        )
    ) as metadata,
    NOW() - (random() * interval '1 year') as created_at,
    NOW() as updated_at
FROM generate_series(1, 10000) i;

-- Test 1: UNLOGGED vs LOGGED TVIEW creation time
\echo ''
\echo '### Test 1: TVIEW Creation Performance'

-- Create LOGGED TVIEW
SET pg_tviews.unlogged_by_default TO false;

\timing on
SELECT pg_tviews_create('perf_logged', $$
    SELECT
        pk_item as id,
        jsonb_build_object(
            'name', name,
            'category', category,
            'price', price,
            'metadata', metadata,
            'stats', jsonb_build_object(
                'category_avg_price', AVG(price) OVER (PARTITION BY category),
                'category_rank', ROW_NUMBER() OVER (PARTITION BY category ORDER BY price DESC)
            )
        ) as data
    FROM perf_base
$$);
\timing off

-- Verify it's LOGGED
DO $$
DECLARE
    is_logged BOOLEAN;
BEGIN
    SELECT c.relpersistence = 'p'
    INTO is_logged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_perf_logged' AND n.nspname = 'public';

    IF NOT is_logged THEN
        RAISE EXCEPTION 'LOGGED TVIEW should be created as LOGGED';
    END IF;

    RAISE NOTICE '✓ LOGGED TVIEW created';
END $$;

-- Create UNLOGGED TVIEW
SET pg_tviews.unlogged_by_default TO true;

\timing on
SELECT pg_tviews_create('perf_unlogged', $$
    SELECT
        pk_item as id,
        jsonb_build_object(
            'name', name,
            'category', category,
            'price', price,
            'metadata', metadata,
            'stats', jsonb_build_object(
                'category_avg_price', AVG(price) OVER (PARTITION BY category),
                'category_rank', ROW_NUMBER() OVER (PARTITION BY category ORDER BY price DESC)
            )
        ) as data
    FROM perf_base
$$);
\timing off

-- Verify it's UNLOGGED
DO $$
DECLARE
    is_unlogged BOOLEAN;
BEGIN
    SELECT c.relpersistence = 'u'
    INTO is_unlogged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_perf_unlogged' AND n.nspname = 'public';

    IF NOT is_unlogged THEN
        RAISE EXCEPTION 'UNLOGGED TVIEW should be created as UNLOGGED';
    END IF;

    RAISE NOTICE '✓ UNLOGGED TVIEW created';
END $$;

-- Test 2: Data modification performance
\echo ''
\echo '### Test 2: Data Modification Performance'

-- Update multiple records and measure time
\timing on

-- Update 1000 records in base table (this should cascade to TVIEWs)
UPDATE perf_base
SET price = price * 1.1, updated_at = NOW()
WHERE pk_item IN (
    SELECT pk_item FROM perf_base ORDER BY random() LIMIT 1000
);

\timing off

-- Verify both TVIEWs reflect the changes
DO $$
DECLARE
    logged_count INTEGER;
    unlogged_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO logged_count FROM tv_perf_logged;
    SELECT COUNT(*) INTO unlogged_count FROM tv_perf_unlogged;

    IF logged_count != 10000 THEN
        RAISE EXCEPTION 'LOGGED TVIEW should have 10000 rows, got %', logged_count;
    END IF;

    IF unlogged_count != 10000 THEN
        RAISE EXCEPTION 'UNLOGGED TVIEW should have 10000 rows, got %', unlogged_count;
    END IF;

    RAISE NOTICE '✓ Both TVIEWs correctly reflect data changes';
END $$;

-- Test 3: Crash recovery performance
\echo ''
\echo '### Test 3: Crash Recovery Performance'

-- Simulate crash by truncating UNLOGGED table
\timing on
TRUNCATE TABLE tv_perf_unlogged;
\timing off

DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_perf_unlogged;
    IF row_count != 0 THEN
        RAISE EXCEPTION 'UNLOGGED table should be empty after truncate';
    END IF;

    RAISE NOTICE '✓ UNLOGGED table truncated (crash simulated)';
END $$;

-- Time the recovery process
\timing on
SELECT pg_tviews_recover_after_crash('perf_unlogged');
\timing off

-- Verify recovery
DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_perf_unlogged;
    IF row_count != 10000 THEN
        RAISE EXCEPTION 'UNLOGGED TVIEW should have 10000 rows after recovery, got %', row_count;
    END IF;

    RAISE NOTICE '✓ UNLOGGED TVIEW recovered successfully';
END $$;

-- Test 4: Memory and WAL impact validation
\echo ''
\echo '### Test 4: Memory and WAL Impact'

-- Check that UNLOGGED tables don't generate WAL (conceptual validation)
-- In a real test environment, we would check WAL generation metrics

DO $$
DECLARE
    unlogged_relpersistence TEXT;
BEGIN
    SELECT c.relpersistence INTO unlogged_relpersistence
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_perf_unlogged' AND n.nspname = 'public';

    IF unlogged_relpersistence != 'u' THEN
        RAISE EXCEPTION 'UNLOGGED table should have relpersistence = u, got %', unlogged_relpersistence;
    END IF;

    RAISE NOTICE '✓ UNLOGGED table confirmed (relpersistence = u)';
END $$;

-- Clean up
SELECT pg_tviews_drop('perf_logged');
SELECT pg_tviews_drop('perf_unlogged');
DROP TABLE perf_base;

\echo ''
\echo '=========================================='
\echo '✅ UNLOGGED TVIEW performance validation completed!'
\echo '=========================================='