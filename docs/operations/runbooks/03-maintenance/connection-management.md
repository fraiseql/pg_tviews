# Connection Management Runbook

## Purpose
Monitor and manage database connections to ensure optimal performance and prevent connection pool exhaustion in pg_tviews environments.

## When to Use
- **Daily Monitoring**: Check connection usage patterns
- **Performance Issues**: When system response slows due to connection pressure
- **Connection Alerts**: When connection pool usage exceeds thresholds
- **Maintenance Windows**: Clean up stale connections before maintenance
- **Incident Response**: As part of diagnosing system slowdowns

## Prerequisites
- PostgreSQL monitoring access (`pg_stat_activity`)
- Connection pool configuration knowledge
- System monitoring tools (CPU, memory)
- Appropriate permissions to terminate connections if needed

## Daily Connection Monitoring (10 minutes)

### Step 1: Current Connection Overview
```sql
-- Get comprehensive connection status
SELECT
    COUNT(*) as total_connections,
    COUNT(*) FILTER (WHERE state = 'active') as active_connections,
    COUNT(*) FILTER (WHERE state = 'idle') as idle_connections,
    COUNT(*) FILTER (WHERE state = 'idle in transaction') as idle_in_transaction,
    COUNT(*) FILTER (WHERE state = 'waiting') as waiting_connections,
    MAX(EXTRACT(EPOCH FROM (NOW() - query_start))) FILTER (WHERE state = 'active') as longest_active_seconds,
    MAX(EXTRACT(EPOCH FROM (NOW() - query_start))) FILTER (WHERE state = 'idle in transaction') as longest_idle_tx_seconds
FROM pg_stat_activity;
```

### Step 2: TVIEW-Specific Connection Analysis
```sql
-- Analyze TVIEW-related connection usage
SELECT
    usename,
    client_addr,
    state,
    COUNT(*) as connection_count,
    MIN(query_start) as oldest_connection,
    MAX(EXTRACT(EPOCH FROM (NOW() - query_start))) as max_age_seconds,
    COUNT(*) FILTER (WHERE query LIKE '%tview%' OR query LIKE '%refresh%') as tview_connections
FROM pg_stat_activity
GROUP BY usename, client_addr, state
ORDER BY connection_count DESC, max_age_seconds DESC;
```

### Step 3: Connection Pool Health Check
```sql
-- Check connection pool utilization
SELECT
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections,
    (SELECT COUNT(*) FROM pg_stat_activity) as current_connections,
    ROUND(
        (SELECT COUNT(*) FROM pg_stat_activity)::numeric /
        (SELECT setting FROM pg_settings WHERE name = 'max_connections')::numeric * 100,
        2
    ) as utilization_percent,
    CASE
        WHEN (SELECT COUNT(*) FROM pg_stat_activity) > (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.8
        THEN 'CRITICAL'
        WHEN (SELECT COUNT(*) FROM pg_stat_activity) > (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.6
        THEN 'WARNING'
        ELSE 'HEALTHY'
    END as status;
```

## Connection Cleanup Procedures

### Step 1: Identify Problematic Connections
```sql
-- Find connections that may need cleanup
SELECT
    pid,
    usename,
    client_addr,
    state,
    EXTRACT(EPOCH FROM (NOW() - query_start)) as age_seconds,
    LEFT(query, 50) as query_preview
FROM pg_stat_activity
WHERE state IN ('idle', 'idle in transaction')
  AND query_start < NOW() - INTERVAL '30 minutes'
ORDER BY query_start ASC;
```

### Step 2: Safe Connection Termination
```sql
-- Terminate idle connections older than 1 hour (use with caution)
SELECT
    pid,
    usename,
    client_addr,
    pg_terminate_backend(pid) as terminated
FROM pg_stat_activity
WHERE state = 'idle'
  AND query_start < NOW() - INTERVAL '1 hour'
  AND pid != pg_backend_pid();  -- Don't terminate ourselves

-- Report termination results
SELECT
    'Connections terminated' as action,
    COUNT(*) as count
FROM pg_stat_activity
WHERE state = 'idle'
  AND query_start < NOW() - INTERVAL '1 hour';
```

### Step 3: Handle Long-Running Transactions
```sql
-- Identify and handle long-running transactions
SELECT
    pid,
    usename,
    client_addr,
    xact_start,
    EXTRACT(EPOCH FROM (NOW() - xact_start)) as transaction_age_seconds,
    LEFT(query, 50) as current_query
FROM pg_stat_activity
WHERE state = 'idle in transaction'
  AND xact_start < NOW() - INTERVAL '30 minutes'
ORDER BY xact_start ASC;

-- For transactions that can be safely terminated:
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle in transaction'
  AND xact_start < NOW() - INTERVAL '2 hours';
```

## Connection Pool Optimization

### Step 1: Analyze Connection Patterns
```sql
-- Analyze connection usage patterns over time
SELECT
    DATE_TRUNC('hour', query_start) as hour,
    COUNT(*) as connections_started,
    AVG(EXTRACT(EPOCH FROM (NOW() - query_start))) as avg_connection_lifetime
FROM pg_stat_activity
WHERE query_start > NOW() - INTERVAL '24 hours'
GROUP BY DATE_TRUNC('hour', query_start)
ORDER BY hour DESC;
```

### Step 2: Connection Pool Configuration Review
```sql
-- Review current connection settings
SELECT name, setting, unit, context
FROM pg_settings
WHERE name IN (
    'max_connections',
    'shared_preload_libraries',
    'pg_stat_statements.max',
    'pg_stat_statements.track'
)
ORDER BY name;

-- Check for connection pooler configuration (if using pgbouncer)
-- This would be external to PostgreSQL
```

### Step 3: Application Connection Tuning
```sql
-- Identify applications with high connection counts
SELECT
    usename,
    client_addr,
    COUNT(*) as connection_count,
    string_agg(DISTINCT state, ', ') as states
FROM pg_stat_activity
GROUP BY usename, client_addr
HAVING COUNT(*) > 5
ORDER BY COUNT(*) DESC;

-- Recommendations for application teams:
-- 1. Implement connection pooling
-- 2. Reduce connection lifetime
-- 3. Use prepared statements
-- 4. Implement connection retry logic
```

## TVIEW-Specific Connection Issues

### Step 1: Monitor TVIEW Refresh Connections
```sql
-- Check for TVIEW refresh connection usage
SELECT
    pid,
    usename,
    client_addr,
    EXTRACT(EPOCH FROM (NOW() - query_start)) as query_age_seconds,
    LEFT(query, 100) as query_preview
FROM pg_stat_activity
WHERE query LIKE '%tview%' OR query LIKE '%refresh%'
ORDER BY query_start ASC;
```

### Step 2: Connection Impact on TVIEW Performance
```sql
-- Check if connection pressure affects TVIEW operations
SELECT
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as avg_refresh_time_ms,
    CASE
        WHEN (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') > (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.7
        THEN 'HIGH_CONNECTION_PRESSURE'
        ELSE 'NORMAL'
    END as connection_pressure_status
FROM pg_stat_bgwriter;
```

### Step 3: TVIEW Connection Pool Recommendations
```sql
-- Specific recommendations for TVIEW applications
SELECT
    'TVIEW Connection Guidelines' as recommendation,
    CASE
        WHEN (SELECT COUNT(*) FROM pg_stat_activity) > 50 THEN 'Use connection pooler (pgbouncer)'
        WHEN (SELECT AVG(EXTRACT(EPOCH FROM (NOW() - query_start))) FROM pg_stat_activity WHERE query LIKE '%tview%') > 300 THEN 'Reduce TVIEW query timeout'
        WHEN (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) > 100 THEN 'Increase TVIEW worker threads'
        ELSE 'Connection usage is optimal'
    END as advice
FROM pg_stat_bgwriter;
```

## Emergency Connection Management

### Step 1: Critical Connection Assessment
```sql
-- Assess critical connection situation
SELECT
    'CRITICAL ASSESSMENT' as status,
    (SELECT COUNT(*) FROM pg_stat_activity) as total_connections,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'waiting') as waiting_connections,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_tview_refreshes
FROM pg_stat_bgwriter;
```

### Step 2: Emergency Connection Cleanup
```sql
-- Emergency: Terminate non-critical idle connections
SELECT
    pid,
    usename,
    pg_terminate_backend(pid) as terminated
FROM pg_stat_activity
WHERE state = 'idle'
  AND query_start < NOW() - INTERVAL '10 minutes'
  AND usename NOT IN ('postgres', 'pg_tviews_admin')  -- Preserve critical users
  AND pid != pg_backend_pid();

-- Emergency: Cancel long-running non-TVIEW queries
SELECT
    pid,
    query,
    pg_cancel_backend(pid) as cancelled
FROM pg_stat_activity
WHERE state = 'active'
  AND query_start < NOW() - INTERVAL '30 minutes'
  AND query NOT LIKE '%tview%'
  AND query NOT LIKE '%refresh%'
  AND pid != pg_backend_pid();
```

### Step 3: Post-Emergency Assessment
```sql
-- Verify emergency actions were effective
SELECT
    'POST-EMERGENCY STATUS' as status,
    (SELECT COUNT(*) FROM pg_stat_activity) as current_connections,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'waiting') as waiting_connections,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes
FROM pg_stat_bgwriter;
```

## Monitoring and Alerting

### Connection Health Alerts
```sql
-- Create connection monitoring function
CREATE OR REPLACE FUNCTION pg_tviews_connection_alerts()
RETURNS TABLE (
    alert_level TEXT,
    metric TEXT,
    current_value INTEGER,
    threshold INTEGER,
    recommendation TEXT
) AS $$
DECLARE
    max_conns INTEGER;
    current_conns INTEGER;
BEGIN
    SELECT setting::INTEGER INTO max_conns FROM pg_settings WHERE name = 'max_connections';
    SELECT COUNT(*) INTO current_conns FROM pg_stat_activity;

    -- Connection utilization alert
    IF current_conns > max_conns * 0.9 THEN
        RETURN QUERY SELECT 'CRITICAL'::TEXT, 'connection_utilization'::TEXT, current_conns, (max_conns * 0.9)::INTEGER, 'Reduce connection usage or increase max_connections'::TEXT;
    ELSIF current_conns > max_conns * 0.7 THEN
        RETURN QUERY SELECT 'WARNING'::TEXT, 'connection_utilization'::TEXT, current_conns, (max_conns * 0.7)::INTEGER, 'Monitor connection usage closely'::TEXT;
    END IF;

    -- Idle in transaction alert
    IF (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'idle in transaction') > 10 THEN
        RETURN QUERY SELECT 'WARNING'::TEXT, 'idle_in_transaction'::TEXT, (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'idle in transaction'), 10, 'Review application transaction handling'::TEXT;
    END IF;

    -- Long-running query alert
    IF (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active' AND query_start < NOW() - INTERVAL '30 minutes') > 0 THEN
        RETURN QUERY SELECT 'WARNING'::TEXT, 'long_running_queries'::TEXT, (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active' AND query_start < NOW() - INTERVAL '30 minutes'), 0, 'Investigate long-running queries'::TEXT;
    END IF;

    RETURN;
END;
$$ LANGUAGE plpgsql;
```

### Automated Monitoring
```bash
# Add to monitoring system
# Example cron job: */5 * * * * psql -c "SELECT * FROM pg_tviews_connection_alerts();"
```

## Troubleshooting

### Connection Pool Exhaustion
```sql
-- Symptoms: New connections fail, system slowdown
-- Diagnosis:
SELECT
    'CONNECTION_POOL_EXHAUSTION' as issue,
    (SELECT COUNT(*) FROM pg_stat_activity) as current_connections,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections
FROM pg_stat_bgwriter;

-- Solutions:
-- 1. Increase max_connections (requires restart)
-- 2. Implement connection pooling (pgbouncer)
-- 3. Reduce application connection usage
-- 4. Terminate idle connections
```

### Connection Leaks
```sql
-- Symptoms: Growing idle connection count
-- Diagnosis:
SELECT usename, client_addr, COUNT(*) as idle_count
FROM pg_stat_activity
WHERE state = 'idle'
GROUP BY usename, client_addr
HAVING COUNT(*) > 3
ORDER BY COUNT(*) DESC;

-- Solutions:
-- 1. Fix application connection handling
-- 2. Set appropriate connection timeouts
-- 3. Use connection pooler with leak detection
```

### TVIEW Connection Contention
```sql
-- Symptoms: TVIEW refreshes slow or fail
-- Diagnosis:
SELECT
    'TVIEW_CONNECTION_CONTENTION' as issue,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE query LIKE '%tview%') as tview_connections,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes
FROM pg_stat_bgwriter;

-- Solutions:
-- 1. Increase connection pool for TVIEW operations
-- 2. Implement TVIEW-specific connection pool
-- 3. Stagger TVIEW refresh schedules
```

## Related Runbooks

- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Overall system health
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Performance impact analysis
- [Regular Maintenance](regular-maintenance.md) - Connection cleanup procedures
- [Emergency Procedures](../04-incident-response/emergency-procedures.md) - Crisis connection management

## Best Practices

1. **Monitor Regularly**: Check connection usage daily
2. **Set Appropriate Limits**: Configure connection pools properly
3. **Implement Timeouts**: Set connection and query timeouts
4. **Use Connection Poolers**: pgbouncer or similar for high-traffic systems
5. **Monitor Application Behavior**: Track connection usage patterns by application
6. **Plan for Peak Usage**: Ensure capacity for peak loads
7. **Document Incidents**: Record connection-related issues and resolutions</content>
<parameter name="filePath">docs/operations/runbooks/03-maintenance/connection-management.md