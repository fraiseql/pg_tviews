-- pg_tviews Health Check Script
-- Comprehensive health verification for all TVIEWs
-- Run: psql -f docs/operations/runbooks/scripts/health-check.sql

\echo '=== pg_tviews Health Check ==='
\echo 'Timestamp:' :DATE
\echo ''

-- Check 1: TVIEW Inventory
\echo '1. TVIEW Inventory:'
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as recently_refreshed,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as with_errors,
    COUNT(*) FILTER (WHERE last_refreshed < NOW() - INTERVAL '24 hours') as stale_tviews
FROM pg_tviews_metadata;

\echo ''
\echo '2. TVIEW Status Summary:'
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    CASE
        WHEN last_error IS NOT NULL THEN 'ERROR'
        WHEN last_refresh_duration_ms > 30000 THEN 'SLOW'
        WHEN last_refreshed < NOW() - INTERVAL '1 hour' THEN 'STALE'
        ELSE 'HEALTHY'
    END as status,
    COALESCE(last_error, 'None') as last_error
FROM pg_tviews_metadata
ORDER BY
    CASE
        WHEN last_error IS NOT NULL THEN 1
        WHEN last_refreshed < NOW() - INTERVAL '1 hour' THEN 2
        WHEN last_refresh_duration_ms > 30000 THEN 3
        ELSE 4
    END,
    last_refreshed DESC
LIMIT 10;

\echo ''
\echo '3. Queue Status:'
SELECT
    COUNT(*) as total_queued,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed,
    MIN(created_at) as oldest_pending,
    MAX(created_at) as newest_pending,
    ROUND(AVG(EXTRACT(EPOCH FROM (NOW() - created_at))), 0) as avg_age_seconds
FROM pg_tviews_queue;

\echo ''
\echo '4. Performance Summary (last 24h):'
SELECT
    COUNT(*) as refreshes_attempted,
    COUNT(*) FILTER (WHERE last_error IS NULL) as successful_refreshes,
    ROUND(AVG(last_refresh_duration_ms), 0) as avg_duration_ms,
    ROUND(MAX(last_refresh_duration_ms), 0) as max_duration_ms,
    COUNT(*) FILTER (WHERE last_refresh_duration_ms > 30000) as slow_refreshes
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '24 hours';

\echo ''
\echo '5. System Resources:'
SELECT
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections,
    pg_size_pretty((SELECT setting::bigint * 8192 FROM pg_settings WHERE name = 'shared_buffers')) as shared_buffers,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as recent_block_access
FROM pg_stat_bgwriter;

\echo ''
\echo '6. Top Issues (if any):'

-- Check for critical issues
DO $$
DECLARE
    error_count INTEGER;
    stale_count INTEGER;
    slow_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO error_count FROM pg_tviews_metadata WHERE last_error IS NOT NULL;
    SELECT COUNT(*) INTO stale_count FROM pg_tviews_metadata WHERE last_refreshed < NOW() - INTERVAL '2 hours';
    SELECT COUNT(*) INTO slow_count FROM pg_tviews_metadata WHERE last_refresh_duration_ms > 60000;

    IF error_count > 0 THEN
        RAISE NOTICE '❌ CRITICAL: % TVIEWs have errors', error_count;
    END IF;

    IF stale_count > 0 THEN
        RAISE NOTICE '⚠️  WARNING: % TVIEWs are stale (>2 hours)', stale_count;
    END IF;

    IF slow_count > 0 THEN
        RAISE NOTICE '⚠️  WARNING: % TVIEWs are slow (>1 minute)', slow_count;
    END IF;

    IF error_count = 0 AND stale_count = 0 AND slow_count = 0 THEN
        RAISE NOTICE '✅ HEALTHY: All TVIEWs operating normally';
    END IF;
END $$;

\echo ''
\echo '=== Health Check Complete ==='