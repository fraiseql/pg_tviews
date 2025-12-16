-- Phase 5 Integration Tests: Cached Plan Refresh Integration
-- Tests cached vs uncached refresh path selection and performance

-- Test 1: Initial uncached refresh (cache cold)
CREATE TABLE test_cache_refresh (
    pk_test_cache_refresh BIGINT PRIMARY KEY,
    data TEXT
);

SELECT pg_tviews_create('cache_refresh', $$
    SELECT pk_test_cache_refresh,
           jsonb_build_object('data', data) as data
    FROM test_cache_refresh
$$);

-- Insert test data
INSERT INTO test_cache_refresh
SELECT i, 'data_' || i::text FROM generate_series(1, 10) i;

-- First refresh (uncached path)
BEGIN;
    UPDATE test_cache_refresh SET data = 'updated_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 1;
COMMIT;

-- Test 2: Cached refresh (cache warm)
-- Subsequent refreshes should use cached path
BEGIN;
    UPDATE test_cache_refresh SET data = 'cached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 2;

    UPDATE test_cache_refresh SET data = 'cached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 3;
COMMIT;

-- Test 3: Performance comparison
-- Create larger dataset for performance testing
INSERT INTO test_cache_refresh
SELECT i, 'perf_' || i::text FROM generate_series(100, 200) i;

-- Time cached vs uncached performance
\timing on

-- Uncached (simulate by clearing cache)
-- Note: In practice, cache clearing requires internal access
BEGIN;
    UPDATE test_cache_refresh SET data = 'uncached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 100;
COMMIT;

-- Cached (subsequent operations)
BEGIN;
    UPDATE test_cache_refresh SET data = 'cached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 101;

    UPDATE test_cache_refresh SET data = 'cached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 102;

    UPDATE test_cache_refresh SET data = 'cached_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 103;
COMMIT;

\timing off

-- Test 4: Cache invalidation and fallback
-- Simulate schema change that should invalidate cache
ALTER TABLE test_cache_refresh ADD COLUMN new_field TEXT;

BEGIN;
    -- This should detect cache invalidation and fall back to uncached
    UPDATE test_cache_refresh SET data = 'fallback_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 104;
COMMIT;

-- Test 5: Batch operations
BEGIN;
    -- Multiple updates in single transaction
    UPDATE test_cache_refresh SET data = 'batch_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh BETWEEN 200 AND 205;

    -- Check queue status during batch
    SELECT * FROM pg_tviews_queue_info();
    -- Should show accumulated queue items

COMMIT;

-- Test 6: Mixed entity operations
CREATE TABLE test_cache_refresh2 (
    pk_test_cache_refresh2 BIGINT PRIMARY KEY,
    name TEXT
);

SELECT pg_tviews_create('cache_refresh2', $$
    SELECT pk_test_cache_refresh2,
           jsonb_build_object('name', name) as data
    FROM test_cache_refresh2
$$);

INSERT INTO test_cache_refresh2 VALUES (1, 'entity2_item');

BEGIN;
    -- Operations across multiple entities
    UPDATE test_cache_refresh SET data = 'multi_' || pk_test_cache_refresh::text
    WHERE pk_test_cache_refresh = 1;

    UPDATE test_cache_refresh2 SET name = 'updated_entity2'
    WHERE pk_test_cache_refresh2 = 1;

    -- Should show both entities in queue
    SELECT * FROM pg_tviews_queue_info();

COMMIT;

-- Verify all operations completed
SELECT COUNT(*) as total_updates FROM test_cache_refresh
WHERE data LIKE 'cached_%' OR data LIKE 'uncached_%' OR data LIKE 'fallback_%' OR data LIKE 'batch_%';

-- Cleanup
DROP TABLE test_cache_refresh CASCADE;
DROP TABLE test_cache_refresh2 CASCADE;
SELECT pg_tviews_drop('cache_refresh');
SELECT pg_tviews_drop('cache_refresh2');