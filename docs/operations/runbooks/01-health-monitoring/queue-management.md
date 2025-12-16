# Queue Management Runbook

## Purpose
Monitor, maintain, and troubleshoot the pg_tviews refresh queue to ensure efficient processing of TVIEW updates.

## When to Use
- **Daily Monitoring**: Check queue status and health
- **Performance Issues**: When refresh operations seem slow or stuck
- **After Failures**: Clean up after system crashes or transaction failures
- **Maintenance Windows**: Regular queue cleanup and optimization
- **Incident Response**: As part of diagnosing synchronization problems

## Prerequisites
- PostgreSQL CLI access (`psql`)
- Database credentials with SELECT/DELETE permissions on queue tables
- Understanding of TVIEW refresh mechanics
- Access to system logs for troubleshooting

## Queue Status Check (5 minutes)

### Step 1: Current Queue Overview
```sql
-- Get comprehensive queue status
SELECT
    COUNT(*) as total_items,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending_items,
    COUNT(*) FILTER (WHERE processed_at IS NOT NULL) as processed_items,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed_items,
    MIN(created_at) as oldest_pending,
    MAX(created_at) as newest_pending,
    AVG(EXTRACT(EPOCH FROM (NOW() - created_at))) FILTER (WHERE processed_at IS NULL) as avg_pending_age_seconds
FROM pg_tviews_queue;
```

### Step 2: Priority Distribution
```sql
-- Check priority distribution
SELECT
    priority,
    COUNT(*) as count,
    MIN(created_at) as oldest,
    MAX(created_at) as newest
FROM pg_tviews_queue
WHERE processed_at IS NULL
GROUP BY priority
ORDER BY
    CASE priority
        WHEN 'high' THEN 1
        WHEN 'normal' THEN 2
        WHEN 'low' THEN 3
    END;
```

### Step 3: Entity Distribution
```sql
-- Check which TVIEWs have queued items
SELECT
    entity_name,
    COUNT(*) as queued_items,
    MIN(created_at) as oldest_item,
    MAX(created_at) as newest_item
FROM pg_tviews_queue
WHERE processed_at IS NULL
GROUP BY entity_name
ORDER BY COUNT(*) DESC;
```

## Queue Maintenance Procedures

### Routine Cleanup (15 minutes)

Run this weekly or when queue grows beyond normal levels:

#### Step 1: Identify Stale Items
```sql
-- Find items older than 1 hour that haven't been processed
SELECT
    entity_name,
    primary_key_value,
    priority,
    created_at,
    NOW() - created_at as age
FROM pg_tviews_queue
WHERE processed_at IS NULL
  AND created_at < NOW() - INTERVAL '1 hour'
ORDER BY created_at ASC;
```

#### Step 2: Safe Cleanup of Stale Items
```sql
-- Only remove items that are truly stale (no active transaction)
-- This query identifies items that can be safely removed
WITH active_transactions AS (
    SELECT DISTINCT entity_name, primary_key_value
    FROM pg_tviews_queue q
    WHERE processed_at IS NULL
      AND EXISTS (
          SELECT 1 FROM pg_stat_activity
          WHERE query LIKE '%' || q.entity_name || '%'
             OR query LIKE '%refresh%'
      )
)
DELETE FROM pg_tviews_queue
WHERE processed_at IS NULL
  AND created_at < NOW() - INTERVAL '2 hours'
  AND (entity_name, primary_key_value) NOT IN (
      SELECT entity_name, primary_key_value FROM active_transactions
  );
```

#### Step 3: Verify Cleanup
```sql
-- Check that cleanup was successful
SELECT COUNT(*) as remaining_stale_items
FROM pg_tviews_queue
WHERE processed_at IS NULL
  AND created_at < NOW() - INTERVAL '1 hour';
```

### Failed Item Handling

#### Step 1: Identify Failed Items
```sql
-- Find items that failed processing
SELECT
    entity_name,
    primary_key_value,
    priority,
    created_at,
    error_message,
    retry_count
FROM pg_tviews_queue
WHERE error_message IS NOT NULL
ORDER BY created_at DESC;
```

#### Step 2: Retry Failed Items (Manual)
```sql
-- For items that can be retried, trigger manual refresh
-- Replace 'your_entity' and 'your_key' with actual values
SELECT pg_tviews_refresh('your_entity', 'your_key');

-- Then remove from queue if successful
DELETE FROM pg_tviews_queue
WHERE entity_name = 'your_entity'
  AND primary_key_value = 'your_key'
  AND error_message IS NOT NULL;
```

#### Step 3: Remove Permanently Failed Items
```sql
-- For items that consistently fail (retry_count > 3)
DELETE FROM pg_tviews_queue
WHERE retry_count > 3
  AND error_message IS NOT NULL
  AND created_at < NOW() - INTERVAL '24 hours';
```

## Queue Performance Optimization

### Step 1: Analyze Processing Rates
```sql
-- Check processing throughput over time
SELECT
    DATE_TRUNC('hour', processed_at) as hour,
    COUNT(*) as items_processed,
    AVG(EXTRACT(EPOCH FROM (processed_at - created_at))) as avg_processing_time_seconds,
    MAX(EXTRACT(EPOCH FROM (processed_at - created_at))) as max_processing_time_seconds
FROM pg_tviews_queue
WHERE processed_at IS NOT NULL
  AND processed_at > NOW() - INTERVAL '24 hours'
GROUP BY DATE_TRUNC('hour', processed_at)
ORDER BY hour DESC;
```

### Step 2: Identify Bottlenecks
```sql
-- Find TVIEWs with slow processing
SELECT
    entity_name,
    COUNT(*) as total_processed,
    AVG(EXTRACT(EPOCH FROM (processed_at - created_at))) as avg_time_seconds,
    MAX(EXTRACT(EPOCH FROM (processed_at - created_at))) as max_time_seconds
FROM pg_tviews_queue
WHERE processed_at IS NOT NULL
  AND processed_at > NOW() - INTERVAL '7 days'
GROUP BY entity_name
HAVING AVG(EXTRACT(EPOCH FROM (processed_at - created_at))) > 30
ORDER BY avg_time_seconds DESC;
```

### Step 3: Queue Size Monitoring
```sql
-- Monitor queue growth trends
SELECT
    DATE_TRUNC('day', created_at) as day,
    COUNT(*) as items_created,
    COUNT(*) FILTER (WHERE processed_at IS NOT NULL) as items_processed,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as items_pending
FROM pg_tviews_queue
WHERE created_at > NOW() - INTERVAL '30 days'
GROUP BY DATE_TRUNC('day', created_at)
ORDER BY day DESC;
```

## Emergency Queue Operations

### Complete Queue Flush (Use with Caution)
```sql
-- WARNING: This will cancel all pending refreshes
-- Only use during maintenance windows or emergencies

-- Step 1: Check what will be affected
SELECT COUNT(*) as items_to_flush FROM pg_tviews_queue WHERE processed_at IS NULL;

-- Step 2: Flush queue (if confirmed)
DELETE FROM pg_tviews_queue WHERE processed_at IS NULL;

-- Step 3: Verify
SELECT COUNT(*) as remaining_items FROM pg_tviews_queue WHERE processed_at IS NULL;
```

### Queue Pause (For Maintenance)
```sql
-- Temporarily disable queue processing
-- Note: This is a conceptual operation - actual implementation depends on your setup

-- Check if you have a queue processing control mechanism
SELECT * FROM pg_settings WHERE name LIKE '%tview%' OR name LIKE '%queue%';

-- If available, pause processing:
-- ALTER SYSTEM SET pg_tviews.queue_processing = 'off';
-- SELECT pg_reload_conf();
```

## Monitoring and Alerting

### Recommended Alerts
- Queue size > 1000 items
- Items pending > 30 minutes
- Processing rate < 10 items/minute
- Error rate > 5%

### Automated Monitoring Script
```bash
# Run queue monitoring
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f docs/operations/runbooks/scripts/queue-status.sql
```

## Troubleshooting

### Queue Not Processing
```sql
-- Check if queue processor is running
SELECT * FROM pg_stat_activity WHERE query LIKE '%tview%' OR query LIKE '%queue%';

-- Check for blocking locks
SELECT * FROM pg_locks WHERE NOT granted;

-- Check system resources
SELECT * FROM pg_stat_bgwriter;
```

### High Queue Growth
```sql
-- Identify source of queue items
SELECT entity_name, COUNT(*) as queue_count
FROM pg_tviews_queue
WHERE processed_at IS NULL
GROUP BY entity_name
ORDER BY COUNT(*) DESC
LIMIT 10;

-- Check if specific TVIEWs are problematic
SELECT entity_name, last_error, last_refreshed
FROM pg_tviews_metadata
WHERE entity_name IN (
    SELECT entity_name FROM pg_tviews_queue
    WHERE processed_at IS NULL
    GROUP BY entity_name
    HAVING COUNT(*) > 100
);
```

### Performance Degradation
```sql
-- Check for table bloat affecting queue operations
SELECT schemaname, tablename, n_dead_tup, n_live_tup
FROM pg_stat_user_tables
WHERE tablename LIKE '%queue%'
   OR schemaname LIKE '%tview%';

-- Consider VACUUM if bloat > 20%
VACUUM ANALYZE pg_tviews_queue;
```

## Related Runbooks

- [TVIEW Health Check](tview-health-check.md) - Overall system health
- [Performance Monitoring](performance-monitoring.md) - Detailed performance analysis
- [Manual Refresh](../02-refresh-operations/manual-refresh.md) - Individual refresh operations
- [Emergency Procedures](../04-incident-response/emergency-procedures.md) - Crisis response

## Best Practices

1. **Monitor Regularly**: Check queue status daily
2. **Clean Up Weekly**: Remove stale items during maintenance windows
3. **Alert on Growth**: Set up monitoring for unusual queue growth
4. **Document Issues**: Track recurring queue problems and solutions
5. **Test Procedures**: Validate cleanup procedures in staging first</content>
<parameter name="filePath">docs/operations/runbooks/01-health-monitoring/queue-management.md