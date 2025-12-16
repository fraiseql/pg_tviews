# Performance Monitoring Runbook

## Purpose
Monitor and analyze pg_tviews performance metrics to ensure optimal operation and identify performance degradation early.

## When to Use
- **Hourly Monitoring**: Check key performance indicators
- **Performance Issues**: When users report slow queries or system sluggishness
- **Capacity Planning**: Before scaling decisions or infrastructure changes
- **Post-Changes**: After TVIEW schema changes or bulk data operations
- **Trend Analysis**: Monthly performance reviews

## Prerequisites
- PostgreSQL monitoring access (`pg_stat_*` views)
- System monitoring tools (CPU, memory, disk I/O)
- Historical performance data (recommended: 30+ days)
- Baseline performance metrics from healthy periods

## Key Performance Indicators (KPIs)

### TVIEW-Specific Metrics

#### Refresh Performance
```sql
-- Current refresh performance
SELECT
    entity_name,
    last_refresh_duration_ms,
    last_refreshed,
    CASE
        WHEN last_refresh_duration_ms < 100 THEN 'EXCELLENT'
        WHEN last_refresh_duration_ms < 500 THEN 'GOOD'
        WHEN last_refresh_duration_ms < 2000 THEN 'FAIR'
        WHEN last_refresh_duration_ms < 5000 THEN 'SLOW'
        ELSE 'CRITICAL'
    END as performance_rating
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '1 hour'
ORDER BY last_refresh_duration_ms DESC;
```

#### Queue Throughput
```sql
-- Queue processing throughput (last hour)
SELECT
    COUNT(*) as items_processed,
    AVG(EXTRACT(EPOCH FROM (processed_at - created_at))) as avg_processing_time_seconds,
    MAX(EXTRACT(EPOCH FROM (processed_at - created_at))) as max_processing_time_seconds,
    COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (processed_at - created_at)) > 30) as slow_items
FROM pg_tviews_queue
WHERE processed_at > NOW() - INTERVAL '1 hour';
```

### System Resource Metrics

#### Database Performance
```sql
-- Database-wide performance indicators
SELECT
    datname,
    numbackends,
    xact_commit,
    xact_rollback,
    blks_read,
    blks_hit,
    tup_returned,
    tup_fetched,
    tup_inserted,
    tup_updated,
    tup_deleted
FROM pg_stat_database
WHERE datname = current_database();
```

#### Table Statistics
```sql
-- TVIEW table performance
SELECT
    schemaname,
    tablename,
    seq_scan,
    seq_tup_read,
    idx_scan,
    idx_tup_fetch,
    n_tup_ins,
    n_tup_upd,
    n_tup_del,
    n_live_tup,
    n_dead_tup
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY n_tup_ins + n_tup_upd + n_tup_del DESC;
```

## Performance Baselines

### Establish Baselines (Monthly)
```sql
-- Create performance baseline (run during healthy periods)
CREATE TABLE IF NOT EXISTS pg_tviews_performance_baseline (
    collected_at TIMESTAMP DEFAULT NOW(),
    metric_name TEXT,
    metric_value NUMERIC,
    notes TEXT
);

-- Insert current baseline metrics
INSERT INTO pg_tviews_performance_baseline (metric_name, metric_value, notes)
SELECT
    'avg_refresh_time_ms',
    AVG(last_refresh_duration_ms),
    'Baseline from healthy period'
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '24 hours'

UNION ALL

SELECT
    'queue_throughput_per_hour',
    COUNT(*) / 24.0,
    'Items processed per hour baseline'
FROM pg_tviews_queue
WHERE processed_at > NOW() - INTERVAL '24 hours';
```

### Compare to Baselines
```sql
-- Compare current performance to baseline
WITH current_metrics AS (
    SELECT
        'avg_refresh_time_ms' as metric,
        AVG(last_refresh_duration_ms) as current_value
    FROM pg_tviews_metadata
    WHERE last_refreshed > NOW() - INTERVAL '1 hour'

    UNION ALL

    SELECT
        'queue_throughput_per_hour' as metric,
        COUNT(*) * 1.0 as current_value
    FROM pg_tviews_queue
    WHERE processed_at > NOW() - INTERVAL '1 hour'
)
SELECT
    cm.metric,
    cm.current_value,
    pb.metric_value as baseline_value,
    ROUND((cm.current_value - pb.metric_value) / pb.metric_value * 100, 2) as percent_change,
    CASE
        WHEN ABS((cm.current_value - pb.metric_value) / pb.metric_value) > 0.5 THEN 'CRITICAL'
        WHEN ABS((cm.current_value - pb.metric_value) / pb.metric_value) > 0.25 THEN 'WARNING'
        ELSE 'NORMAL'
    END as status
FROM current_metrics cm
JOIN pg_tviews_performance_baseline pb ON cm.metric = pb.metric_name
WHERE pb.collected_at > NOW() - INTERVAL '30 days'
ORDER BY ABS((cm.current_value - pb.metric_value) / pb.metric_value) DESC;
```

## Performance Analysis Procedures

### Step 1: Identify Slow TVIEWs
```sql
-- Find TVIEWs with performance degradation
SELECT
    entity_name,
    last_refresh_duration_ms,
    last_refreshed,
    (SELECT AVG(last_refresh_duration_ms)
     FROM pg_tviews_metadata m2
     WHERE m2.entity_name = m1.entity_name
       AND m2.last_refreshed > NOW() - INTERVAL '7 days') as week_avg,
    CASE
        WHEN last_refresh_duration_ms > (SELECT AVG(last_refresh_duration_ms) * 2
                                        FROM pg_tviews_metadata m2
                                        WHERE m2.entity_name = m1.entity_name
                                          AND m2.last_refreshed > NOW() - INTERVAL '7 days')
        THEN 'DEGRADED'
        ELSE 'NORMAL'
    END as status
FROM pg_tviews_metadata m1
WHERE last_refreshed > NOW() - INTERVAL '24 hours'
ORDER BY last_refresh_duration_ms DESC;
```

### Step 2: Analyze System Bottlenecks
```sql
-- Check for system resource issues
SELECT
    'CPU' as resource,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections

UNION ALL

SELECT
    'Memory' as resource,
    (SELECT setting FROM pg_settings WHERE name = 'shared_buffers') as shared_buffers,
    (SELECT setting FROM pg_settings WHERE name = 'work_mem') as work_mem

UNION ALL

SELECT
    'I/O' as resource,
    (SELECT sum(blks_read) FROM pg_stat_database) as blocks_read,
    (SELECT sum(blks_hit) FROM pg_stat_database) as blocks_hit;
```

### Step 3: Query Performance Analysis
```sql
-- Analyze slow queries affecting TVIEWs
SELECT
    query,
    calls,
    total_time / 1000 as total_time_seconds,
    mean_time / 1000 as mean_time_seconds,
    rows
FROM pg_stat_statements
WHERE query LIKE '%tview%' OR query LIKE '%pg_tviews%'
ORDER BY mean_time DESC
LIMIT 10;
```

## Performance Optimization

### Index Optimization
```sql
-- Check for missing indexes on TVIEW tables
SELECT
    schemaname,
    tablename,
    attname,
    n_distinct,
    correlation
FROM pg_stats
WHERE schemaname LIKE '%tview%'
  AND attname IN ('primary_key_column', 'updated_at', 'created_at')
  AND n_distinct > 1000
ORDER BY n_distinct DESC;
```

### Table Maintenance
```sql
-- Check for table bloat
SELECT
    schemaname,
    tablename,
    n_dead_tup,
    n_live_tup,
    ROUND(n_dead_tup::numeric / (n_live_tup + n_dead_tup) * 100, 2) as bloat_ratio
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
  AND n_dead_tup > 1000
ORDER BY bloat_ratio DESC;
```

### Configuration Tuning
```sql
-- Check current TVIEW-related settings
SELECT name, setting, unit, context
FROM pg_settings
WHERE name LIKE '%tview%' OR name LIKE '%refresh%' OR name LIKE '%queue%'
ORDER BY name;
```

## Automated Monitoring

### Performance Alert Queries
```sql
-- Critical alerts (immediate action)
SELECT 'CRITICAL: Slow refresh detected' as alert
WHERE EXISTS (
    SELECT 1 FROM pg_tviews_metadata
    WHERE last_refresh_duration_ms > 30000
      AND last_refreshed > NOW() - INTERVAL '30 minutes'
);

-- Warning alerts (investigate)
SELECT 'WARNING: Queue backlog growing' as alert
WHERE (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) > 100;

-- Info alerts (monitor trends)
SELECT 'INFO: Performance trending down' as alert
WHERE (
    SELECT AVG(last_refresh_duration_ms)
    FROM pg_tviews_metadata
    WHERE last_refreshed > NOW() - INTERVAL '1 hour'
) > (
    SELECT AVG(last_refresh_duration_ms)
    FROM pg_tviews_metadata
    WHERE last_refreshed > NOW() - INTERVAL '24 hours'
) * 1.5;
```

### Monitoring Script
```bash
# Run comprehensive performance monitoring
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f docs/operations/runbooks/scripts/performance-monitor.sql
```

## Troubleshooting Performance Issues

### Slow Refresh Diagnosis
```sql
-- Step 1: Identify the slow TVIEW
SELECT entity_name, last_refresh_duration_ms, last_refreshed
FROM pg_tviews_metadata
ORDER BY last_refresh_duration_ms DESC
LIMIT 1;

-- Step 2: Check system load during refresh
SELECT * FROM pg_stat_activity
WHERE query_start < NOW() - INTERVAL '30 minutes'
ORDER BY query_start ASC;

-- Step 3: Analyze the TVIEW structure
SELECT * FROM information_schema.columns
WHERE table_schema || '.' || table_name = 'your_slow_tview'
ORDER BY ordinal_position;
```

### Memory Issues
```sql
-- Check memory usage patterns
SELECT
    name,
    setting,
    CASE
        WHEN name = 'shared_buffers' THEN setting || ' (' || ROUND(setting::numeric / 1024 / 1024, 1) || ' GB)'
        WHEN name = 'work_mem' THEN setting || ' (' || ROUND(setting::numeric / 1024, 1) || ' MB)'
        WHEN name = 'maintenance_work_mem' THEN setting || ' (' || ROUND(setting::numeric / 1024, 1) || ' MB)'
        ELSE setting
    END as readable_setting
FROM pg_settings
WHERE name IN ('shared_buffers', 'work_mem', 'maintenance_work_mem');
```

### I/O Bottlenecks
```sql
-- Check I/O performance
SELECT
    schemaname,
    tablename,
    seq_scan,
    seq_tup_read,
    idx_scan,
    idx_tup_fetch,
    ROUND(seq_tup_read::numeric / GREATEST(seq_scan, 1), 2) as avg_tuples_per_seq_scan,
    ROUND(idx_tup_fetch::numeric / GREATEST(idx_scan, 1), 2) as avg_tuples_per_idx_scan
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY seq_tup_read DESC;
```

## Related Runbooks

- [TVIEW Health Check](tview-health-check.md) - Overall system health
- [Queue Management](queue-management.md) - Queue-specific performance
- [Table Analysis](../03-maintenance/table-analysis.md) - Storage optimization
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Specific refresh issues

## Best Practices

1. **Establish Baselines**: Measure performance during healthy periods
2. **Monitor Trends**: Watch for gradual performance degradation
3. **Set Alerts**: Configure monitoring for critical thresholds
4. **Regular Analysis**: Review performance metrics monthly
5. **Document Changes**: Track performance impact of configuration changes
6. **Capacity Planning**: Use performance data for scaling decisions</content>
<parameter name="filePath">docs/operations/runbooks/01-health-monitoring/performance-monitoring.md