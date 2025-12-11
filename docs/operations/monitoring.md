# Monitoring Guide

Comprehensive monitoring setup for pg_tviews in production environments.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

This guide covers monitoring pg_tviews performance, health, and operational metrics. Proper monitoring ensures your TVIEWs stay performant and consistent with your base tables.

## Health Checks

### Basic Health Check

Implement a basic health check function:

```sql
-- Create health check function
CREATE OR REPLACE FUNCTION pg_tviews_health_check()
RETURNS TABLE(
    check_name text,
    status text,
    message text,
    details jsonb
) AS $$
DECLARE
    ext_version text;
    ivm_available boolean;
    trigger_count int;
    tview_count int;
BEGIN
    -- Check extension version
    SELECT pg_tviews_version() INTO ext_version;

    RETURN QUERY SELECT
        'extension_version'::text,
        'OK'::text,
        'pg_tviews extension loaded'::text,
        jsonb_build_object('version', ext_version);

    -- Check jsonb_ivm availability
    SELECT pg_tviews_check_jsonb_ivm() INTO ivm_available;

    RETURN QUERY SELECT
        'jsonb_ivm'::text,
        CASE WHEN ivm_available THEN 'OK' ELSE 'WARNING' END,
        CASE WHEN ivm_available
             THEN 'jsonb_ivm extension available'
             ELSE 'jsonb_ivm extension not available - reduced performance'
        END,
        jsonb_build_object('available', ivm_available);

    -- Check trigger count
    SELECT COUNT(*)::int INTO trigger_count
    FROM pg_trigger
    WHERE tgname LIKE 'tview%';

    RETURN QUERY SELECT
        'triggers'::text,
        CASE WHEN trigger_count > 0 THEN 'OK' ELSE 'ERROR' END,
        format('%s TVIEW triggers found', trigger_count),
        jsonb_build_object('count', trigger_count);

    -- Check TVIEW count
    SELECT COUNT(*)::int INTO tview_count
    FROM pg_tview_meta;

    RETURN QUERY SELECT
        'tviews'::text,
        CASE WHEN tview_count > 0 THEN 'OK' ELSE 'WARNING' END,
        format('%s TVIEWs registered', tview_count),
        jsonb_build_object('count', tview_count);

END;
$$ LANGUAGE plpgsql;
```

### Usage

```sql
-- Run health check
SELECT * FROM pg_tviews_health_check();

-- Check for issues
SELECT * FROM pg_tviews_health_check()
WHERE status IN ('ERROR', 'WARNING');
```

## Performance Metrics

### Queue Statistics

Monitor TVIEW refresh queue performance:

```sql
-- Get current queue statistics
SELECT pg_tviews_queue_stats();
```

Returns JSONB with:
```json
{
  "queue_size": 5,
  "total_refreshes": 23,
  "total_iterations": 2,
  "max_iterations": 3,
  "total_timing_ms": 45.2,
  "graph_cache_hit_rate": 0.85,
  "table_cache_hit_rate": 0.92,
  "graph_cache_hits": 12,
  "graph_cache_misses": 2,
  "table_cache_hits": 18,
  "table_cache_misses": 2
}
```

### Key Metrics to Monitor

```sql
-- Queue size (should be low)
SELECT (pg_tviews_queue_stats()->>'queue_size')::int as queue_size;

-- Refresh timing (should be fast)
SELECT (pg_tviews_queue_stats()->>'total_timing_ms')::float as total_timing_ms;

-- Cache hit rates (should be > 80%)
SELECT
    (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float as graph_hit_rate,
    (pg_tviews_queue_stats()->>'table_cache_hit_rate')::float as table_hit_rate;
```

### Performance Summary

Track historical performance:

```sql
-- Create performance tracking table
CREATE TABLE tview_performance_history (
    id BIGSERIAL PRIMARY KEY,
    collected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    queue_stats jsonb,
    active_tviews jsonb
);

-- Collect performance data
CREATE OR REPLACE FUNCTION collect_tview_performance()
RETURNS void AS $$
BEGIN
    INSERT INTO tview_performance_history (queue_stats, active_tviews)
    SELECT
        pg_tviews_queue_stats(),
        jsonb_object_agg(entity, jsonb_build_object(
            'table_size', pg_total_relation_size(table_oid),
            'last_refresh', now()
        ))
    FROM pg_tview_meta;
END;
$$ LANGUAGE plpgsql;
```

## Cache Monitoring

### Cache Hit Rates

Monitor caching effectiveness:

```sql
-- Graph cache performance
SELECT
    (pg_tviews_queue_stats()->>'graph_cache_hits')::int as hits,
    (pg_tviews_queue_stats()->>'graph_cache_misses')::int as misses,
    CASE
        WHEN (pg_tviews_queue_stats()->>'graph_cache_hits')::int +
             (pg_tviews_queue_stats()->>'graph_cache_misses')::int > 0
        THEN ROUND(
            (pg_tviews_queue_stats()->>'graph_cache_hits')::float /
            ((pg_tviews_queue_stats()->>'graph_cache_hits')::float +
             (pg_tviews_queue_stats()->>'graph_cache_misses')::float) * 100, 2
        )
        ELSE 0
    END as hit_rate_percentage;

-- Table cache performance
SELECT
    (pg_tviews_queue_stats()->>'table_cache_hits')::int as hits,
    (pg_tviews_queue_stats()->>'table_cache_misses')::int as misses,
    CASE
        WHEN (pg_tviews_queue_stats()->>'table_cache_hits')::int +
             (pg_tviews_queue_stats()->>'table_cache_misses')::int > 0
        THEN ROUND(
            (pg_tviews_queue_stats()->>'table_cache_hits')::float /
            ((pg_tviews_queue_stats()->>'table_cache_hits')::float +
             (pg_tviews_queue_stats()->>'table_cache_misses')::float) * 100, 2
        )
        ELSE 0
    END as hit_rate_percentage;
```

### Cache Size Monitoring

Track cache memory usage:

```sql
-- Estimate cache sizes
SELECT
    'graph_cache' as cache_type,
    COUNT(*) as entries,
    pg_size_pretty(SUM(octet_length(entity::text))) as estimated_size
FROM pg_tview_meta;

-- Monitor for cache bloat
SELECT
    schemaname,
    tablename,
    n_tup_ins, n_tup_upd, n_tup_del,
    n_live_tup, n_dead_tup,
    ROUND(n_dead_tup::float / GREATEST(n_live_tup, 1) * 100, 2) as bloat_ratio
FROM pg_stat_user_tables
WHERE schemaname = 'public' AND tablename LIKE 'tv_%'
ORDER BY bloat_ratio DESC;
```

## TVIEW Consistency Monitoring

### Count Consistency

Monitor TVIEW vs base table counts:

```sql
-- Check for count mismatches
CREATE OR REPLACE FUNCTION check_tview_consistency()
RETURNS TABLE(
    tview_name text,
    base_count bigint,
    tview_count bigint,
    difference bigint,
    status text
) AS $$
DECLARE
    rec record;
BEGIN
    FOR rec IN
        SELECT
            m.entity,
            m.table_oid,
            (SELECT COUNT(*) FROM pg_class c WHERE c.oid = m.table_oid) as tview_count,
            CASE
                WHEN m.entity = 'post' THEN (SELECT COUNT(*) FROM tb_post)
                WHEN m.entity = 'user' THEN (SELECT COUNT(*) FROM tb_user)
                WHEN m.entity = 'comment' THEN (SELECT COUNT(*) FROM tb_comment)
                ELSE 0
            END as base_count
        FROM pg_tview_meta m
    LOOP
        RETURN QUERY SELECT
            'tv_' || rec.entity,
            rec.base_count,
            rec.tview_count,
            rec.base_count - rec.tview_count,
            CASE
                WHEN rec.base_count = rec.tview_count THEN 'OK'
                WHEN ABS(rec.base_count - rec.tview_count) < (rec.base_count * 0.01) THEN 'WARNING'
                ELSE 'ERROR'
            END;
    END LOOP;
END;
$$ LANGUAGE plpgsql;
```

### Data Consistency

Spot check data consistency:

```sql
-- Random sampling of TVIEW consistency
CREATE OR REPLACE FUNCTION sample_tview_consistency(sample_size int DEFAULT 10)
RETURNS TABLE(
    entity text,
    id uuid,
    base_data jsonb,
    tview_data jsonb,
    matches boolean
) AS $$
DECLARE
    rec record;
BEGIN
    -- Sample posts
    FOR rec IN
        SELECT
            p.id,
            jsonb_build_object('title', p.title, 'content', p.content) as base_data,
            (SELECT data FROM tv_post WHERE id = p.id) as tview_data
        FROM tb_post p
        ORDER BY RANDOM()
        LIMIT sample_size
    LOOP
        RETURN QUERY SELECT
            'post'::text,
            rec.id,
            rec.base_data,
            rec.tview_data,
            rec.base_data = (rec.tview_data - 'id' - 'author') as matches;
    END LOOP;
END;
$$ LANGUAGE plpgsql;
```

## Alerting Setup

### Nagios/Icinga Checks

```bash
#!/bin/bash
# Nagios check for pg_tviews health

PSQL="psql -h $PGHOST -U $PGUSER -d $PGDATABASE -t -c"

# Check extension
VERSION=$($PSQL "SELECT pg_tviews_version()")
if [ $? -ne 0 ]; then
    echo "CRITICAL: Cannot connect to pg_tviews"
    exit 2
fi

# Check queue size
QUEUE_SIZE=$($PSQL "SELECT (pg_tviews_queue_stats()->>'queue_size')::int")
if [ $QUEUE_SIZE -gt 1000 ]; then
    echo "CRITICAL: TVIEW queue size $QUEUE_SIZE > 1000"
    exit 2
elif [ $QUEUE_SIZE -gt 100 ]; then
    echo "WARNING: TVIEW queue size $QUEUE_SIZE > 100"
    exit 1
fi

# Check cache hit rates
GRAPH_RATE=$($PSQL "SELECT (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float * 100")
if [ $(echo "$GRAPH_RATE < 80" | bc -l) -eq 1 ]; then
    echo "WARNING: Graph cache hit rate ${GRAPH_RATE}% < 80%"
    exit 1
fi

echo "OK: pg_tviews healthy - queue: $QUEUE_SIZE, cache: ${GRAPH_RATE}%"
exit 0
```

### Prometheus Metrics

```sql
-- Create Prometheus metrics endpoint
CREATE OR REPLACE FUNCTION prometheus_metrics()
RETURNS text AS $$
DECLARE
    result text := '';
    rec record;
BEGIN
    -- Queue metrics
    SELECT * INTO rec FROM pg_tviews_queue_stats();

    result := result || '# HELP pgtviews_queue_size Current refresh queue size' || E'\n';
    result := result || '# TYPE pgtviews_queue_size gauge' || E'\n';
    result := result || 'pgtviews_queue_size ' || (rec->>'queue_size') || E'\n';

    result := result || '# HELP pgtviews_refresh_timing_ms Total refresh timing in milliseconds' || E'\n';
    result := result || '# TYPE pgtviews_refresh_timing_ms gauge' || E'\n';
    result := result || 'pgtviews_refresh_timing_ms ' || (rec->>'total_timing_ms') || E'\n';

    result := result || '# HELP pgtviews_cache_hit_rate Cache hit rate (0.0-1.0)' || E'\n';
    result := result || '# TYPE pgtviews_cache_hit_rate gauge' || E'\n';
    result := result || 'pgtviews_cache_hit_rate{type="graph"} ' || (rec->>'graph_cache_hit_rate') || E'\n';
    result := result || 'pgtviews_cache_hit_rate{type="table"} ' || (rec->>'table_cache_hit_rate') || E'\n';

    -- TVIEW metrics
    FOR rec IN SELECT entity, table_oid FROM pg_tview_meta LOOP
        result := result || '# HELP pgtviews_tview_size_bytes TVIEW table size in bytes' || E'\n';
        result := result || '# TYPE pgtviews_tview_size_bytes gauge' || E'\n';
        result := result || 'pgtviews_tview_size_bytes{tview="' || rec.entity || '"} ' ||
                  (SELECT pg_total_relation_size(rec.table_oid)) || E'\n';
    END LOOP;

    RETURN result;
END;
$$ LANGUAGE plpgsql;
```

### Grafana Dashboards

Create Grafana dashboard with these panels:

1. **Queue Size**: Line graph of queue size over time
2. **Refresh Timing**: Average refresh time in milliseconds
3. **Cache Hit Rates**: Graph and table cache hit percentages
4. **TVIEW Sizes**: Table sizes for each TVIEW
5. **Consistency Checks**: Count differences between TVIEWs and base tables
6. **Error Rates**: Failed refresh operations

## Logging and Tracing

### PostgreSQL Log Configuration

```sql
-- Enable detailed logging
ALTER SYSTEM SET log_statement = 'ddl';
ALTER SYSTEM SET log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h ';
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- Log slow queries

-- Log TVIEW operations
ALTER SYSTEM SET log_min_messages = 'info';
```

### TVIEW Operation Logging

```sql
-- Create audit log table
CREATE TABLE tview_audit_log (
    id BIGSERIAL PRIMARY KEY,
    logged_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    operation text NOT NULL,
    entity text,
    pk_value bigint,
    timing_ms float,
    success boolean,
    error_message text
);

-- Log TVIEW operations (requires custom triggers or application logging)
CREATE OR REPLACE FUNCTION log_tview_operation(
    op text,
    ent text,
    pk_val bigint,
    timing float DEFAULT NULL,
    success bool DEFAULT true,
    error_msg text DEFAULT NULL
) RETURNS void AS $$
BEGIN
    INSERT INTO tview_audit_log (operation, entity, pk_value, timing_ms, success, error_message)
    VALUES (op, ent, pk_val, timing, success, error_msg);
END;
$$ LANGUAGE plpgsql;
```

### Application-Level Monitoring

```javascript
// Application monitoring example
const monitorTviewRefresh = async (operation, entity, pkValue) => {
  const startTime = Date.now();

  try {
    // Perform operation
    await performDatabaseOperation(operation, entity, pkValue);

    // Log success
    await logTviewOperation('refresh', entity, pkValue,
                          Date.now() - startTime, true);

  } catch (error) {
    // Log failure
    await logTviewOperation('refresh', entity, pkValue,
                          Date.now() - startTime, false, error.message);

    // Alert on failures
    alertSystem.sendAlert('TVIEW_REFRESH_FAILED', {
      entity, pkValue, error: error.message
    });
  }
};
```

## Operational Dashboards

### Real-time Dashboard

```sql
-- Real-time metrics view
CREATE OR REPLACE VIEW tview_realtime_metrics AS
SELECT
    now() as collected_at,
    (pg_tviews_queue_stats()->>'queue_size')::int as queue_size,
    (pg_tviews_queue_stats()->>'total_timing_ms')::float as total_timing_ms,
    (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float as graph_cache_hit_rate,
    (pg_tviews_queue_stats()->>'table_cache_hit_rate')::float as table_cache_hit_rate,
    (SELECT COUNT(*) FROM pg_tview_meta) as tview_count,
    (SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE 'tview%') as trigger_count
;
```

### Historical Trends

```sql
-- Daily summary view
CREATE OR REPLACE VIEW tview_daily_summary AS
SELECT
    DATE(collected_at) as date,
    AVG((queue_stats->>'queue_size')::int) as avg_queue_size,
    MAX((queue_stats->>'queue_size')::int) as max_queue_size,
    AVG((queue_stats->>'total_timing_ms')::float) as avg_timing_ms,
    MIN((queue_stats->>'graph_cache_hit_rate')::float) as min_graph_hit_rate,
    COUNT(*) as samples_count
FROM tview_performance_history
WHERE collected_at >= CURRENT_DATE - INTERVAL '30 days'
GROUP BY DATE(collected_at)
ORDER BY date DESC;
```

## Troubleshooting with Monitoring

### High Queue Size

**Symptoms**: Queue size > 100
**Investigation**:
```sql
-- Check what's in the queue
SELECT pg_tviews_debug_queue();

-- Check for slow operations
SELECT * FROM pg_stat_activity
WHERE query LIKE '%pg_tviews%' AND state = 'active';

-- Check cascade depth
SELECT COUNT(*) FROM pg_tview_meta;  -- Many TVIEWs = deep cascades
```

**Solutions**:
- Enable statement-level triggers
- Reduce cascade depth
- Optimize slow TVIEW definitions

### Low Cache Hit Rates

**Symptoms**: Cache hit rate < 80%
**Investigation**:
```sql
-- Check cache statistics
SELECT pg_tviews_queue_stats();

-- Check for cache invalidation
SELECT * FROM pg_tview_meta;  -- Many entities = more cache misses
```

**Solutions**:
- Increase shared_buffers
- Reduce number of entities
- Optimize TVIEW dependencies

### Performance Degradation

**Symptoms**: Slow refresh times
**Investigation**:
```sql
-- Check system resources
SELECT * FROM pg_stat_bgwriter;
SELECT * FROM pg_stat_database;

-- Check TVIEW sizes
SELECT schemaname, tablename, pg_total_relation_size(oid) as size
FROM pg_class
WHERE relname LIKE 'tv_%'
ORDER BY size DESC;
```

**Solutions**:
- Add indexes on TVIEWs
- Partition large TVIEWs
- Optimize TVIEW definitions

## Best Practices

### Monitoring Setup

1. **Automate Health Checks**: Run every 5 minutes
2. **Set Up Alerts**: Critical for queue size > 1000, timing > 5 seconds
3. **Monitor Trends**: Track performance over time
4. **Log Everything**: Enable comprehensive logging

### Alert Thresholds

- **Queue Size**: Warning > 100, Critical > 1000
- **Refresh Timing**: Warning > 1 second, Critical > 5 seconds
- **Cache Hit Rate**: Warning < 80%, Critical < 50%
- **Count Mismatch**: Any difference between TVIEW and base table

### Dashboard Organization

1. **Real-time Panel**: Current queue size, timing, hit rates
2. **Historical Trends**: 24-hour and 7-day views
3. **TVIEW Health**: Size, count consistency, error rates
4. **System Resources**: CPU, memory, disk I/O correlation

## See Also

- [Operator Guide](../user-guides/operators.md) - Production deployment
- [Troubleshooting Guide](troubleshooting.md) - Issue resolution
- [Performance Tuning](performance-tuning.md) - Optimization strategies