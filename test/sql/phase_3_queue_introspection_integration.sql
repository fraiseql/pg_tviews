-- Phase 3 Integration Tests: Queue Introspection
-- Tests queue monitoring and introspection capabilities

-- Test 1: Queue info function
SELECT * FROM pg_tviews_queue_info();
-- Expected: (0, {}) - empty queue initially

-- Test 2: Queue monitoring view
SELECT * FROM pg_tviews_queue_realtime;
-- Should show session info with empty queue

-- Create test table and TVIEW
CREATE TABLE test_queue_monitor (
    pk_test_queue_monitor BIGINT PRIMARY KEY,
    data TEXT
);

SELECT pg_tviews_create('queue_monitor', $$
    SELECT pk_test_queue_monitor,
           jsonb_build_object('data', data) as data
    FROM test_queue_monitor
$$);

-- Test 3: Queue population during transaction
BEGIN;
    -- Insert data (should enqueue refresh)
    INSERT INTO test_queue_monitor VALUES (1, 'test_data_1');
    INSERT INTO test_queue_monitor VALUES (2, 'test_data_2');
    INSERT INTO test_queue_monitor VALUES (3, 'test_data_3');

    -- Check queue status
    SELECT * FROM pg_tviews_queue_info();
    -- Expected: (3, {queue_monitor}) - 3 items for queue_monitor entity

    SELECT * FROM pg_tviews_queue_realtime;
    -- Should show current session with queue_size = 3

    -- Insert more data for same entity
    INSERT INTO test_queue_monitor VALUES (4, 'test_data_4');
    INSERT INTO test_queue_monitor VALUES (5, 'test_data_5');

    SELECT * FROM pg_tviews_queue_info();
    -- Expected: (5, {queue_monitor}) - accumulated

COMMIT;

-- After commit, queue should be processed and empty
SELECT * FROM pg_tviews_queue_info();
-- Expected: (0, {}) - queue cleared

-- Test 4: Multiple entities in queue
CREATE TABLE test_queue_monitor2 (
    pk_test_queue_monitor2 BIGINT PRIMARY KEY,
    name TEXT
);

SELECT pg_tviews_create('queue_monitor2', $$
    SELECT pk_test_queue_monitor2,
           jsonb_build_object('name', name) as data
    FROM test_queue_monitor2
$$);

BEGIN;
    -- Mix operations across entities
    INSERT INTO test_queue_monitor VALUES (10, 'cross_entity_1');
    INSERT INTO test_queue_monitor2 VALUES (1, 'entity_2_item');

    SELECT * FROM pg_tviews_queue_info();
    -- Expected: (2, {queue_monitor, queue_monitor2}) - both entities

COMMIT;

-- Test 5: Queue introspection during complex operations
BEGIN;
    -- Bulk insert
    INSERT INTO test_queue_monitor
    SELECT i, 'bulk_' || i::text FROM generate_series(100, 110) i;

    -- Check queue
    SELECT queue_size FROM pg_tviews_queue_info();
    -- Expected: 11 items

    -- Update operation
    UPDATE test_queue_monitor SET data = 'updated' WHERE pk_test_queue_monitor = 1;

    SELECT queue_size FROM pg_tviews_queue_info();
    -- Expected: 12 items (additional update)

COMMIT;

-- Final verification
SELECT * FROM pg_tviews_queue_info();
-- Expected: (0, {}) - all processed

-- Cleanup
DROP TABLE test_queue_monitor CASCADE;
DROP TABLE test_queue_monitor2 CASCADE;
SELECT pg_tviews_drop('queue_monitor');
SELECT pg_tviews_drop('queue_monitor2');