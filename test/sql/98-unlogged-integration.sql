-- UNLOGGED TVIEW Integration Tests
-- Comprehensive end-to-end testing of UNLOGGED TVIEW lifecycle
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

\echo '=========================================='
\echo 'UNLOGGED TVIEW Integration Tests'
\echo 'End-to-end lifecycle validation'
\echo '=========================================='

CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Test 1: Basic UNLOGGED TVIEW lifecycle
\echo ''
\echo '### Test 1: Basic UNLOGGED TVIEW Lifecycle'
\echo 'Creating TVIEW, modifying data, verifying UNLOGGED status'

-- Create base tables with realistic e-commerce data
CREATE TABLE test_users (
    pk_user BIGINT PRIMARY KEY,
    email TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE test_orders (
    pk_order BIGINT PRIMARY KEY,
    fk_user BIGINT NOT NULL REFERENCES test_users,
    total_amount DECIMAL(10,2) NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE test_order_items (
    pk_item BIGINT PRIMARY KEY,
    fk_order BIGINT NOT NULL REFERENCES test_orders,
    product_name TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    unit_price DECIMAL(10,2) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Insert test data
INSERT INTO test_users VALUES
    (1, 'alice@example.com', 'Alice Johnson'),
    (2, 'bob@example.com', 'Bob Smith'),
    (3, 'carol@example.com', 'Carol Williams');

INSERT INTO test_orders VALUES
    (101, 1, 150.00, 'completed'),
    (102, 2, 75.50, 'pending'),
    (103, 1, 200.25, 'shipped');

INSERT INTO test_order_items VALUES
    (1001, 101, 'Laptop', 1, 1200.00),
    (1002, 101, 'Mouse', 2, 25.00),
    (1003, 102, 'Keyboard', 1, 75.50),
    (1004, 103, 'Monitor', 1, 200.25);

-- Create TVIEWs with UNLOGGED enabled (default)
SELECT pg_tviews_create('user_summary', $$
    SELECT
        u.pk_user as id,
        jsonb_build_object(
            'email', u.email,
            'name', u.name,
            'order_count', COALESCE(stats.order_count, 0),
            'total_spent', COALESCE(stats.total_spent, 0.00),
            'last_order', stats.last_order
        ) as data
    FROM test_users u
    LEFT JOIN (
        SELECT
            fk_user,
            COUNT(*) as order_count,
            SUM(total_amount) as total_spent,
            MAX(created_at) as last_order
        FROM test_orders
        GROUP BY fk_user
    ) stats ON stats.fk_user = u.pk_user
$$);

-- Verify TVIEW was created as UNLOGGED
DO $$
DECLARE
    is_unlogged BOOLEAN;
BEGIN
    SELECT c.relpersistence = 'u'
    INTO is_unlogged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_user_summary' AND n.nspname = 'public';

    IF NOT is_unlogged THEN
        RAISE EXCEPTION 'TVIEW should be UNLOGGED by default';
    END IF;

    RAISE NOTICE '✓ TVIEW created as UNLOGGED';
END $$;

-- Verify initial data
DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_user_summary;
    IF row_count != 3 THEN
        RAISE EXCEPTION 'Expected 3 rows, got %', row_count;
    END IF;

    RAISE NOTICE '✓ TVIEW has correct initial data (3 users)';
END $$;

-- Test 2: Data modification and cascade updates
\echo ''
\echo '### Test 2: Data Modification and Cascade Updates'

-- Add a new order
INSERT INTO test_orders VALUES (104, 3, 50.00, 'pending');

-- Update existing order
UPDATE test_orders SET status = 'completed' WHERE pk_order = 102;

-- Verify TVIEW reflects changes
DO $$
DECLARE
    carol_data JSONB;
BEGIN
    SELECT data INTO carol_data
    FROM tv_user_summary
    WHERE id = 3;

    IF carol_data->>'order_count' != '1' THEN
        RAISE EXCEPTION 'Carol should have 1 order, got %', carol_data->>'order_count';
    END IF;

    IF (carol_data->>'total_spent')::DECIMAL != 50.00 THEN
        RAISE EXCEPTION 'Carol should have spent 50.00, got %', carol_data->>'total_spent';
    END IF;

    RAISE NOTICE '✓ TVIEW correctly reflects new order for Carol';
END $$;

-- Test 3: Crash simulation and recovery
\echo ''
\echo '### Test 3: Crash Simulation and Recovery'

-- Simulate crash by truncating UNLOGGED table
TRUNCATE TABLE tv_user_summary;

-- Verify table is empty
DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_user_summary;
    IF row_count != 0 THEN
        RAISE EXCEPTION 'Table should be empty after truncate, got % rows', row_count;
    END IF;

    RAISE NOTICE '✓ UNLOGGED table truncated (simulated crash)';
END $$;

-- Detect crash condition
DO $$
DECLARE
    crash_detected BOOLEAN;
BEGIN
    SELECT pg_tviews_recover_after_crash('user_summary') INTO crash_detected;

    IF NOT crash_detected THEN
        RAISE EXCEPTION 'Crash should have been detected and recovered';
    END IF;

    RAISE NOTICE '✓ Crash detected and recovery initiated';
END $$;

-- Verify data is restored
DO $$
DECLARE
    row_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO row_count FROM tv_user_summary;
    IF row_count != 3 THEN
        RAISE EXCEPTION 'Expected 3 rows after recovery, got %', row_count;
    END IF;

    RAISE NOTICE '✓ Data restored after crash recovery';
END $$;

-- Test 4: GUC parameter control
\echo ''
\echo '### Test 4: GUC Parameter Control'

-- Test creating LOGGED TVIEW when GUC is false
SET pg_tviews.unlogged_by_default TO false;

SELECT pg_tviews_create('logged_test', $$
    SELECT pk_user as id, jsonb_build_object('name', name) as data
    FROM test_users WHERE pk_user = 1
$$);

-- Verify it's LOGGED
DO $$
DECLARE
    is_logged BOOLEAN;
BEGIN
    SELECT c.relpersistence = 'p'
    INTO is_logged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_logged_test' AND n.nspname = 'public';

    IF NOT is_logged THEN
        RAISE EXCEPTION 'TVIEW should be LOGGED when GUC is false';
    END IF;

    RAISE NOTICE '✓ GUC parameter correctly controls LOGGED creation';
END $$;

-- Reset GUC
RESET pg_tviews.unlogged_by_default;

-- Test 5: ALTER TABLE operations
\echo ''
\echo '### Test 5: ALTER TABLE Operations'

-- Alter LOGGED table to UNLOGGED (data preserved)
ALTER TABLE tv_logged_test SET UNLOGGED;

DO $$
DECLARE
    is_unlogged BOOLEAN;
    row_count INTEGER;
BEGIN
    SELECT c.relpersistence = 'u' INTO is_unlogged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_logged_test' AND n.nspname = 'public';

    IF NOT is_unlogged THEN
        RAISE EXCEPTION 'Table should be UNLOGGED after ALTER';
    END IF;

    SELECT COUNT(*) INTO row_count FROM tv_logged_test;
    IF row_count != 1 THEN
        RAISE EXCEPTION 'Data should be preserved, expected 1 row, got %', row_count;
    END IF;

    RAISE NOTICE '✓ ALTER TABLE LOGGED to UNLOGGED preserves data';
END $$;

-- Alter back to LOGGED (data is preserved by PostgreSQL)
ALTER TABLE tv_logged_test SET LOGGED;

DO $$
DECLARE
    is_logged BOOLEAN;
    row_count INTEGER;
BEGIN
    SELECT c.relpersistence = 'p' INTO is_logged
    FROM pg_class c
    JOIN pg_namespace n ON c.relnamespace = n.oid
    WHERE c.relname = 'tv_logged_test' AND n.nspname = 'public';

    IF NOT is_logged THEN
        RAISE EXCEPTION 'Table should be LOGGED after ALTER';
    END IF;

    SELECT COUNT(*) INTO row_count FROM tv_logged_test;
    IF row_count != 1 THEN
        RAISE EXCEPTION 'Data should be preserved after UNLOGGED->LOGGED, expected 1 row, got %', row_count;
    END IF;

    RAISE NOTICE '✓ ALTER TABLE UNLOGGED to LOGGED preserves data';
END $$;

-- Clean up
SELECT pg_tviews_drop('user_summary');
SELECT pg_tviews_drop('logged_test');

\echo ''
\echo '=========================================='
\echo '✅ All UNLOGGED TVIEW integration tests passed!'
\echo '=========================================='