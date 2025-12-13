# Refresh Troubleshooting Runbook

## Purpose
Diagnose and resolve issues with TVIEW refresh operations, from slow performance to complete failures.

## When to Use
- **Refresh Failures**: When TVIEW refresh operations return errors
- **Slow Performance**: When refreshes take longer than expected
- **Data Inconsistencies**: When TVIEW data doesn't match source tables
- **Stuck Refreshes**: When refresh operations appear to hang
- **Queue Issues**: When refresh queue processing stops working

## Prerequisites
- PostgreSQL monitoring access (`pg_stat_*` views, logs)
- Database credentials with full access to TVIEW metadata
- System monitoring tools (CPU, memory, disk I/O)
- Access to PostgreSQL error logs
- Understanding of TVIEW refresh mechanics

## Initial Assessment (5 minutes)

### Step 1: Check TVIEW Status
```sql
-- Get comprehensive status for affected TVIEW
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    last_error,
    CASE
        WHEN last_error IS NOT NULL THEN 'ERROR'
        WHEN last_refresh_duration_ms > 30000 THEN 'SLOW'
        WHEN last_refreshed < NOW() - INTERVAL '1 hour' THEN 'STALE'
        ELSE 'HEALTHY'
    END as status
FROM pg_tviews_metadata
WHERE entity_name = 'your_problematic_tview';
```

### Step 2: Check Queue Status
```sql
-- Examine refresh queue for the TVIEW
SELECT
    COUNT(*) as queued_items,
    COUNT(*) FILTER (WHERE processed_at IS NULL) as pending_items,
    COUNT(*) FILTER (WHERE error_message IS NOT NULL) as failed_items,
    MIN(created_at) as oldest_pending,
    MAX(created_at) as newest_pending
FROM pg_tviews_queue
WHERE entity_name = 'your_problematic_tview';
```

### Step 3: System Resource Check
```sql
-- Check system resources during issue
SELECT
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT * FROM pg_stat_bgwriter) as bgwriter_stats,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as recent_io
FROM pg_stat_database
WHERE datname = current_database();
```

## Common Refresh Issues and Solutions

### Issue 1: "TVIEW not found" Error

**Symptoms**: `ERROR: TVIEW 'name' does not exist`

**Diagnosis**:
```sql
-- Check if TVIEW exists
SELECT entity_name, created_at
FROM pg_tviews_metadata
WHERE entity_name LIKE '%name%';

-- Check for typos in TVIEW name
SELECT entity_name
FROM pg_tviews_metadata
ORDER BY entity_name;
```

**Solutions**:
```sql
-- If TVIEW doesn't exist, create it
SELECT pg_tviews_convert_existing_table('correct_schema.correct_table');

-- If name is wrong, use correct name
SELECT pg_tviews_refresh('correct_tview_name');
```

### Issue 2: Permission Denied

**Symptoms**: `ERROR: permission denied for table tview_name`

**Diagnosis**:
```sql
-- Check current user permissions
SELECT current_user, session_user;

-- Check TVIEW ownership
SELECT schemaname, tablename, tableowner
FROM pg_tables
WHERE tablename LIKE '%tview_name%';
```

**Solutions**:
```sql
-- Grant necessary permissions
GRANT SELECT, UPDATE, DELETE ON tview_name TO your_user;
GRANT USAGE ON SCHEMA schema_name TO your_user;

-- Or switch to privileged user
SET ROLE privileged_user;
SELECT pg_tviews_refresh('tview_name');
```

### Issue 3: Lock Conflicts

**Symptoms**: `ERROR: canceling statement due to lock timeout`

**Diagnosis**:
```sql
-- Find blocking transactions
SELECT
    blocked.pid as blocked_pid,
    blocked.query as blocked_query,
    blocking.pid as blocking_pid,
    blocking.query as blocking_query,
    blocked.age as blocked_age
FROM (
    SELECT
        pid,
        query,
        EXTRACT(EPOCH FROM (NOW() - query_start)) as age
    FROM pg_stat_activity
    WHERE state = 'active'
) blocked
JOIN pg_locks blocked_locks ON blocked.pid = blocked_locks.pid
JOIN pg_locks blocking_locks ON blocked_locks.locktype = blocking_locks.locktype
    AND blocked_locks.database = blocking_locks.database
    AND blocked_locks.relation = blocking_locks.relation
    AND blocked_locks.page = blocking_locks.page
    AND blocked_locks.tuple = blocking_locks.tuple
    AND blocked_locks.virtualxid = blocking_locks.virtualxid
    AND blocked_locks.transactionid = blocking_locks.transactionid
    AND blocked_locks.classid = blocking_locks.classid
    AND blocked_locks.objid = blocking_locks.objid
    AND blocked_locks.objsubid = blocking_locks.objsubid
    AND blocked_locks.pid != blocking_locks.pid
    AND blocking_locks.granted
JOIN (
    SELECT pid, query
    FROM pg_stat_activity
    WHERE state = 'active'
) blocking ON blocking_locks.pid = blocking.pid;
```

**Solutions**:
```sql
-- Terminate blocking query (use with caution)
SELECT pg_cancel_backend(blocking_pid);

-- Or terminate entire session
SELECT pg_terminate_backend(blocking_pid);

-- Wait for blocking transaction to complete
-- Or reschedule refresh during maintenance window
```

### Issue 4: Out of Memory

**Symptoms**: `ERROR: out of memory` or extremely slow performance

**Diagnosis**:
```sql
-- Check memory settings
SELECT name, setting, unit
FROM pg_settings
WHERE name IN ('work_mem', 'maintenance_work_mem', 'shared_buffers');

-- Check TVIEW size
SELECT
    entity_name,
    pg_size_pretty(pg_total_relation_size(entity_name)) as size,
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema || '.' || table_name = m.entity_name) as columns
FROM pg_tviews_metadata m
WHERE entity_name = 'problematic_tview';
```

**Solutions**:
```sql
-- Increase memory settings temporarily
SET work_mem = '256MB';
SET maintenance_work_mem = '512MB';

-- Refresh in smaller chunks (if supported)
SELECT pg_tviews_refresh_chunked('tview_name', 1000);  -- Hypothetical function

-- Schedule during low-usage period
-- Consider TVIEW partitioning for large datasets
```

### Issue 5: Slow Refresh Performance

**Symptoms**: Refresh takes > 30 seconds for normal operations

**Diagnosis**:
```sql
-- Analyze refresh performance history
SELECT
    entity_name,
    last_refresh_duration_ms / 1000 as duration_seconds,
    last_refreshed,
    (SELECT AVG(last_refresh_duration_ms) / 1000
     FROM pg_tviews_metadata m2
     WHERE m2.entity_name = m1.entity_name
       AND m2.last_refreshed > NOW() - INTERVAL '7 days') as week_avg_seconds
FROM pg_tviews_metadata m1
WHERE entity_name = 'slow_tview';

-- Check for table bloat
SELECT
    schemaname, tablename,
    n_dead_tup, n_live_tup,
    ROUND(n_dead_tup::numeric / (n_live_tup + n_dead_tup) * 100, 2) as bloat_ratio
FROM pg_stat_user_tables
WHERE tablename LIKE '%tview%';

-- Check index usage
SELECT
    schemaname, tablename, indexname,
    idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
WHERE tablename LIKE '%tview%'
ORDER BY idx_scan DESC;
```

**Solutions**:
```sql
-- Reindex if necessary
REINDEX TABLE tview_name;

-- Vacuum to reduce bloat
VACUUM ANALYZE tview_name;

-- Check query plan for inefficiencies
EXPLAIN ANALYZE SELECT * FROM source_table WHERE id = 123;

-- Consider refresh optimization settings
ALTER TABLE tview_name SET (autovacuum_vacuum_scale_factor = 0.1);
```

## Advanced Troubleshooting

### Step 1: Enable Detailed Logging
```sql
-- Enable detailed logging for refresh operations
ALTER SYSTEM SET log_statement = 'ddl';
ALTER SYSTEM SET log_duration = on;
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- Log queries > 1s
SELECT pg_reload_conf();

-- Monitor logs during refresh attempt
tail -f /var/log/postgresql/postgresql.log | grep -i tview
```

### Step 2: Test with Minimal Data
```sql
-- Create test scenario with small dataset
CREATE TEMP TABLE test_source AS
SELECT * FROM source_table LIMIT 100;

-- Test refresh on small scale
SELECT pg_tviews_refresh('test_tview');

-- Compare performance
SELECT last_refresh_duration_ms
FROM pg_tviews_metadata
WHERE entity_name = 'test_tview';
```

### Step 3: Isolate Components
```sql
-- Test individual components of refresh process

-- 1. Test source table access
SELECT COUNT(*) FROM source_table LIMIT 1;

-- 2. Test TVIEW metadata
SELECT * FROM pg_tviews_metadata WHERE entity_name = 'problem_tview';

-- 3. Test queue operations
SELECT * FROM pg_tviews_queue WHERE entity_name = 'problem_tview' LIMIT 5;

-- 4. Test with different isolation levels
BEGIN ISOLATION LEVEL READ COMMITTED;
SELECT pg_tviews_refresh('tview_name');
COMMIT;
```

## Automated Diagnostics

### Create Diagnostic Script
```sql
-- Comprehensive diagnostic query
CREATE OR REPLACE FUNCTION pg_tviews_diagnose_refresh(tview_name TEXT)
RETURNS TABLE (
    check_name TEXT,
    status TEXT,
    details TEXT,
    recommendation TEXT
) AS $$
BEGIN
    -- TVIEW exists check
    IF NOT EXISTS (SELECT 1 FROM pg_tviews_metadata WHERE entity_name = tview_name) THEN
        RETURN QUERY SELECT
            'TVIEW exists'::TEXT,
            'FAIL'::TEXT,
            'TVIEW not found in metadata'::TEXT,
            'Verify TVIEW name or recreate TVIEW'::TEXT;
        RETURN;
    END IF;

    -- Permission check
    BEGIN
        EXECUTE 'SELECT 1 FROM ' || tview_name || ' LIMIT 1';
        RETURN QUERY SELECT
            'Permissions'::TEXT,
            'PASS'::TEXT,
            'Can access TVIEW'::TEXT,
            'No action needed'::TEXT;
    EXCEPTION WHEN insufficient_privilege THEN
        RETURN QUERY SELECT
            'Permissions'::TEXT,
            'FAIL'::TEXT,
            'Permission denied'::TEXT,
            'Grant SELECT permission on TVIEW'::TEXT;
    END;

    -- Add more diagnostic checks...
END;
$$ LANGUAGE plpgsql;
```

### Run Diagnostics
```sql
-- Use diagnostic function
SELECT * FROM pg_tviews_diagnose_refresh('your_tview_name');
```

## Prevention and Monitoring

### Proactive Monitoring
```sql
-- Set up alerts for common issues
CREATE OR REPLACE FUNCTION pg_tviews_monitor_health()
RETURNS TABLE (
    alert_level TEXT,
    issue TEXT,
    affected_tviews TEXT
) AS $$
BEGIN
    -- Check for stale TVIEWs
    RETURN QUERY
    SELECT
        'WARNING'::TEXT,
        'Stale TVIEWs detected'::TEXT,
        string_agg(entity_name, ', ')
    FROM pg_tviews_metadata
    WHERE last_refreshed < NOW() - INTERVAL '2 hours'
      AND last_error IS NULL;

    -- Check for refresh errors
    RETURN QUERY
    SELECT
        'ERROR'::TEXT,
        'TVIEWs with refresh errors'::TEXT,
        string_agg(entity_name, ', ')
    FROM pg_tviews_metadata
    WHERE last_error IS NOT NULL;

    -- Check queue backlog
    IF (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) > 100 THEN
        RETURN QUERY SELECT
            'WARNING'::TEXT,
            'Large queue backlog'::TEXT,
            (SELECT COUNT(*)::TEXT FROM pg_tviews_queue WHERE processed_at IS NULL);
    END IF;
END;
$$ LANGUAGE plpgsql;
```

### Regular Health Checks
```bash
# Add to cron for regular monitoring
*/15 * * * * psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT * FROM pg_tviews_monitor_health();"
```

## Related Runbooks

- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Overall system health
- [Manual Refresh](manual-refresh.md) - Individual refresh operations
- [Batch Refresh](batch-refresh.md) - Multiple TVIEW operations
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Performance analysis
- [Incident Checklist](../04-incident-response/incident-checklist.md) - Crisis response

## Best Practices

1. **Monitor Regularly**: Set up automated health checks
2. **Log Issues**: Document symptoms, diagnosis, and solutions
3. **Test Fixes**: Validate solutions in staging before production
4. **Escalate Early**: Don't spend hours on complex issues
5. **Document Workarounds**: Record temporary solutions for future reference
6. **Review Patterns**: Look for systemic issues requiring code changes</content>
<parameter name="filePath">docs/operations/runbooks/02-refresh-operations/refresh-troubleshooting.md