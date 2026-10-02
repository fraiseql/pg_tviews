-- pg_tviews Queue Cleanup Script
-- Safe maintenance of the refresh queue
-- Run: psql -f docs/operations/runbooks/scripts/queue-cleanup.sql

\echo '=== pg_tviews Queue Cleanup ==='
\echo 'Timestamp:' :DATE
\echo ''

-- Pre-cleanup assessment
\echo '1. Pre-Cleanup Queue Status:'
SELECT
    COUNT(*) as total_items,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending_items,
    COUNT(*) FILTER (WHERE processed_at IS NOT NULL) as processed_items,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed_items,
    MIN(created_at) as oldest_item,
    MAX(created_at) as newest_item
FROM pg_tviews_queue;

\echo ''
\echo '2. Items Eligible for Cleanup:'

-- Identify stale pending items (older than 2 hours, no active processing)
\echo '   Stale Pending Items (>2 hours, no active processing):'
SELECT
    COUNT(*) as stale_pending_count,
    MIN(created_at) as oldest_stale,
    string_agg(DISTINCT entity_name, ', ' LIMIT 5) as sample_entities
FROM pg_tviews_queue q1
WHERE processed_at IS NULL
  AND created_at < NOW() - INTERVAL '2 hours'
  AND NOT EXISTS (
      SELECT 1 FROM pg_stat_activity
      WHERE query LIKE '%' || q1.entity_name || '%'
         OR query LIKE '%refresh%'
  );

-- Identify old successful items (older than 24 hours)
\echo '   Old Successful Items (>24 hours):'
SELECT COUNT(*) as old_successful_count
FROM pg_tviews_queue
WHERE processed_at IS NOT NULL
  AND error_message IS NULL
  AND processed_at < NOW() - INTERVAL '24 hours';

-- Identify old failed items (older than 7 days)
\echo '   Old Failed Items (>7 days):'
SELECT COUNT(*) as old_failed_count
FROM pg_tviews_queue
WHERE error_message IS NOT NULL
  AND created_at < NOW() - INTERVAL '7 days';

\echo ''
\echo '3. Performing Safe Cleanup:'

-- Step 1: Remove stale pending items (very careful)
\echo '   Step 1: Removing stale pending items...'
WITH stale_items AS (
    DELETE FROM pg_tviews_queue
    WHERE processed_at IS NULL
      AND created_at < NOW() - INTERVAL '2 hours'
      AND ctid IN (
          SELECT q.ctid
          FROM pg_tviews_queue q
          LEFT JOIN pg_stat_activity a ON (
              a.query LIKE '%' || q.entity_name || '%' OR
              a.query LIKE '%refresh%'
          )
          WHERE q.processed_at IS NULL
            AND q.created_at < NOW() - INTERVAL '2 hours'
            AND a.pid IS NULL  -- No active processing
      )
    RETURNING entity_name, primary_key_value, created_at
)
SELECT
    COUNT(*) as stale_items_removed,
    COALESCE(string_agg(entity_name || ':' || primary_key_value::text, ', ' LIMIT 3), 'None') as sample_removed
FROM stale_items;

-- Step 2: Remove old successful items
\echo '   Step 2: Removing old successful items...'
DELETE FROM pg_tviews_queue
WHERE processed_at IS NOT NULL
  AND error_message IS NULL
  AND processed_at < NOW() - INTERVAL '24 hours';

\echo '   Old successful items removed: ' :ROW_COUNT

-- Step 3: Remove very old failed items
\echo '   Step 3: Removing very old failed items...'
DELETE FROM pg_tviews_queue
WHERE error_message IS NOT NULL
  AND created_at < NOW() - INTERVAL '7 days';

\echo '   Old failed items removed: ' :ROW_COUNT

\echo ''
\echo '4. Post-Cleanup Queue Status:'
SELECT
    COUNT(*) as remaining_items,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending_items,
    COUNT(*) FILTER (WHERE processed_at IS NOT NULL AND error_message IS NULL) as successful_items,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed_items,
    MIN(created_at) as oldest_remaining,
    MAX(created_at) as newest_remaining
FROM pg_tviews_queue;

\echo ''
\echo '5. Queue Health Assessment:'

-- Assess queue health after cleanup
DO $$
DECLARE
    total_count INTEGER;
    pending_count INTEGER;
    failed_count INTEGER;
    oldest_pending INTERVAL;
BEGIN
    SELECT COUNT(*) INTO total_count FROM pg_tviews_queue;
    SELECT COUNT(*) INTO pending_count FROM pg_tviews_queue WHERE processed_at IS NULL;
    SELECT COUNT(*) INTO failed_count FROM pg_tviews_queue WHERE error_message IS NOT NULL;

    SELECT NOW() - MIN(created_at) INTO oldest_pending
    FROM pg_tviews_queue WHERE processed_at IS NULL;

    RAISE NOTICE 'Queue Health Summary:';
    RAISE NOTICE '  Total items: %', total_count;
    RAISE NOTICE '  Pending items: %', pending_count;
    RAISE NOTICE '  Failed items: %', failed_count;

    IF pending_count > 100 THEN
        RAISE NOTICE '⚠️  WARNING: High pending count (% items)', pending_count;
    END IF;

    IF failed_count > 10 THEN
        RAISE NOTICE '⚠️  WARNING: High failure count (% items)', failed_count;
    END IF;

    IF oldest_pending > INTERVAL '1 hour' THEN
        RAISE NOTICE '⚠️  WARNING: Old pending items (oldest: %)', oldest_pending;
    END IF;

    IF pending_count <= 100 AND failed_count <= 10 AND (oldest_pending IS NULL OR oldest_pending <= INTERVAL '1 hour') THEN
        RAISE NOTICE '✅ HEALTHY: Queue is in good condition';
    END IF;
END $$;

\echo ''
\echo '6. Recommendations:'

-- Provide recommendations based on current state
DO $$
DECLARE
    pending_count INTEGER;
    failed_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO pending_count FROM pg_tviews_queue WHERE processed_at IS NULL;
    SELECT COUNT(*) INTO failed_count FROM pg_tviews_queue WHERE error_message IS NOT NULL;

    IF pending_count > 500 THEN
        RAISE NOTICE '🚨 URGENT: Very high pending count. Consider emergency procedures.';
    ELSIF pending_count > 100 THEN
        RAISE NOTICE '⚠️  Monitor: High pending count. Watch for processing delays.';
    END IF;

    IF failed_count > 50 THEN
        RAISE NOTICE '🚨 URGENT: High failure rate. Investigate root cause.';
    ELSIF failed_count > 10 THEN
        RAISE NOTICE '⚠️  Investigate: Elevated failure count. Check error patterns.';
    END IF;

    RAISE NOTICE 'Next cleanup recommended: Run weekly or when queue size > 1000 items';
END $$;

\echo ''
\echo '=== Queue Cleanup Complete ==='