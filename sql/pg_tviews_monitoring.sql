-- Phase 9E: Production Monitoring
-- Enhanced monitoring views and functions for production deployments

-- Real-time queue view
CREATE OR REPLACE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    txid_current() as transaction_id,
    COUNT(*) as queue_size,
    array_agg(DISTINCT entity) as entities,
    MAX(enqueued_at) as last_enqueued
FROM pg_tviews_debug_queue()
GROUP BY current_setting('application_name'), txid_current();

-- Historical performance metrics table
CREATE TABLE IF NOT EXISTS pg_tviews_metrics (
    metric_id BIGSERIAL PRIMARY KEY,
    recorded_at TIMESTAMPTZ DEFAULT now(),
    transaction_id BIGINT,
    queue_size INT,
    refresh_count INT,
    iteration_count INT,
    timing_ms FLOAT,
    graph_cache_hit BOOLEAN,
    table_cache_hits INT,
    prepared_stmt_cache_hits INT,
    prepared_stmt_cache_misses INT,
    bulk_refresh_count INT,
    individual_refresh_count INT
);

-- Function to record metrics (called from Rust code)
CREATE OR REPLACE FUNCTION pg_tviews_record_metrics(
    p_transaction_id BIGINT,
    p_queue_size INT,
    p_refresh_count INT,
    p_iteration_count INT,
    p_timing_ms FLOAT,
    p_graph_cache_hit BOOLEAN,
    p_table_cache_hits INT,
    p_prepared_stmt_cache_hits INT,
    p_prepared_stmt_cache_misses INT,
    p_bulk_refresh_count INT,
    p_individual_refresh_count INT
)
RETURNS void
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO pg_tviews_metrics (
        transaction_id, queue_size, refresh_count, iteration_count, timing_ms,
        graph_cache_hit, table_cache_hits, prepared_stmt_cache_hits,
        prepared_stmt_cache_misses, bulk_refresh_count, individual_refresh_count
    ) VALUES (
        p_transaction_id, p_queue_size, p_refresh_count, p_iteration_count, p_timing_ms,
        p_graph_cache_hit, p_table_cache_hits, p_prepared_stmt_cache_hits,
        p_prepared_stmt_cache_misses, p_bulk_refresh_count, p_individual_refresh_count
    );
END;
$$;

-- pg_stat_statements integration view
CREATE OR REPLACE VIEW pg_tviews_statement_stats AS
SELECT
    query,
    calls,
    total_time,
    mean_time,
    stddev_time,
    rows as rows_affected
FROM pg_stat_statements
WHERE query LIKE '%pg_tview%' OR query LIKE '%tv_%'
ORDER BY total_time DESC;

-- Cache statistics view
CREATE OR REPLACE VIEW pg_tviews_cache_stats AS
SELECT
    'graph_cache' as cache_type,
    COUNT(*) as entries,
    pg_size_pretty(pg_relation_size('pg_tview_meta')) as estimated_size
FROM pg_tview_meta
UNION ALL
SELECT
    'table_cache' as cache_type,
    COUNT(*) as entries,
    pg_size_pretty(COUNT(*) * 64) as estimated_size -- Rough estimate: 64 bytes per entry
FROM (
    SELECT DISTINCT table_oid FROM pg_tview_meta
) t
UNION ALL
SELECT
    'prepared_statements' as cache_type,
    COUNT(*) as entries,
    pg_size_pretty(COUNT(*) * 1024) as estimated_size -- Rough estimate: 1KB per prepared stmt
FROM pg_prepared_statements
WHERE name LIKE 'tview_refresh_%';

-- Performance summary view
CREATE OR REPLACE VIEW pg_tviews_performance_summary AS
SELECT
    date_trunc('hour', recorded_at) as hour,
    COUNT(*) as transactions,
    AVG(queue_size) as avg_queue_size,
    AVG(refresh_count) as avg_refresh_count,
    AVG(iteration_count) as avg_iterations,
    AVG(timing_ms) as avg_timing_ms,
    SUM(bulk_refresh_count) as total_bulk_refreshes,
    SUM(individual_refresh_count) as total_individual_refreshes,
    AVG(CASE WHEN graph_cache_hit THEN 1.0 ELSE 0.0 END) as graph_cache_hit_rate,
    AVG(table_cache_hits::float / NULLIF(table_cache_hits + 1, 0)) as table_cache_hit_rate,
    AVG(prepared_stmt_cache_hits::float / NULLIF(prepared_stmt_cache_hits + prepared_stmt_cache_misses, 0)) as prepared_stmt_hit_rate
FROM pg_tviews_metrics
WHERE recorded_at >= now() - interval '24 hours'
GROUP BY date_trunc('hour', recorded_at)
ORDER BY hour DESC;

-- Function to get current queue debug info (for troubleshooting)
CREATE OR REPLACE FUNCTION pg_tviews_debug_queue()
RETURNS TABLE (
    entity TEXT,
    pk BIGINT,
    enqueued_at TIMESTAMPTZ
)
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
BEGIN
    -- This function would need to be implemented in Rust to access thread-local state
    -- For now, return empty result set
    RETURN QUERY SELECT NULL::TEXT, NULL::BIGINT, NULL::TIMESTAMPTZ WHERE FALSE;
END;
$$;

-- Function to check system health
CREATE OR REPLACE FUNCTION pg_tviews_health_check()
RETURNS TABLE (
    check_name TEXT,
    status TEXT,
    details TEXT
)
LANGUAGE plpgsql
AS $$
BEGIN
    -- Check if extension is properly installed
    RETURN QUERY
    SELECT
        'extension_installed'::TEXT,
        CASE WHEN COUNT(*) > 0 THEN 'OK' ELSE 'ERROR' END,
        'pg_tviews extension ' || CASE WHEN COUNT(*) > 0 THEN 'is' ELSE 'is not' END || ' installed'
    FROM pg_extension WHERE extname = 'pg_tviews';

    -- Check if metadata tables exist
    RETURN QUERY
    SELECT
        'metadata_tables'::TEXT,
        CASE WHEN COUNT(*) = 2 THEN 'OK' ELSE 'ERROR' END,
        COUNT(*) || '/2 metadata tables exist'
    FROM information_schema.tables
    WHERE table_schema = 'public'
    AND table_name IN ('pg_tview_meta', 'pg_tview_helpers');

    -- Check if triggers are installed
    RETURN QUERY
    SELECT
        'statement_triggers'::TEXT,
        CASE WHEN COUNT(*) > 0 THEN 'OK' ELSE 'WARNING' END,
        COUNT(*) || ' statement-level triggers installed'
    FROM pg_trigger
    WHERE tgname LIKE 'pg_tview_stmt_trigger';

    -- Check cache status
    RETURN QUERY
    SELECT
        'cache_status'::TEXT,
        'INFO'::TEXT,
        'Graph cache: ' || (SELECT COUNT(*) FROM pg_tview_meta) || ' entries'
    FROM pg_tview_meta LIMIT 1;
END;
$$;

-- Function to clear old metrics (data retention)
CREATE OR REPLACE FUNCTION pg_tviews_cleanup_metrics(days_old INT DEFAULT 30)
RETURNS INTEGER
LANGUAGE plpgsql
AS $$
DECLARE
    deleted_count INTEGER;
BEGIN
    DELETE FROM pg_tviews_metrics
    WHERE recorded_at < now() - (days_old || ' days')::interval;

    GET DIAGNOSTICS deleted_count = ROW_COUNT;
    RETURN deleted_count;
END;
$$;

-- Grant permissions for monitoring
GRANT SELECT ON pg_tviews_queue_realtime TO PUBLIC;
GRANT SELECT ON pg_tviews_statement_stats TO PUBLIC;
GRANT SELECT ON pg_tviews_cache_stats TO PUBLIC;
GRANT SELECT ON pg_tviews_performance_summary TO PUBLIC;
GRANT EXECUTE ON FUNCTION pg_tviews_health_check() TO PUBLIC;
GRANT EXECUTE ON FUNCTION pg_tviews_cleanup_metrics(INT) TO PUBLIC;