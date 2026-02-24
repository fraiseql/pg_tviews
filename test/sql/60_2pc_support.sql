-- Tests for PREPARE TRANSACTION, COMMIT PREPARED, and recovery scenarios

-- Test 1: Basic PREPARE + COMMIT PREPARED flow
BEGIN;
    -- Create test data
    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, name TEXT);
    INSERT INTO tb_user VALUES (1, 'Alice');

    -- Create a simple TVIEW
    SELECT pg_tviews_create('user', 'SELECT pk_user, name FROM tb_user');

    -- Modify data (should queue refresh)
    UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

    -- PREPARE TRANSACTION (should persist queue)
    PREPARE TRANSACTION 'test_2pc_basic';

    -- Verify queue was persisted
    SELECT gid, queue_size FROM pg_tview_pending_refreshes WHERE gid = 'test_2pc_basic';
ROLLBACK; -- Don't commit yet

-- Now commit the prepared transaction using our function
SELECT pg_tviews_commit_prepared('test_2pc_basic');

-- Verify TVIEW was refreshed
SELECT * FROM tv_user;

-- Test 2: PREPARE + ROLLBACK PREPARED flow
BEGIN;
    -- Modify data again
    UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;

    -- PREPARE TRANSACTION
    PREPARE TRANSACTION 'test_rollback';

    -- Verify queue was persisted
    SELECT gid, queue_size FROM pg_tview_pending_refreshes WHERE gid = 'test_rollback';
ROLLBACK; -- Don't commit yet

-- Rollback the prepared transaction
SELECT pg_tviews_rollback_prepared('test_rollback');

-- Verify queue was cleaned up
SELECT COUNT(*) FROM pg_tview_pending_refreshes WHERE gid = 'test_rollback';

-- Test 3: Recovery of orphaned transactions
BEGIN;
    -- Modify data
    UPDATE tb_user SET name = 'Charlie' WHERE pk_user = 1;

    -- PREPARE TRANSACTION
    PREPARE TRANSACTION 'test_recovery';

    -- Simulate the prepared transaction being "orphaned" by manually setting prepared_at to old time
    UPDATE pg_tview_pending_refreshes
    SET prepared_at = now() - interval '2 hours'
    WHERE gid = 'test_recovery';
ROLLBACK; -- Don't commit yet

-- Run recovery (should process the orphaned transaction)
SELECT * FROM pg_tviews_recover_prepared_transactions();

-- Verify the transaction was recovered
SELECT * FROM tv_user;

-- Test 4: Multiple concurrent recoveries should be safe
-- This test would require multiple connections, but we can test the advisory lock
SELECT * FROM pg_tviews_recover_prepared_transactions();
SELECT * FROM pg_tviews_recover_prepared_transactions(); -- Should skip due to lock

-- Test 5: Error handling - invalid GID
SELECT pg_tviews_commit_prepared('nonexistent_gid'); -- Should fail gracefully

-- Test 6: Queue serialization/deserialization
BEGIN;
    UPDATE tb_user SET name = 'Serialization Test' WHERE pk_user = 1;
    PREPARE TRANSACTION 'test_serialization';

    -- Manually inspect the serialized queue
    SELECT gid, queue_size, jsonb_array_length(refresh_queue) as serialized_count
    FROM pg_tview_pending_refreshes
    WHERE gid = 'test_serialization';
ROLLBACK;

-- Cleanup
DROP TABLE tb_user CASCADE;
DROP TABLE pg_tview_pending_refreshes CASCADE;