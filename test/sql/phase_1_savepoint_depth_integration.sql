-- Phase 1 Integration Tests: Savepoint Depth Tracking
-- Tests that savepoint depth is correctly tracked during transactions

-- Test 1: Basic savepoint depth in nested transactions
SELECT pg_tviews_health_check();

-- Create test table
CREATE TABLE test_savepoint_depth (
    pk_test_savepoint_depth BIGINT PRIMARY KEY,
    data TEXT
);

SELECT pg_tviews_create('savepoint_depth', $$
    SELECT pk_test_savepoint_depth,
           jsonb_build_object('data', data) as data
    FROM test_savepoint_depth
$$);

-- Insert initial data
INSERT INTO test_savepoint_depth VALUES (1, 'initial');

-- Test savepoint depth tracking
BEGIN;
    -- At top level: depth should be 0
    INSERT INTO test_savepoint_depth VALUES (2, 'level_0');

    SAVEPOINT sp1;
        -- Inside first savepoint: depth should be 1
        INSERT INTO test_savepoint_depth VALUES (3, 'level_1');

        SAVEPOINT sp2;
            -- Inside nested savepoint: depth should be 2
            INSERT INTO test_savepoint_depth VALUES (4, 'level_2');

            -- Rollback to sp1: should restore level_1 state
            ROLLBACK TO sp1;
        -- After rollback to sp1: back to depth 1

        INSERT INTO test_savepoint_depth VALUES (5, 'after_rollback_sp1');
    -- End of sp1

    INSERT INTO test_savepoint_depth VALUES (6, 'back_to_level_0');
COMMIT;

-- Verify final state
SELECT COUNT(*) as total_rows FROM test_savepoint_depth;
-- Expected: Should have rows 1, 2, 5, 6 (3 and 4 rolled back)

-- Test 2: Queue persistence across savepoints
BEGIN;
    INSERT INTO test_savepoint_depth VALUES (10, 'queue_test');

    SAVEPOINT before_queue;
        -- This should enqueue a refresh request
        INSERT INTO test_savepoint_depth VALUES (11, 'queue_test_2');

        -- Check that queue shows pending items
        SELECT * FROM pg_tviews_queue_info();
        -- Expected: queue_size > 0

        -- Rollback should clear the queue
        ROLLBACK TO before_queue;

        -- Queue should be empty after rollback
        SELECT * FROM pg_tviews_queue_info();
        -- Expected: queue_size = 0

COMMIT;

-- Cleanup
DROP TABLE test_savepoint_depth CASCADE;
SELECT pg_tviews_drop('savepoint_depth');