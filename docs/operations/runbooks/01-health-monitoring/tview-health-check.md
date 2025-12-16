# TVIEW Health Check Runbook

## Purpose
Regular health check to ensure all TVIEWs are synchronized, operational, and performing within expected parameters.

## When to Use
- **Routine Monitoring**: Every 4 hours during business hours
- **After Major Changes**: Database maintenance, schema changes, bulk data updates
- **User Reports**: When users report synchronization issues or slow queries
- **Pre-Deployments**: Before deploying application changes that affect TVIEWs
- **Incident Investigation**: As part of diagnosing performance issues

## Prerequisites
- PostgreSQL CLI access (`psql`)
- Database credentials with SELECT permissions on TVIEW metadata tables
- Access to system monitoring (optional but recommended)

## Quick Health Check (2 minutes)

Run this procedure for routine monitoring:

### Step 1: Connect to Database
```bash
# Connect to your database
psql -h $DB_HOST -U $DB_USER -d $DB_NAME
```

### Step 2: Check TVIEW Status
```sql
-- Check 1: All TVIEWs are defined and accessible
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as recently_refreshed,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as with_errors
FROM pg_tviews_metadata;

-- Expected: total_tviews > 0, with_errors = 0, recently_refreshed should be reasonable
```

### Step 3: Check Queue Health
```sql
-- Check 2: Queue status (should be minimal during normal operation)
SELECT
    COUNT(*) as queued_items,
    MAX(created_at) as oldest_item,
    COUNT(*) FILTER (WHERE created_at < NOW() - INTERVAL '1 hour') as stale_items
FROM pg_tviews_queue;

-- Expected: queued_items low, oldest_item recent, stale_items = 0
```

### Step 4: Performance Check
```sql
-- Check 3: Recent refresh performance
SELECT
    entity_name,
    last_refresh_duration_ms,
    last_refreshed,
    CASE
        WHEN last_refresh_duration_ms > 5000 THEN 'SLOW'
        WHEN last_refresh_duration_ms > 1000 THEN 'OK'
        ELSE 'FAST'
    END as performance_status
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '24 hours'
ORDER BY last_refresh_duration_ms DESC
LIMIT 5;

-- Expected: No 'SLOW' entries, reasonable performance distribution
```

## Comprehensive Health Check (10 minutes)

Use this for detailed investigation or after issues are detected:

### Step 1: TVIEW Inventory
```sql
-- Complete TVIEW inventory with status
SELECT
    entity_name,
    primary_key_column,
    created_at,
    last_refreshed,
    last_refresh_duration_ms,
    CASE
        WHEN last_error IS NOT NULL THEN 'ERROR'
        WHEN last_refreshed < NOW() - INTERVAL '1 hour' THEN 'STALE'
        WHEN last_refresh_duration_ms > 5000 THEN 'SLOW'
        ELSE 'HEALTHY'
    END as health_status,
    last_error
FROM pg_tviews_metadata
ORDER BY
    CASE
        WHEN last_error IS NOT NULL THEN 1
        WHEN last_refreshed < NOW() - INTERVAL '1 hour' THEN 2
        WHEN last_refresh_duration_ms > 5000 THEN 3
        ELSE 4
    END,
    last_refreshed DESC;
```

### Step 2: Queue Analysis
```sql
-- Detailed queue analysis
SELECT
    COUNT(*) as total_queued,
    COUNT(*) FILTER (WHERE priority = 'high') as high_priority,
    COUNT(*) FILTER (WHERE priority = 'normal') as normal_priority,
    COUNT(*) FILTER (WHERE priority = 'low') as low_priority,
    MIN(created_at) as oldest_item,
    MAX(created_at) as newest_item,
    AVG(EXTRACT(EPOCH FROM (NOW() - created_at))) as avg_age_seconds
FROM pg_tviews_queue;

-- Check for stuck items
SELECT
    entity_name,
    primary_key_value,
    priority,
    created_at,
    NOW() - created_at as age
FROM pg_tviews_queue
WHERE created_at < NOW() - INTERVAL '30 minutes'
ORDER BY created_at ASC;
```

### Step 3: Error Analysis
```sql
-- Recent errors (last 24 hours)
SELECT
    entity_name,
    last_error,
    last_refreshed,
    error_count_24h
FROM (
    SELECT
        entity_name,
        last_error,
        last_refreshed,
        COUNT(*) FILTER (WHERE last_error IS NOT NULL AND last_refreshed > NOW() - INTERVAL '24 hours') as error_count_24h
    FROM pg_tviews_metadata
    WHERE last_error IS NOT NULL
) t
WHERE error_count_24h > 0
ORDER BY error_count_24h DESC;
```

### Step 4: Performance Trends
```sql
-- Performance trends (last 7 days)
SELECT
    DATE_TRUNC('day', last_refreshed) as day,
    COUNT(*) as refreshes,
    AVG(last_refresh_duration_ms) as avg_duration_ms,
    MAX(last_refresh_duration_ms) as max_duration_ms,
    COUNT(*) FILTER (WHERE last_refresh_duration_ms > 5000) as slow_refreshes
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '7 days'
GROUP BY DATE_TRUNC('day', last_refreshed)
ORDER BY day DESC;
```

### Step 5: System Resource Check
```sql
-- Check system resources (if monitoring tables exist)
SELECT
    schemaname,
    tablename,
    n_tup_ins,
    n_tup_upd,
    n_tup_del,
    n_live_tup,
    n_dead_tup,
    last_vacuum,
    last_autovacuum,
    last_analyze,
    last_autoanalyze
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY n_dead_tup DESC;
```

## Automated Health Check Script

Use the provided script for consistent monitoring:

```bash
# Run automated health check
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f docs/operations/runbooks/scripts/health-check.sql
```

## Expected Results

### Healthy System
- ✅ All TVIEWs show `HEALTHY` status
- ✅ Queue has minimal items (< 10)
- ✅ No stale queue items (> 30 minutes)
- ✅ Refresh durations < 5 seconds typically
- ✅ No recent errors
- ✅ Performance trends stable

### Warning Signs
- ⚠️ TVIEWs with `STALE` status (> 1 hour since refresh)
- ⚠️ Queue growing steadily
- ⚠️ Increasing refresh durations
- ⚠️ Occasional errors (< 5% of refreshes)

### Critical Issues
- ❌ TVIEWs with `ERROR` status
- ❌ Queue items > 1 hour old
- ❌ Refresh failures > 10%
- ❌ Performance degradation > 50%
- ❌ System resource exhaustion

## Troubleshooting

### TVIEW Shows STALE Status
```sql
-- Check if refresh is queued
SELECT * FROM pg_tviews_queue WHERE entity_name = 'your_tview_name';

-- Manual refresh if needed
SELECT pg_tviews_refresh('your_tview_name');
```

### Queue Growing
```sql
-- Check for blocking transactions
SELECT * FROM pg_stat_activity WHERE state = 'idle in transaction';

-- Check for long-running refreshes
SELECT entity_name, last_refresh_duration_ms, last_refreshed
FROM pg_tviews_metadata
WHERE last_refresh_duration_ms > 10000
ORDER BY last_refresh_duration_ms DESC;
```

### Performance Degradation
```sql
-- Check system load
SELECT * FROM pg_stat_bgwriter;
SELECT * FROM pg_stat_database WHERE datname = current_database();

-- Check for table bloat
SELECT schemaname, tablename, n_dead_tup, n_live_tup
FROM pg_stat_user_tables
WHERE n_dead_tup > n_live_tup * 0.2;
```

## Escalation

If health checks reveal issues:

1. **Minor Issues**: Document and monitor for trends
2. **Performance Issues**: Check system resources, consider maintenance
3. **Queue Issues**: Investigate blocking transactions, consider manual cleanup
4. **Critical Errors**: Follow [Incident Checklist](../04-incident-response/incident-checklist.md)

## Related Runbooks

- [Queue Management](queue-management.md) - For queue-specific issues
- [Performance Monitoring](performance-monitoring.md) - For detailed performance analysis
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - For refresh-specific issues</content>
<parameter name="filePath">docs/operations/runbooks/01-health-monitoring/tview-health-check.md