# pg_tviews Debugging Guide

**Version**: 0.1.0-alpha
**Last Updated**: December 10, 2025

## Overview

This guide provides systematic troubleshooting procedures for common pg_tviews issues. Use the flowcharts and debugging steps to identify and resolve problems.

## Quick Diagnosis

### System Health Check

Start with a comprehensive health check:

```sql
-- Overall system status
SELECT * FROM pg_tviews_health_check();

-- Current queue status
SELECT * FROM pg_tviews_queue_realtime;

-- Recent performance
SELECT * FROM pg_tviews_performance_summary LIMIT 5;

-- Cache status
SELECT * FROM pg_tviews_cache_stats;
```

### Common Symptoms and Solutions

| Symptom | Likely Cause | Quick Fix |
|---------|-------------|-----------|
| TVIEW not refreshing | Missing triggers | `SELECT pg_tviews_install_stmt_triggers();` |
| Slow performance | No jsonb_ivm | `CREATE EXTENSION jsonb_ivm;` |
| Queue buildup | Long transactions | Check `pg_stat_activity` |
| Permission errors | Missing grants | Grant permissions on TVIEW tables |
| Memory errors | Large datasets | Increase `work_mem` |

## Troubleshooting Flowcharts

### TVIEW Not Refreshing

```
Start: Data changed in base table
    ↓
Does TVIEW show changes?
    ├─ YES → Problem solved
    └─ NO → Continue
        ↓
Check triggers installed?
SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE '%tview%';
    ├─ 0 triggers → Run: SELECT pg_tviews_install_stmt_triggers();
    └─ Triggers exist → Continue
        ↓
Check queue status
SELECT * FROM pg_tviews_queue_realtime;
    ├─ Queue empty → Manual refresh: SELECT pg_tviews_cascade(table_oid, pk);
    └─ Queue has items → Continue
        ↓
Check for stuck transactions
SELECT * FROM pg_stat_activity WHERE state = 'idle in transaction';
    ├─ Long-running tx → Kill or wait for completion
    └─ No stuck tx → Continue
        ↓
Check TVIEW permissions
SELECT * FROM information_schema.table_privileges WHERE table_name LIKE 'tv_%';
    ├─ Missing permissions → GRANT SELECT,UPDATE ON tv_* TO user;
    └─ Permissions OK → Continue
        ↓
Check TVIEW definition
SELECT * FROM pg_tview_meta WHERE entity = 'entity_name';
    ├─ Definition missing → Recreate TVIEW
    └─ Definition exists → Check PostgreSQL logs
```

### Performance Degradation

```
Start: Slow TVIEW operations
    ↓
Check cache hit rates
SELECT * FROM pg_tviews_cache_stats;
    ├─ Low hit rates → Check jsonb_ivm: SELECT pg_tviews_check_jsonb_ivm();
    │   ├─ FALSE → CREATE EXTENSION jsonb_ivm;
    │   └─ TRUE → Continue
    └─ Good hit rates → Continue
        ↓
Check queue size
SELECT * FROM pg_tviews_queue_realtime;
    ├─ Large queue → Check for bulk operations, consider statement triggers
    └─ Normal queue → Continue
        ↓
Check memory settings
SHOW work_mem; SHOW maintenance_work_mem;
    ├─ Too low → Increase: SET work_mem = '64MB';
    └─ Adequate → Continue
        ↓
Check index usage
EXPLAIN ANALYZE SELECT * FROM tv_table WHERE condition;
    ├─ Seq scan → Add indexes on JSONB fields
    └─ Index scan → Continue
        ↓
Check concurrent load
SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active';
    ├─ High concurrency → Check connection pool settings
    └─ Normal load → Check PostgreSQL logs for specific errors
```

### Queue Buildup Issues

```
Start: Queue size growing
    ↓
Check queue size trend
SELECT
    date_trunc('hour', recorded_at) as hour,
    AVG(queue_size) as avg_queue
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '24 hours'
GROUP BY 1 ORDER BY 1;
    ├─ Steady growth → Continue
    └─ Normal fluctuations → Monitor and alert if exceeds threshold
        ↓
Check transaction length
SELECT pid, xact_start, now() - xact_start as duration
FROM pg_stat_activity
WHERE state = 'active' AND xact_start IS NOT NULL
ORDER BY duration DESC;
    ├─ Long transactions → Optimize or break into smaller transactions
    └─ Normal tx length → Continue
        ↓
Check for deadlocks
Check PostgreSQL logs for deadlock messages
    ├─ Deadlocks found → Review transaction ordering
    └─ No deadlocks → Continue
        ↓
Check refresh performance
SELECT * FROM pg_tviews_performance_summary LIMIT 5;
    ├─ Slow refreshes → Investigate specific operations
    └─ Fast refreshes → Check for application issues
```

## Debugging Tools

### Queue Debugging

```sql
-- View current queue contents
SELECT * FROM pg_tviews_debug_queue();

-- Monitor queue in real-time (run in separate session)
SELECT pg_sleep(1);
SELECT * FROM pg_tviews_queue_realtime;

-- Check queue processing history
SELECT
    recorded_at,
    queue_size,
    refresh_count,
    timing_ms
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
ORDER BY recorded_at DESC;
```

### Trigger Debugging

```sql
-- Check installed triggers
SELECT
    tgname,
    tgrelid::regclass as table_name,
    tgenabled
FROM pg_trigger
WHERE tgname LIKE '%tview%'
ORDER BY table_name;

-- Test trigger manually
INSERT INTO base_table (id, data) VALUES (999, '{}');
SELECT * FROM tv_table WHERE pk_entity = 999;

-- Check trigger function exists
SELECT proname FROM pg_proc WHERE proname LIKE '%tview%trigger%';
```

### Performance Debugging

```sql
-- Slow query analysis
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM tv_table WHERE data->>'field' = 'value';

-- Cache performance analysis
SELECT
    'graph_cache' as cache,
    COUNT(*) as entries
FROM pg_tview_meta
UNION ALL
SELECT
    'prepared_statements',
    COUNT(*)
FROM pg_prepared_statements
WHERE name LIKE 'tview_refresh_%';

-- Memory usage analysis
SELECT
    name,
    setting,
    unit
FROM pg_settings
WHERE name IN ('work_mem', 'maintenance_work_mem', 'shared_buffers');
```

### Dependency Debugging

```sql
-- View TVIEW dependencies
SELECT
    entity,
    dependencies,
    dependency_types
FROM pg_tview_meta;

-- Check for circular dependencies
WITH RECURSIVE dep_chain AS (
    SELECT entity, entity as root, 0 as depth
    FROM pg_tview_meta
    UNION ALL
    SELECT m.entity, dc.root, dc.depth + 1
    FROM pg_tview_meta m
    JOIN dep_chain dc ON m.entity = ANY(dc.dependencies)
    WHERE dc.depth < 10
)
SELECT root, array_agg(entity ORDER BY depth) as chain
FROM dep_chain
GROUP BY root
HAVING COUNT(*) > 1;

-- Test dependency resolution
SELECT pg_tviews_analyze_select('SELECT ... FROM table1 JOIN table2 ...');
```

## Common Issues and Solutions

### Issue: TVIEW Creation Fails

**Symptoms**: `CREATE TVIEW` returns error

**Debug Steps**:
1. Check SQL syntax: `EXPLAIN SELECT ...;`
2. Verify required columns: `pk_<entity>`, `data`
3. Check table permissions: `\dp base_table`
4. Validate dependencies: `SELECT pg_tviews_analyze_select('SELECT ...');`

**Common Solutions**:
```sql
-- Fix missing primary key
CREATE TVIEW tv_posts AS
SELECT id as pk_post, jsonb_build_object('title', title) as data
FROM posts;

-- Fix permissions
GRANT SELECT ON posts TO pg_tviews_user;
GRANT ALL ON tv_posts TO pg_tviews_user;
```

### Issue: Automatic Refresh Not Working

**Symptoms**: Base table changes don't appear in TVIEW

**Debug Steps**:
1. Check triggers: `SELECT * FROM pg_trigger WHERE tgname LIKE '%tview%';`
2. Verify trigger function: `SELECT * FROM pg_proc WHERE proname LIKE '%trigger%';`
3. Test manual refresh: `SELECT pg_tviews_cascade('table'::regclass::oid, 123);`
4. Check queue: `SELECT * FROM pg_tviews_debug_queue();`

**Common Solutions**:
```sql
-- Reinstall triggers
SELECT pg_tviews_install_stmt_triggers();

-- Manual refresh stuck items
SELECT pg_tviews_cascade('table'::regclass::oid, pk_value)
FROM stuck_items;
```

### Issue: Performance Degradation

**Symptoms**: TVIEW queries getting slower

**Debug Steps**:
1. Check cache status: `SELECT * FROM pg_tviews_cache_stats;`
2. Monitor hit rates: `SELECT * FROM pg_tviews_performance_summary;`
3. Analyze query plans: `EXPLAIN ANALYZE SELECT * FROM tv_table;`
4. Check memory settings: `SHOW work_mem;`

**Common Solutions**:
```sql
-- Install jsonb_ivm for better performance
CREATE EXTENSION jsonb_ivm;

-- Add indexes on JSONB fields
CREATE INDEX idx_tv_posts_title ON tv_posts ((data->>'title'));
CREATE INDEX idx_tv_posts_author ON tv_posts ((data->'author'->>'id'));

-- Increase memory settings
SET work_mem = '128MB';
SET maintenance_work_mem = '512MB';
```

### Issue: Memory Issues

**Symptoms**: Out of memory errors, crashes

**Debug Steps**:
1. Check current memory: `SHOW work_mem; SHOW shared_buffers;`
2. Monitor memory usage: `SELECT * FROM pg_stat_activity;`
3. Check for large datasets: `SELECT COUNT(*) FROM tv_table;`
4. Review query patterns: `SELECT * FROM pg_stat_statements WHERE query LIKE '%tv_%';`

**Common Solutions**:
```sql
-- Increase memory settings
ALTER SYSTEM SET work_mem = '256MB';
ALTER SYSTEM SET maintenance_work_mem = '1GB';
SELECT pg_reload_conf();

-- Process large datasets in batches
CREATE TEMP TABLE batch_ids AS
SELECT pk_entity FROM tv_table LIMIT 1000 OFFSET 0;

UPDATE tv_table SET data = data || '{"processed": true}'
WHERE pk_entity IN (SELECT pk_entity FROM batch_ids);
```

### Issue: Connection Pool Issues

**Symptoms**: "queue state lost" errors, connection timeouts

**Debug Steps**:
1. Check pool configuration: `SHOW POOLS;` (PgBouncer)
2. Verify DISCARD ALL: `SHOW server_reset_query;`
3. Monitor connection usage: `SELECT * FROM pg_stat_activity;`
4. Check for long transactions: `SELECT xact_start, now() - xact_start FROM pg_stat_activity;`

**Common Solutions**:
```ini
# PgBouncer configuration
[pgbouncer]
pool_mode = transaction
server_reset_query = DISCARD ALL
max_client_conn = 1000
default_pool_size = 20
reserve_pool_size = 5
```

### Issue: Data Inconsistency

**Symptoms**: TVIEW shows stale or incorrect data

**Debug Steps**:
1. Compare base vs TVIEW: `SELECT COUNT(*) FROM base_table; SELECT COUNT(*) FROM tv_table;`
2. Check for failed refreshes: `SELECT * FROM pg_tviews_metrics WHERE refresh_count = 0;`
3. Verify data integrity: `SELECT pk_entity, data FROM tv_table WHERE data IS NULL;`
4. Check for constraint violations

**Common Solutions**:
```sql
-- Force full refresh
TRUNCATE tv_table;
INSERT INTO tv_table SELECT * FROM v_table;

-- Fix data inconsistencies
UPDATE tv_table SET data = (
    SELECT jsonb_build_object(...) FROM base_table WHERE id = pk_entity
) WHERE data->>'field' IS NULL;
```

## Advanced Debugging

### PostgreSQL Log Analysis

```bash
# Enable detailed logging
ALTER SYSTEM SET log_statement = 'ddl';
ALTER SYSTEM SET log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h ';
SELECT pg_reload_conf();

# Search for TVIEW-related errors
grep "pg_tviews\|tview\|TVIEW" /var/log/postgresql/postgresql-*.log

# Monitor trigger execution
grep "trigger" /var/log/postgresql/postgresql-*.log
```

### Performance Profiling

```sql
-- Create performance snapshot
CREATE TABLE perf_snapshot AS
SELECT
    now() as snapshot_time,
    (SELECT COUNT(*) FROM pg_tviews_queue_realtime) as queue_size,
    (SELECT * FROM pg_tviews_cache_stats) as cache_stats,
    (SELECT * FROM pg_stat_activity WHERE state = 'active') as active_sessions;

-- Compare snapshots over time
SELECT
    snapshot_time,
    queue_size,
    cache_stats,
    active_sessions
FROM perf_snapshot
ORDER BY snapshot_time DESC;
```

### Memory Leak Detection

```sql
-- Monitor memory usage
SELECT
    datname,
    usename,
    pid,
    memory_used,
    memory_allocated
FROM pg_stat_activity
WHERE memory_used > 100 * 1024 * 1024;  -- 100MB

-- Check for connection leaks
SELECT
    usename,
    count(*) as connection_count
FROM pg_stat_activity
GROUP BY usename
ORDER BY connection_count DESC;
```

### Lock Analysis

```sql
-- Check for blocking locks
SELECT
    blocked_locks.pid as blocked_pid,
    blocked_activity.usename as blocked_user,
    blocking_locks.pid as blocking_pid,
    blocking_activity.usename as blocking_user,
    blocked_activity.query as blocked_query
FROM pg_locks blocked_locks
JOIN pg_stat_activity blocked_activity ON blocked_activity.pid = blocked_locks.pid
JOIN pg_locks blocking_locks
    ON blocking_locks.locktype = blocked_locks.locktype
    AND blocking_locks.database IS NOT DISTINCT FROM blocked_locks.database
    AND blocking_locks.relation IS NOT DISTINCT FROM blocked_locks.relation
    AND blocking_locks.page IS NOT DISTINCT FROM blocked_locks.page
    AND blocking_locks.tuple IS NOT DISTINCT FROM blocked_locks.tuple
    AND blocking_locks.virtualxid IS NOT DISTINCT FROM blocked_locks.virtualxid
    AND blocking_locks.transactionid IS NOT DISTINCT FROM blocked_locks.transactionid
    AND blocking_locks.classid IS NOT DISTINCT FROM blocked_locks.classid
    AND blocking_locks.objid IS NOT DISTINCT FROM blocked_locks.objid
    AND blocking_locks.objsubid IS NOT DISTINCT FROM blocked_locks.objsubid
    AND blocking_locks.pid != blocked_locks.pid
JOIN pg_stat_activity blocking_activity ON blocking_activity.pid = blocking_locks.pid
WHERE NOT blocked_locks.granted;
```

## Emergency Procedures

### Complete TVIEW Reset

```sql
-- Emergency: Reset all TVIEWs
BEGIN;

-- Drop all TVIEWs
SELECT pg_tviews_drop(entity, true) FROM pg_tview_meta;

-- Clear metadata
TRUNCATE pg_tview_meta;

-- Clear metrics
TRUNCATE pg_tviews_metrics;

-- Recreate TVIEWs from backup definitions
-- (Restore from DDL backup)

COMMIT;
```

### System Recovery

```sql
-- Recover from corrupted state
BEGIN;

-- Disconnect all users
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = current_database()
  AND pid != pg_backend_pid();

-- Drop and recreate extension
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

-- Restore from backup
-- (Follow backup restore procedures)

COMMIT;
```

### Data Repair

```sql
-- Repair inconsistent TVIEW data
CREATE TEMP TABLE repair_log (pk_entity bigint, old_data jsonb, new_data jsonb);

INSERT INTO repair_log
SELECT
    t.pk_entity,
    t.data as old_data,
    (SELECT jsonb_build_object(...) FROM base_table b WHERE b.id = t.pk_entity) as new_data
FROM tv_table t
WHERE t.data->>'field' != (SELECT value FROM base_table WHERE id = t.pk_entity);

-- Apply repairs
UPDATE tv_table SET data = new_data
FROM repair_log
WHERE tv_table.pk_entity = repair_log.pk_entity;
```

## Best Practices

### Proactive Monitoring

```sql
-- Daily health check script
#!/bin/bash
psql -d mydb -c "
    SELECT status, check_name, details
    FROM pg_tviews_health_check()
    WHERE status != 'OK'
" > health_check.log

if [ -s health_check.log ]; then
    mail -s 'pg_tviews Health Check Failed' admin@example.com < health_check.log
fi
```

### Alert Configuration

```sql
-- Create alerting view
CREATE VIEW alerting_conditions AS
SELECT
    'queue_size' as metric,
    queue_size as value,
    100 as threshold,
    CASE WHEN queue_size > 100 THEN 'CRITICAL' ELSE 'OK' END as status
FROM pg_tviews_queue_realtime

UNION ALL

SELECT
    'timing' as metric,
    timing_ms as value,
    500 as threshold,
    CASE WHEN timing_ms > 500 THEN 'WARNING' ELSE 'OK' END as status
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '5 minutes';
```

### Documentation Maintenance

```sql
-- Document your TVIEWs
CREATE TABLE tview_documentation (
    entity text PRIMARY KEY,
    description text,
    dependencies text[],
    refresh_frequency text,
    business_owner text,
    last_reviewed date
);

-- Keep documentation current
UPDATE tview_documentation SET
    last_reviewed = CURRENT_DATE,
    dependencies = (SELECT array_agg(entity) FROM pg_tview_meta WHERE entity != tview_documentation.entity)
WHERE entity IN (SELECT entity FROM pg_tview_meta);
```

## See Also

- [Error Reference](ERROR_REFERENCE.md) - Complete error documentation
- [API Reference](API_REFERENCE.md) - Function documentation
- [Monitoring Guide](MONITORING.md) - Health checking and metrics
- [Operations Guide](OPERATIONS.md) - Production procedures