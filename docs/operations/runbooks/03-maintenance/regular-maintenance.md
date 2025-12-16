# Regular Maintenance Runbook

## Purpose
Perform routine maintenance tasks to ensure optimal pg_tviews performance and prevent issues from accumulating over time.

## When to Use
- **Weekly Maintenance**: Standard preventive maintenance during low-usage windows
- **Monthly Deep Maintenance**: Comprehensive cleanup and optimization
- **After Bulk Operations**: Following large data imports or schema changes
- **Performance Degradation**: When system performance declines gradually
- **Storage Alerts**: When disk usage warnings appear

## Prerequisites
- Maintenance window scheduled (1-4 hours depending on scope)
- Database backup completed before maintenance
- System monitoring in place to track impact
- Appropriate database permissions for maintenance operations

## Weekly Maintenance (30 minutes)

### Step 1: Pre-Maintenance Assessment
```sql
-- Check current system state before maintenance
SELECT
    schemaname,
    tablename,
    n_tup_ins,
    n_tup_upd,
    n_tup_del,
    n_live_tup,
    n_dead_tup,
    ROUND(n_dead_tup::numeric / (n_live_tup + n_dead_tup) * 100, 2) as bloat_ratio,
    last_vacuum,
    last_autovacuum,
    last_analyze,
    last_autoanalyze
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY n_dead_tup DESC;
```

### Step 2: Vacuum TVIEW Metadata Tables
```sql
-- Vacuum metadata tables to reclaim space and update statistics
VACUUM ANALYZE pg_tviews_metadata;

-- Check vacuum impact
SELECT
    schemaname,
    tablename,
    last_vacuum,
    last_analyze,
    n_live_tup,
    n_dead_tup
FROM pg_stat_user_tables
WHERE tablename = 'pg_tviews_metadata';
```

### Step 3: Clean Old Queue Items
```sql
-- Remove successfully processed items older than 7 days
DELETE FROM pg_tviews_queue
WHERE processed_at IS NOT NULL
  AND processed_at < NOW() - INTERVAL '7 days';

-- Remove failed items older than 30 days
DELETE FROM pg_tviews_queue
WHERE error_message IS NOT NULL
  AND created_at < NOW() - INTERVAL '30 days';

-- Report cleanup results
SELECT
    'Queue items removed' as action,
    ROW_COUNT as count;
```

### Step 4: Update Table Statistics
```sql
-- Analyze TVIEW tables for better query planning
ANALYZE pg_tviews_metadata;
ANALYZE pg_tviews_queue;

-- Analyze actual TVIEW tables (adjust schema/table names as needed)
DO $$
DECLARE
    tview_record RECORD;
BEGIN
    FOR tview_record IN
        SELECT entity_name FROM pg_tviews_metadata
    LOOP
        BEGIN
            EXECUTE 'ANALYZE ' || tview_record.entity_name;
            RAISE NOTICE 'Analyzed TVIEW: %', tview_record.entity_name;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Failed to analyze TVIEW %: %', tview_record.entity_name, SQLERRM;
        END;
    END LOOP;
END $$;
```

### Step 5: Check Index Health
```sql
-- Verify indexes are being used effectively
SELECT
    schemaname,
    tablename,
    indexname,
    idx_scan,
    idx_tup_read,
    idx_tup_fetch,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY idx_scan DESC;
```

## Monthly Deep Maintenance (2-4 hours)

### Step 1: Comprehensive Vacuum
```sql
-- Full vacuum of all TVIEW-related tables
VACUUM FULL pg_tviews_metadata;
VACUUM FULL pg_tviews_queue;

-- Vacuum TVIEW tables themselves
DO $$
DECLARE
    tview_record RECORD;
BEGIN
    FOR tview_record IN
        SELECT entity_name FROM pg_tviews_metadata
    LOOP
        BEGIN
            EXECUTE 'VACUUM FULL ' || tview_record.entity_name;
            RAISE NOTICE 'Full vacuum completed for TVIEW: %', tview_record.entity_name;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Failed to vacuum TVIEW %: %', tview_record.entity_name, SQLERRM;
        END;
    END LOOP;
END $$;
```

### Step 2: Reindex if Necessary
```sql
-- Check for indexes that might benefit from reindexing
SELECT
    schemaname,
    tablename,
    indexname,
    pg_size_pretty(pg_relation_size(indexrelid)) as size,
    idx_scan,
    CASE
        WHEN idx_scan < 1000 THEN 'POTENTIAL_REINDEX'
        ELSE 'OK'
    END as reindex_status
FROM pg_stat_user_indexes
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY pg_relation_size(indexrelid) DESC;

-- Reindex specific indexes if needed (adjust as necessary)
-- REINDEX INDEX CONCURRENTLY index_name;
```

### Step 3: Archive Old Data
```sql
-- Archive very old queue data to separate table
CREATE TABLE IF NOT EXISTS pg_tviews_queue_archive (
    LIKE pg_tviews_queue INCLUDING ALL
);

-- Move items older than 90 days to archive
INSERT INTO pg_tviews_queue_archive
SELECT * FROM pg_tviews_queue
WHERE created_at < NOW() - INTERVAL '90 days';

DELETE FROM pg_tviews_queue
WHERE created_at < NOW() - INTERVAL '90 days';

-- Report archiving results
SELECT
    'Items archived' as action,
    COUNT(*) as count
FROM pg_tviews_queue_archive
WHERE created_at >= NOW() - INTERVAL '90 days';
```

### Step 4: Performance Baseline Update
```sql
-- Update performance baselines after maintenance
DELETE FROM pg_tviews_performance_baseline
WHERE collected_at < NOW() - INTERVAL '90 days';

INSERT INTO pg_tviews_performance_baseline (metric_name, metric_value, notes)
SELECT
    'avg_refresh_time_post_maintenance',
    AVG(last_refresh_duration_ms),
    'Baseline after ' || TO_CHAR(NOW(), 'YYYY-MM-DD') || ' maintenance'
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '24 hours';
```

## Performance Optimization Tasks

### Step 1: Query Performance Review
```sql
-- Identify slow queries from the last month
SELECT
    query,
    calls,
    total_time / 1000 as total_time_seconds,
    mean_time / 1000 as mean_time_seconds,
    rows
FROM pg_stat_statements
WHERE query LIKE '%tview%' OR query LIKE '%refresh%'
  AND mean_time > 1000  -- Queries taking > 1 second on average
ORDER BY mean_time DESC
LIMIT 10;
```

### Step 2: Configuration Optimization
```sql
-- Check current TVIEW-related settings
SELECT name, setting, unit, context
FROM pg_settings
WHERE name LIKE '%tview%' OR name LIKE '%refresh%' OR name LIKE '%queue%'
ORDER BY name;

-- Suggested optimizations (adjust based on your workload)
-- ALTER SYSTEM SET pg_tviews.queue_batch_size = '100';
-- ALTER SYSTEM SET pg_tviews.refresh_timeout = '300';
-- SELECT pg_reload_conf();
```

### Step 3: Connection Pool Tuning
```sql
-- Monitor connection usage patterns
SELECT
    state,
    COUNT(*) as count,
    ROUND(AVG(EXTRACT(EPOCH FROM (NOW() - query_start))), 0) as avg_age_seconds
FROM pg_stat_activity
GROUP BY state
ORDER BY count DESC;

-- Check for connection leaks
SELECT
    usename,
    client_addr,
    COUNT(*) as connection_count,
    MIN(query_start) as oldest_connection
FROM pg_stat_activity
WHERE state = 'idle'
GROUP BY usename, client_addr
HAVING COUNT(*) > 5
ORDER BY COUNT(*) DESC;
```

## Monitoring and Verification

### Post-Maintenance Validation
```sql
-- Verify maintenance effectiveness
SELECT
    schemaname,
    tablename,
    n_dead_tup,
    n_live_tup,
    ROUND(n_dead_tup::numeric / (n_live_tup + n_dead_tup) * 100, 2) as bloat_ratio,
    last_vacuum,
    last_analyze
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%';

-- Check system performance after maintenance
SELECT
    'System performance check' as check_type,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as block_access
FROM pg_stat_bgwriter;
```

### Maintenance Logging
```sql
-- Log maintenance activities
CREATE TABLE IF NOT EXISTS pg_tviews_maintenance_log (
    maintenance_id SERIAL PRIMARY KEY,
    maintenance_type TEXT,
    started_at TIMESTAMP DEFAULT NOW(),
    completed_at TIMESTAMP,
    items_processed INTEGER,
    notes TEXT
);

-- Record maintenance completion
INSERT INTO pg_tviews_maintenance_log (maintenance_type, completed_at, notes)
VALUES ('weekly_maintenance', NOW(), 'Completed vacuum, analyze, and queue cleanup');
```

## Automated Maintenance

### Cron Job Setup
```bash
# Weekly maintenance cron job
# Add to crontab: 0 2 * * 1 psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f /path/to/weekly-maintenance.sql

# Example weekly maintenance script content:
# VACUUM ANALYZE pg_tviews_metadata;
# VACUUM ANALYZE pg_tviews_queue;
# DELETE FROM pg_tviews_queue WHERE processed_at < NOW() - INTERVAL '7 days';
```

### Monitoring Integration
```sql
-- Create alerts for maintenance needs
CREATE OR REPLACE FUNCTION pg_tviews_maintenance_alerts()
RETURNS TABLE (
    alert_type TEXT,
    severity TEXT,
    description TEXT,
    recommendation TEXT
) AS $$
BEGIN
    -- Check for high bloat
    RETURN QUERY
    SELECT
        'BLOAT'::TEXT,
        'WARNING'::TEXT,
        'High table bloat detected'::TEXT,
        'Schedule vacuum maintenance'::TEXT
    WHERE EXISTS (
        SELECT 1 FROM pg_stat_user_tables
        WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
          AND n_dead_tup > n_live_tup * 0.5
    );

    -- Check for old queue items
    RETURN QUERY
    SELECT
        'QUEUE_MAINTENANCE'::TEXT,
        'INFO'::TEXT,
        'Old queue items need cleanup'::TEXT,
        'Run queue cleanup procedure'::TEXT
    WHERE (SELECT COUNT(*) FROM pg_tviews_queue WHERE created_at < NOW() - INTERVAL '30 days') > 1000;

    RETURN;
END;
$$ LANGUAGE plpgsql;
```

## Troubleshooting Maintenance Issues

### Vacuum Problems
```sql
-- If vacuum is blocked by long-running transactions
SELECT * FROM pg_stat_activity
WHERE state = 'idle in transaction'
  AND query_start < NOW() - INTERVAL '1 hour';

-- Cancel blocking transactions if safe
-- SELECT pg_cancel_backend(pid);
```

### Permission Issues
```sql
-- Check maintenance permissions
SELECT
    usename,
    usesuper,
    usecreatedb,
    userepl
FROM pg_user
WHERE usename = current_user;

-- Grant necessary permissions if needed
-- GRANT pg_signal_backend TO maintenance_user;
```

### Performance Impact
```sql
-- Monitor maintenance impact
SELECT
    query,
    state,
    EXTRACT(EPOCH FROM (NOW() - query_start)) as duration_seconds
FROM pg_stat_activity
WHERE query LIKE '%VACUUM%' OR query LIKE '%ANALYZE%'
ORDER BY query_start;
```

## Related Runbooks

- [Table Analysis](table-analysis.md) - Detailed table maintenance procedures
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Monitor maintenance impact
- [Connection Management](connection-management.md) - Connection-related maintenance
- [Emergency Procedures](../04-incident-response/emergency-procedures.md) - Crisis maintenance

## Best Practices

1. **Schedule Wisely**: Run maintenance during low-usage periods
2. **Monitor Impact**: Track system performance during maintenance
3. **Test First**: Validate maintenance procedures in staging
4. **Document Changes**: Record all maintenance activities
5. **Regular Review**: Adjust maintenance frequency based on system needs
6. **Backup First**: Always backup before major maintenance operations</content>
<parameter name="filePath">docs/operations/runbooks/03-maintenance/regular-maintenance.md