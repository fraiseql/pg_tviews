# pg_tviews Monitoring Guide

**Version**: 0.1.0-alpha
**Last Updated**: December 10, 2025

## Overview

This guide covers monitoring, metrics, and health checking for pg_tviews in production environments. pg_tviews provides comprehensive monitoring infrastructure to ensure system health and performance.

## Quick Start

```sql
-- Check system health
SELECT * FROM pg_tviews_health_check();

-- View real-time queue activity
SELECT * FROM pg_tviews_queue_realtime;

-- Check cache performance
SELECT * FROM pg_tviews_cache_stats;

-- View performance trends
SELECT * FROM pg_tviews_performance_summary;
```

## Monitoring Views

### pg_tviews_queue_realtime

**Purpose**: View current refresh queue state per active session and transaction.

**Columns**:
- `session` (TEXT): Application name / session identifier
- `transaction_id` (BIGINT): Current transaction ID
- `queue_size` (INT): Number of pending refreshes
- `entities` (TEXT[]): Array of affected entity names
- `last_enqueued` (TIMESTAMPTZ): Most recent enqueue timestamp

**Example**:
```sql
-- View active refresh queues
SELECT * FROM pg_tviews_queue_realtime;

-- Results:
   session   | transaction_id | queue_size |    entities    |      last_enqueued
-------------+----------------+------------+----------------+-------------------------
  myapp      |      123456    |     15     | {post,comment} | 2025-12-10 10:30:45+00
```

**Use Cases**:
- Monitor queue buildup during bulk operations
- Identify which sessions have pending refreshes
- Debug transaction isolation issues

**Performance**: Minimal overhead, safe for frequent polling.

**Notes**:
- Only shows transactions with queued operations
- Queue cleared on COMMIT/ABORT
- Empty result means no pending operations

### pg_tviews_cache_stats

**Purpose**: Cache hit/miss statistics for performance tuning.

**Columns**:
- `cache_type` (TEXT): Type of cache ('graph_cache', 'table_cache', 'prepared_statements')
- `entries` (BIGINT): Number of entries in cache
- `estimated_size` (TEXT): Estimated memory usage

**Example**:
```sql
SELECT * FROM pg_tviews_cache_stats;

-- Results:
     cache_type     | entries | estimated_size
--------------------+---------+----------------
  graph_cache       |      15 | 8192 bytes
  table_cache       |       8 | 512 bytes
  prepared_statements |     23 | 23 kB
```

**Use Cases**:
- Monitor cache growth and memory usage
- Identify cache inefficiencies
- Tune cache-related performance

**Performance**: Fast query, safe for monitoring dashboards.

**Notes**:
- Graph cache: One entry per TVIEW definition
- Table cache: One entry per unique table OID
- Prepared statements: Cached refresh queries

### pg_tviews_performance_summary

**Purpose**: Hourly aggregated performance metrics.

**Columns**:
- `hour` (TIMESTAMPTZ): Hour bucket (truncated)
- `transactions` (BIGINT): Number of transactions processed
- `avg_queue_size` (FLOAT): Average queue size per transaction
- `avg_refresh_count` (FLOAT): Average refreshes per transaction
- `avg_iterations` (FLOAT): Average cascade iterations
- `avg_timing_ms` (FLOAT): Average refresh timing in milliseconds
- `total_bulk_refreshes` (BIGINT): Total bulk refresh operations
- `total_individual_refreshes` (BIGINT): Total individual refresh operations
- `graph_cache_hit_rate` (FLOAT): Graph cache hit rate (0.0-1.0)
- `table_cache_hit_rate` (FLOAT): Table cache hit rate (0.0-1.0)
- `prepared_stmt_hit_rate` (FLOAT): Prepared statement hit rate (0.0-1.0)

**Example**:
```sql
SELECT * FROM pg_tviews_performance_summary LIMIT 5;

-- Results:
        hour        | transactions | avg_queue_size | avg_refresh_count | avg_iterations | avg_timing_ms | total_bulk_refreshes | total_individual_refreshes | graph_cache_hit_rate | table_cache_hit_rate | prepared_stmt_hit_rate
--------------------+--------------+----------------+-------------------+---------------+---------------+----------------------+---------------------------+----------------------+----------------------+------------------------
 2025-12-10 10:00:00 |           45 |           2.3 |              8.7 |          1.2 |         15.3 |                   23 |                       122 |                 0.85 |                 0.92 |                   0.78
```

**Use Cases**:
- Trend analysis over time
- Performance regression detection
- Capacity planning

**Performance**: Aggregates last 24 hours, moderate query cost.

**Notes**:
- Data retained for 24 hours by default
- Hourly aggregation reduces storage needs
- Cache hit rates are critical for performance

### pg_tviews_statement_stats

**Purpose**: Integration with pg_stat_statements for TVIEW-related queries.

**Columns**:
- `query` (TEXT): SQL query text
- `calls` (BIGINT): Number of times executed
- `total_time` (FLOAT): Total execution time in milliseconds
- `mean_time` (FLOAT): Mean execution time per call
- `stddev_time` (FLOAT): Standard deviation of execution time
- `rows_affected` (BIGINT): Total rows affected

**Example**:
```sql
SELECT * FROM pg_tviews_statement_stats LIMIT 3;

-- Results:
                                                                 query                                                                 | calls | total_time | mean_time | stddev_time | rows_affected
----------------------------------------------------------------------------------------------------------------------------------------+-------+------------+-----------+-------------+---------------
  SELECT pg_tviews_version()                                                                                                           |    12 |      0.123 |    0.010 |      0.005 |             0
  SELECT * FROM tv_post WHERE data->>'author_id' = $1                                                                                |   456 |    123.456 |    0.271 |      0.089 |           234
  UPDATE tv_post SET data = jsonb_set(data, '{title}', $1) WHERE pk_post = $2                                                        |    89 |     45.678 |    0.513 |      0.234 |            89
```

**Use Cases**:
- Identify slow TVIEW queries
- Monitor query performance patterns
- Optimize frequently executed operations

**Performance**: Depends on pg_stat_statements configuration.

**Notes**:
- Requires pg_stat_statements extension
- Only shows queries containing 'pg_tview' or 'tv_'
- Useful for application performance analysis

## Monitoring Functions

### pg_tviews_health_check()

**Signature**:
```sql
pg_tviews_health_check()
RETURNS TABLE (check_name TEXT, status TEXT, details TEXT)
```

**Description**: Comprehensive health check for pg_tviews system components.

**Returns**:
- `check_name`: Name of the check performed
- `status`: 'OK', 'WARNING', 'ERROR', or 'INFO'
- `details`: Human-readable details about the check

**Example**:
```sql
SELECT * FROM pg_tviews_health_check();

     check_name        | status |              details
-----------------------+--------+-----------------------------------
 extension_installed   | OK     | pg_tviews extension is installed
 metadata_tables       | OK     | 2/2 metadata tables exist
 statement_triggers    | OK     | 5 statement-level triggers installed
 cache_status          | INFO   | Graph cache: 15 entries
```

**Health Checks Performed**:
1. **Extension Installation**: Verifies pg_tviews is installed
2. **Metadata Tables**: Checks pg_tview_meta and pg_tview_helpers exist
3. **Statement Triggers**: Counts installed statement-level triggers
4. **Cache Status**: Reports graph cache size

**Recommended Usage**:
```sql
-- In monitoring system, alert on any ERROR status:
SELECT * FROM pg_tviews_health_check()
WHERE status = 'ERROR';

-- Daily health check report:
SELECT
    check_name,
    status,
    details,
    NOW() as checked_at
FROM pg_tviews_health_check()
ORDER BY
    CASE status
        WHEN 'ERROR' THEN 1
        WHEN 'WARNING' THEN 2
        ELSE 3
    END;
```

**Performance**: Fast check (~10ms), safe for frequent execution.

### pg_tviews_record_metrics()

**Signature**:
```sql
pg_tviews_record_metrics(
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
) RETURNS void
```

**Description**: Records performance metrics for a completed refresh operation. Called automatically by the refresh engine.

**Parameters**:
- `p_transaction_id`: PostgreSQL transaction ID
- `p_queue_size`: Number of operations in refresh queue
- `p_refresh_count`: Total refreshes performed
- `p_iteration_count`: Cascade iteration count
- `p_timing_ms`: Total timing in milliseconds
- `p_graph_cache_hit`: Whether graph cache was hit
- `p_table_cache_hits`: Table cache hits
- `p_prepared_stmt_cache_hits`: Prepared statement cache hits
- `p_prepared_stmt_cache_misses`: Prepared statement cache misses
- `p_bulk_refresh_count`: Number of bulk refresh operations
- `p_individual_refresh_count`: Number of individual refresh operations

**Example**:
```sql
-- Manual metric recording (normally automatic)
SELECT pg_tviews_record_metrics(
    123456, 5, 8, 2, 15.3, true, 5, 7, 1, 2, 6
);
```

**Notes**:
- Called automatically during refresh operations
- Manual calls generally not needed
- Useful for testing or custom monitoring

### pg_tviews_cleanup_metrics()

**Signature**:
```sql
pg_tviews_cleanup_metrics(days_old INT DEFAULT 30) RETURNS INTEGER
```

**Description**: Removes old metrics data for retention management.

**Parameters**:
- `days_old` (INT, optional): Delete metrics older than this many days (default: 30)

**Returns**:
- `INTEGER`: Number of rows deleted

**Example**:
```sql
-- Delete metrics older than 7 days
SELECT pg_tviews_cleanup_metrics(7);
-- Returns: 1250

-- Delete all metrics (for testing)
SELECT pg_tviews_cleanup_metrics(0);
-- Returns: 5678
```

**Recommended Usage**:
```sql
-- Monthly cleanup (keep 30 days)
SELECT pg_tviews_cleanup_metrics(30);

-- Automated cleanup via cron:
-- 0 2 1 * * psql -c "SELECT pg_tviews_cleanup_metrics(30);" mydb
```

**Performance**: Fast operation, minimal impact.

### pg_tviews_debug_queue()

**Signature**:
```sql
pg_tviews_debug_queue()
RETURNS TABLE (entity TEXT, pk BIGINT, enqueued_at TIMESTAMPTZ)
```

**Description**: Returns current contents of the refresh queue for debugging purposes.

**Returns**:
- `entity` (TEXT): Entity name being refreshed
- `pk` (BIGINT): Primary key value
- `enqueued_at` (TIMESTAMPTZ): When the operation was enqueued

**Example**:
```sql
SELECT * FROM pg_tviews_debug_queue();

-- Results:
 entity | pk  |      enqueued_at
--------+-----+-------------------------
 post   | 123 | 2025-12-10 10:30:45+00
 user   | 456 | 2025-12-10 10:30:46+00
```

**Notes**:
- Thread-local state (safe for concurrent connections)
- Useful for debugging refresh cascades
- Cross-referenced in API Reference

## Metrics Collection

### Automatic Collection

pg_tviews automatically records performance metrics to the `pg_tviews_metrics` table during refresh operations.

**Metrics Table Schema**:
```sql
CREATE TABLE pg_tviews_metrics (
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
```

**What Gets Recorded**:
- Queue size at commit time
- Number of refreshes performed
- Iteration count for cascading updates
- Timing in milliseconds
- Cache hit/miss statistics
- Bulk vs individual refresh operations

### Manual Metric Recording

For custom monitoring:
```sql
SELECT pg_tviews_record_metrics(
    p_transaction_id := txid_current(),
    p_queue_size := 10,
    p_refresh_count := 8,
    p_iteration_count := 2,
    p_timing_ms := 15.3,
    p_graph_cache_hit := true,
    p_table_cache_hits := 5,
    p_prepared_stmt_cache_hits := 7,
    p_prepared_stmt_cache_misses := 1,
    p_bulk_refresh_count := 2,
    p_individual_refresh_count := 6
);
```

### Metrics Retention

**Default Retention**: Unlimited (grows unbounded)

**Recommended Practice**: Clean old metrics monthly
```sql
-- Delete metrics older than 30 days
SELECT pg_tviews_cleanup_metrics(30);

-- Returns: Number of rows deleted
```

**Automated Cleanup** (add to cron or pg_cron):
```sql
-- Run daily
SELECT pg_tviews_cleanup_metrics(30);
```

### Query Historical Metrics

```sql
-- Average timing over last 24 hours
SELECT
    date_trunc('hour', recorded_at) as hour,
    AVG(timing_ms) as avg_timing_ms,
    MAX(timing_ms) as max_timing_ms,
    AVG(queue_size) as avg_queue_size
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '24 hours'
GROUP BY 1
ORDER BY 1;

-- Cache hit rates
SELECT
    COUNT(*) as total_operations,
    SUM(CASE WHEN graph_cache_hit THEN 1 ELSE 0 END)::FLOAT / COUNT(*) * 100
        as graph_cache_hit_rate,
    AVG(prepared_stmt_cache_hits::FLOAT /
        NULLIF(prepared_stmt_cache_hits + prepared_stmt_cache_misses, 0) * 100)
        as prepared_stmt_hit_rate
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '24 hours';
```

## Recommended Alerts

### Critical Alerts 🔴

**1. Health Check Failures**
```sql
-- Alert if any health check fails
SELECT * FROM pg_tviews_health_check()
WHERE status = 'ERROR';
```
**Threshold**: Any ERROR status
**Action**: Immediate investigation required

**2. Excessive Queue Size**
```sql
-- Alert if queue exceeds threshold
SELECT * FROM pg_tviews_queue_realtime
WHERE queue_size > 1000;
```
**Threshold**: queue_size > 1000
**Action**: Check for stuck transactions or bulk operation issues

**3. Slow Refresh Performance**
```sql
-- Alert if average timing exceeds threshold
SELECT AVG(timing_ms) as avg_ms
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '5 minutes'
HAVING AVG(timing_ms) > 500;
```
**Threshold**: avg_timing_ms > 500ms
**Action**: Investigate query performance, check for table bloat

### Warning Alerts 🟡

**4. Low Cache Hit Rates**
```sql
-- Alert if cache performance degrades
SELECT
    cache_type,
    entries
FROM pg_tviews_cache_stats
WHERE cache_type = 'graph_cache'
  AND entries < 10;  -- Suspiciously low
```
**Threshold**: Graph cache < 10 entries (expected: 50-100+)
**Action**: Check if cache invalidation is too aggressive

**5. High Iteration Counts**
```sql
-- Alert if cascades iterate excessively
SELECT
    transaction_id,
    iteration_count
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
  AND iteration_count > 5;
```
**Threshold**: iteration_count > 5
**Action**: Possible dependency cycle or deep cascade chain

**6. Metrics Table Growth**
```sql
-- Alert if metrics table grows too large
SELECT
    pg_size_pretty(pg_relation_size('pg_tviews_metrics')) as size,
    COUNT(*) as row_count
FROM pg_tviews_metrics
HAVING COUNT(*) > 1000000;  -- 1M rows
```
**Threshold**: > 1M rows or >100MB
**Action**: Run pg_tviews_cleanup_metrics()

### Monitoring Query Examples

**Grafana/Prometheus Integration**:
```sql
-- Export metrics for time-series database
SELECT
    EXTRACT(EPOCH FROM recorded_at) as timestamp,
    timing_ms,
    queue_size,
    refresh_count,
    CASE WHEN graph_cache_hit THEN 1 ELSE 0 END as cache_hit
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
ORDER BY recorded_at;
```

**Nagios/Icinga Check**:
```bash
#!/bin/bash
# Check pg_tviews health
result=$(psql -tAc "
    SELECT COUNT(*)
    FROM pg_tviews_health_check()
    WHERE status = 'ERROR'
")

if [ "$result" -gt 0 ]; then
    echo "CRITICAL: pg_tviews health check failed"
    exit 2
fi

echo "OK: pg_tviews healthy"
exit 0
```

## Performance Analysis

### Identifying Bottlenecks

**Slow Refresh Operations**:
```sql
-- Find slowest refresh operations
SELECT
    recorded_at,
    transaction_id,
    timing_ms,
    queue_size,
    refresh_count,
    iteration_count
FROM pg_tviews_metrics
WHERE timing_ms > 100  -- Adjust threshold
ORDER BY timing_ms DESC
LIMIT 10;
```

**Cache Performance Issues**:
```sql
-- Check cache hit rates over time
SELECT
    date_trunc('hour', recorded_at) as hour,
    AVG(CASE WHEN graph_cache_hit THEN 1.0 ELSE 0.0 END) as graph_hit_rate,
    AVG(prepared_stmt_cache_hits::float /
        NULLIF(prepared_stmt_cache_hits + prepared_stmt_cache_misses, 0)) as stmt_hit_rate
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '24 hours'
GROUP BY 1
ORDER BY 1;
```

**Queue Buildup Analysis**:
```sql
-- Analyze queue patterns
SELECT
    date_trunc('hour', recorded_at) as hour,
    AVG(queue_size) as avg_queue,
    MAX(queue_size) as max_queue,
    COUNT(*) as transactions
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '24 hours'
GROUP BY 1
ORDER BY 1;
```

### Optimization Strategies

**1. Statement-Level Triggers**
For bulk operations, use statement-level triggers:
```sql
SELECT pg_tviews_install_stmt_triggers();
```
Benefits: 100-500× performance improvement for bulk operations.

**2. Cache Optimization**
Monitor and optimize cache hit rates:
- Graph cache hit rate should be > 80%
- Prepared statement hit rate should be > 70%
- If low, check for excessive cache invalidation

**3. Query Optimization**
Use `pg_tviews_statement_stats` to identify slow queries:
```sql
SELECT query, mean_time, calls
FROM pg_tviews_statement_stats
WHERE mean_time > 10  -- Adjust threshold
ORDER BY mean_time DESC;
```

## Troubleshooting

### Common Issues

**Health Check Shows Errors**:
- **Extension not installed**: Run `CREATE EXTENSION pg_tviews;`
- **Missing metadata tables**: Check for `pg_tview_meta` and `pg_tview_helpers`
- **No statement triggers**: Run `SELECT pg_tviews_install_stmt_triggers();`

**Performance Degradation**:
- Check cache hit rates with `pg_tviews_cache_stats`
- Monitor queue sizes with `pg_tviews_queue_realtime`
- Review slow operations in `pg_tviews_metrics`

**Metrics Table Growing Too Large**:
```sql
-- Check table size
SELECT pg_size_pretty(pg_relation_size('pg_tviews_metrics'));

-- Clean up old data
SELECT pg_tviews_cleanup_metrics(7);  -- Keep 7 days
```

### Debug Information

**Current Queue State**:
```sql
SELECT * FROM pg_tviews_debug_queue();
SELECT * FROM pg_tviews_queue_realtime();
```

**System Status**:
```sql
SELECT * FROM pg_tviews_health_check();
SELECT * FROM pg_tviews_cache_stats();
```

**Recent Performance**:
```sql
SELECT * FROM pg_tviews_performance_summary LIMIT 5;
```

## See Also

- [API Reference](API_REFERENCE.md)
- [Debugging Guide](DEBUGGING.md)
- [Operations Guide](OPERATIONS.md)