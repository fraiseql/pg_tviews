-- pg_tviews Refresh Status Script
-- Monitor current refresh operations and queue status
-- Run: psql -f docs/operations/runbooks/scripts/refresh-status.sql

\echo '=== pg_tviews Refresh Status ==='
\echo 'Timestamp:' :DATE
\echo ''

-- Current refresh activity
\echo '1. Active Refresh Operations:'
SELECT
    pid,
    query_start,
    EXTRACT(EPOCH FROM (NOW() - query_start)) as duration_seconds,
    LEFT(query, 100) as query_preview
FROM pg_stat_activity
WHERE query LIKE '%tview%' OR query LIKE '%refresh%'
  AND state = 'active'
ORDER BY query_start;

\echo ''
\echo '2. Recent Refresh History (last 30 minutes):'
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms / 1000 as duration_seconds,
    CASE
        WHEN last_error IS NOT NULL THEN 'FAILED'
        WHEN last_refresh_duration_ms > 30000 THEN 'SLOW'
        ELSE 'SUCCESS'
    END as status,
    COALESCE(LEFT(last_error, 50), 'None') as error_preview
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '30 minutes'
ORDER BY last_refreshed DESC;

\echo ''
\echo '3. Queue Processing Status:'
SELECT
    COUNT(*) as total_queued,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending,
    COUNT(*) FILTER (WHERE processed_at IS NOT NULL AND error_message IS NULL) as successful,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed,
    ROUND(AVG(EXTRACT(EPOCH FROM (processed_at - created_at))), 1) as avg_processing_time_seconds,
    ROUND(MAX(EXTRACT(EPOCH FROM (processed_at - created_at))), 1) as max_processing_time_seconds
FROM pg_tviews_queue
WHERE created_at > NOW() - INTERVAL '1 hour';

\echo ''
\echo '4. Queue Backlog Analysis:'
SELECT
    priority,
    COUNT(*) as count,
    ROUND(AVG(EXTRACT(EPOCH FROM (NOW() - created_at))), 0) as avg_age_seconds,
    MIN(created_at) as oldest_item
FROM pg_tviews_queue
WHERE processed_at IS NULL
GROUP BY priority
ORDER BY
    CASE priority
        WHEN 'high' THEN 1
        WHEN 'normal' THEN 2
        WHEN 'low' THEN 3
    END;

\echo ''
\echo '5. Failed Refreshes (last 24 hours):'
SELECT
    entity_name,
    COUNT(*) as failure_count,
    MAX(last_refreshed) as last_failure_time,
    LEFT(MAX(last_error), 100) as latest_error
FROM pg_tviews_metadata
WHERE last_error IS NOT NULL
  AND last_refreshed > NOW() - INTERVAL '24 hours'
GROUP BY entity_name
ORDER BY COUNT(*) DESC;

\echo ''
\echo '6. Performance Trends (by hour):'
SELECT
    DATE_TRUNC('hour', last_refreshed) as hour,
    COUNT(*) as refreshes,
    ROUND(AVG(last_refresh_duration_ms) / 1000, 1) as avg_duration_sec,
    ROUND(MAX(last_refresh_duration_ms) / 1000, 1) as max_duration_sec,
    COUNT(*) FILTER (WHERE last_refresh_duration_ms > 30000) as slow_refreshes
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '24 hours'
GROUP BY DATE_TRUNC('hour', last_refreshed)
ORDER BY hour DESC
LIMIT 6;

\echo ''
\echo '7. System Impact Assessment:'
SELECT
    'Active TVIEW-related connections' as metric,
    COUNT(*) as value
FROM pg_stat_activity
WHERE query LIKE '%tview%' OR query LIKE '%refresh%'

UNION ALL

SELECT
    'Recent block I/O operations' as metric,
    (sum(blks_hit) + sum(blks_read))::text as value
FROM pg_stat_database
WHERE datname = current_database()

UNION ALL

SELECT
    'TVIEW table modifications (last 5 min)' as metric,
    (sum(n_tup_ins) + sum(n_tup_upd) + sum(n_tup_del))::text as value
FROM pg_stat_user_tables
WHERE tablename LIKE '%tview%'
  AND last_analyze > NOW() - INTERVAL '5 minutes';

\echo ''
\echo '=== Refresh Status Complete ==='