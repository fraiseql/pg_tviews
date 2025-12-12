# Performance Tuning Guide

Advanced performance optimization strategies for pg_tviews in production.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

This guide covers advanced performance tuning for pg_tviews. While pg_tviews provides excellent out-of-the-box performance, proper tuning can achieve even better results for high-throughput applications.

## Baseline Performance

### Default Performance Characteristics

- **Single row updates**: 0.5-2ms
- **Bulk operations**: 10-100ms for 100-1000 rows
- **Cache hit rates**: 85-95% after warm-up
- **Memory usage**: 50-200MB depending on dataset

### Performance Monitoring

Establish performance baselines:

```sql
-- Create performance baseline table
CREATE TABLE tview_performance_baseline (
    id BIGSERIAL PRIMARY KEY,
    test_name text NOT NULL,
    start_time timestamptz NOT NULL,
    end_time timestamptz NOT NULL,
    operations_count int NOT NULL,
    avg_latency_ms float,
    p95_latency_ms float,
    p99_latency_ms float,
    throughput_ops_sec float,
    queue_stats jsonb,
    notes text
);

-- Performance testing function
CREATE OR REPLACE FUNCTION benchmark_tview_operation(
    test_name text,
    operation_type text,
    iterations int DEFAULT 100
) RETURNS jsonb AS $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    latencies float[] := '{}';
    i int;
    op_start timestamptz;
    op_end timestamptz;
    latency_ms float;
BEGIN
    start_time := clock_timestamp();

    FOR i IN 1..iterations LOOP
        op_start := clock_timestamp();

        -- Execute operation based on type
        CASE operation_type
            WHEN 'single_insert' THEN
                INSERT INTO tb_post (id, title, fk_user)
                VALUES (gen_random_uuid(), 'Test Post ' || i, 1);
            WHEN 'bulk_insert' THEN
                -- Bulk operation
                INSERT INTO tb_post (id, title, fk_user)
                SELECT gen_random_uuid(), 'Test Post ' || (i*10 + j), 1
                FROM generate_series(1, 10) j;
            WHEN 'single_update' THEN
                UPDATE tb_post SET title = 'Updated ' || i
                WHERE pk_post = (SELECT pk_post FROM tb_post LIMIT 1 OFFSET i);
        END CASE;

        op_end := clock_timestamp();
        latency_ms := EXTRACT(epoch FROM (op_end - op_start)) * 1000;
        latencies := latencies || latency_ms;
    END LOOP;

    end_time := clock_timestamp();

    -- Calculate statistics
    INSERT INTO tview_performance_baseline (
        test_name, start_time, end_time, operations_count,
        avg_latency_ms, p95_latency_ms, p99_latency_ms,
        throughput_ops_sec, queue_stats
    ) VALUES (
        test_name, start_time, end_time, iterations,
        (SELECT avg(x) FROM unnest(latencies) x),
        (SELECT percentile_cont(0.95) WITHIN GROUP (ORDER BY x) FROM unnest(latencies) x),
        (SELECT percentile_cont(0.99) WITHIN GROUP (ORDER BY x) FROM unnest(latencies) x),
        iterations / EXTRACT(epoch FROM (end_time - start_time)),
        pg_tviews_queue_stats()
    );

    RETURN jsonb_build_object(
        'test_name', test_name,
        'iterations', iterations,
        'avg_latency_ms', (SELECT avg(x) FROM unnest(latencies) x),
        'p95_latency_ms', (SELECT percentile_cont(0.95) WITHIN GROUP (ORDER BY x) FROM unnest(latencies) x),
        'throughput', iterations / EXTRACT(epoch FROM (end_time - start_time))
    );
END;
$$ LANGUAGE plpgsql;
```

## System Configuration Tuning

### Memory Configuration

Optimize PostgreSQL memory settings for TVIEW workloads:

```sql
-- Shared buffers (25% of RAM, max 8GB)
ALTER SYSTEM SET shared_buffers = '2GB';

-- Work memory (per connection, 2-4MB typical)
ALTER SYSTEM SET work_mem = '4MB';

-- Maintenance work memory (for index builds, etc.)
ALTER SYSTEM SET maintenance_work_mem = '512MB';

-- WAL buffers
ALTER SYSTEM SET wal_buffers = '16MB';

-- Effective cache size (helps query planner)
ALTER SYSTEM SET effective_cache_size = '6GB';
```

### CPU Configuration

Optimize for concurrent workloads:

```sql
-- Connection settings
ALTER SYSTEM SET max_connections = 200;

-- Worker processes
ALTER SYSTEM SET max_worker_processes = 8;
ALTER SYSTEM SET max_parallel_workers_per_gather = 4;
ALTER SYSTEM SET max_parallel_workers = 8;

-- Background writer
ALTER SYSTEM SET bgwriter_delay = '20ms';
ALTER SYSTEM SET bgwriter_lru_maxpages = 400;
```

### Storage Configuration

Optimize disk I/O for TVIEW workloads:

```sql
-- Checkpoint tuning
ALTER SYSTEM SET checkpoint_segments = 32;
ALTER SYSTEM SET checkpoint_completion_target = 0.9;

-- Autovacuum tuning for TVIEWs
ALTER SYSTEM SET autovacuum_max_workers = 4;
ALTER SYSTEM SET autovacuum_naptime = '10s';

-- WAL tuning
ALTER SYSTEM SET wal_level = replica;
ALTER SYSTEM SET wal_compression = on;
```

## TVIEW-Specific Optimizations

### Statement-Level Triggers

Enable for maximum bulk performance:

```sql
-- Install statement-level triggers
SELECT pg_tviews_install_stmt_triggers();

-- Performance impact:
-- - Single operations: Same performance (~1-2ms)
-- - Bulk operations: 100-500× faster (10ms vs 5 seconds for 1000 rows)
-- - Memory usage: Slightly higher
-- - Compatibility: PostgreSQL 13+
```

### Index Optimization

Strategic indexing for TVIEW query patterns:

```sql
-- Primary lookup indexes
CREATE UNIQUE INDEX CONCURRENTLY idx_tv_post_id ON tv_post(id);
CREATE INDEX CONCURRENTLY idx_tv_post_user_id ON tv_post(user_id);

-- JSONB field indexes for common queries
CREATE INDEX CONCURRENTLY idx_tv_post_created_at ON tv_post USING gin((data->'createdAt'));
CREATE INDEX CONCURRENTLY idx_tv_post_title ON tv_post USING gin((data->'title'));
CREATE INDEX CONCURRENTLY idx_tv_post_tags ON tv_post USING gin((data->'tags'));

-- Composite indexes for complex queries
CREATE INDEX CONCURRENTLY idx_tv_post_user_created ON tv_post(user_id, (data->>'createdAt'));
CREATE INDEX CONCURRENTLY idx_tv_post_category_status ON tv_post((data->'category'->>'id'), (data->>'status'));

-- Partial indexes for active data
CREATE INDEX CONCURRENTLY idx_tv_post_active ON tv_post(id) WHERE (data->>'status') = 'active';

-- Full-text search indexes
CREATE INDEX CONCURRENTLY idx_tv_post_content_fts ON tv_post USING gin(to_tsvector('english', data->>'content'));
```

### Partitioning Strategies

Partition large TVIEWs for better performance:

```sql
-- Time-based partitioning
CREATE TABLE tv_post_y2024 PARTITION OF tv_post
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

CREATE TABLE tv_post_y2025 PARTITION OF tv_post
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

-- Hash partitioning for large user bases
CREATE TABLE tv_post_0 PARTITION OF tv_post
    FOR VALUES WITH (MODULUS 4, REMAINDER 0);
CREATE TABLE tv_post_1 PARTITION OF tv_post
    FOR VALUES WITH (MODULUS 4, REMAINDER 1);
-- etc.

-- Update TVIEW to include partition key
CREATE OR REPLACE FUNCTION post_partition_key(uuid)
RETURNS int AS $$
    SELECT abs(hashtext($1::text)) % 4;
$$ LANGUAGE sql IMMUTABLE;

CREATE TABLE tv_post AS
SELECT
    pk_post,
    id,
    post_partition_key(id) as partition_key,
    jsonb_build_object(...) as data
FROM tb_post;
```

## Query Optimization

### TVIEW Definition Optimization

Optimize TVIEW SELECT statements for performance:

```sql
-- ✅ Efficient: Pre-compute aggregations
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'author', jsonb_build_object('id', u.id, 'name', u.name),
        'commentCount', COALESCE(comment_counts.count, 0),
        'avgRating', COALESCE(rating_stats.avg_rating, 0)
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
LEFT JOIN (
    SELECT fk_post, COUNT(*) as count
    FROM tb_comment
    GROUP BY fk_post
) comment_counts ON p.pk_post = comment_counts.fk_post
LEFT JOIN (
    SELECT fk_post, AVG(rating) as avg_rating
    FROM tb_review
    GROUP BY fk_post
) rating_stats ON p.pk_post = rating_stats.fk_post;

-- ❌ Inefficient: Expensive operations in TVIEW
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'expensiveField', (SELECT expensive_function(p.pk_post))  -- Slow!
    ) as data
FROM tb_post p;
```

### Connection Pool Optimization

Configure connection pools for TVIEW workloads:

```ini
# PgBouncer configuration
[pgbouncer]
pool_mode = transaction
max_client_conn = 10000
default_pool_size = 50
reserve_pool_size = 10
reserve_pool_timeout = 5
max_db_connections = 100
max_user_connections = 1000
server_reset_query = DISCARD ALL

# Connection settings
server_idle_timeout = 30
server_lifetime = 3600
client_idle_timeout = 300
```

## Advanced Caching Strategies

### Multi-Level Caching

Implement application-level caching on top of TVIEWs:

```javascript
// Redis caching layer
class TviewCache {
  constructor(redis, db) {
    this.redis = redis;
    this.db = db;
  }

  async getPost(id) {
    // Check Redis first
    const cached = await this.redis.get(`post:${id}`);
    if (cached) {
      return JSON.parse(cached);
    }

    // Fallback to TVIEW
    const result = await this.db.query(
      'SELECT data FROM tv_post WHERE id = $1',
      [id]
    );

    if (result.rows[0]) {
      // Cache for 5 minutes
      await this.redis.setex(`post:${id}`, 300, JSON.stringify(result.rows[0].data));
      return result.rows[0].data;
    }
  }

  async invalidatePost(id) {
    // Invalidate cache on updates
    await this.redis.del(`post:${id}`);
    // TVIEW automatically updates
  }
}
```

### Cache Warming

Pre-populate caches for frequently accessed data:

```sql
-- Cache warming query
CREATE OR REPLACE FUNCTION warm_tview_cache()
RETURNS void AS $$
DECLARE
    rec record;
BEGIN
    -- Warm up frequently accessed posts
    FOR rec IN
        SELECT id FROM tv_post
        WHERE (data->>'viewCount')::int > 1000
        ORDER BY (data->>'lastViewed')::timestamptz DESC
        LIMIT 1000
    LOOP
        -- Touch each record to warm caches
        PERFORM pg_tviews_cascade('tv_post'::regclass::oid,
                                (SELECT pk_post FROM tv_post WHERE id = rec.id));
    END LOOP;
END;
$$ LANGUAGE plpgsql;
```

## Load Testing

### Load Test Framework

Create comprehensive load testing:

```sql
-- Load testing function
CREATE OR REPLACE FUNCTION load_test_tviews(
    concurrent_users int DEFAULT 10,
    operations_per_user int DEFAULT 100,
    operation_type text DEFAULT 'mixed'
) RETURNS jsonb AS $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    total_operations int;
    results jsonb;
BEGIN
    start_time := clock_timestamp();
    total_operations := concurrent_users * operations_per_user;

    -- Create test data
    INSERT INTO tb_user (id, name)
    SELECT gen_random_uuid(), 'User ' || i
    FROM generate_series(1, concurrent_users) i;

    -- Run concurrent load test
    -- (Implementation would use pg_background or external tool)

    end_time := clock_timestamp();

    results := jsonb_build_object(
        'concurrent_users', concurrent_users,
        'operations_per_user', operations_per_user,
        'total_operations', total_operations,
        'duration_seconds', EXTRACT(epoch FROM (end_time - start_time)),
        'throughput_ops_sec', total_operations / EXTRACT(epoch FROM (end_time - start_time)),
        'final_queue_stats', pg_tviews_queue_stats()
    );

    RETURN results;
END;
$$ LANGUAGE plpgsql;
```

### Load Test Scenarios

Define different load patterns:

```sql
-- Read-heavy workload
-- 90% SELECT, 10% INSERT/UPDATE

-- Write-heavy workload
-- 50% INSERT, 40% UPDATE, 10% SELECT

-- Mixed workload
-- 60% SELECT, 30% UPDATE, 10% INSERT

-- Bulk operations
-- Large batch inserts/updates

-- High concurrency
-- Many simultaneous users
```

## Monitoring and Alerting

### Performance Metrics Dashboard

Create comprehensive monitoring:

```sql
-- Performance metrics view
CREATE OR REPLACE VIEW tview_performance_metrics AS
SELECT
    now() as collected_at,
    (pg_tviews_queue_stats()->>'queue_size')::int as queue_size,
    (pg_tviews_queue_stats()->>'total_timing_ms')::float as total_timing_ms,
    (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float as graph_cache_hit_rate,
    (pg_tviews_queue_stats()->>'table_cache_hit_rate')::float as table_cache_hit_rate,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE query LIKE '%pg_tviews%' AND state = 'active') as active_refreshes,
    (SELECT SUM(pg_total_relation_size(oid)) FROM pg_class WHERE relname LIKE 'tv_%') as total_tview_size_bytes
;

-- Alert thresholds
CREATE OR REPLACE FUNCTION check_performance_alerts()
RETURNS TABLE(alert_level text, metric text, value float, threshold float, message text) AS $$
DECLARE
    queue_size int;
    timing_ms float;
    cache_rate float;
BEGIN
    SELECT
        (pg_tviews_queue_stats()->>'queue_size')::int,
        (pg_tviews_queue_stats()->>'total_timing_ms')::float,
        (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float
    INTO queue_size, timing_ms, cache_rate;

    -- Queue size alerts
    IF queue_size > 1000 THEN
        RETURN QUERY SELECT 'CRITICAL'::text, 'queue_size'::text, queue_size::float, 1000::float,
                          'TVIEW queue size critically high'::text;
    ELSIF queue_size > 100 THEN
        RETURN QUERY SELECT 'WARNING'::text, 'queue_size'::text, queue_size::float, 100::float,
                          'TVIEW queue size elevated'::text;
    END IF;

    -- Timing alerts
    IF timing_ms > 5000 THEN
        RETURN QUERY SELECT 'CRITICAL'::text, 'timing_ms'::text, timing_ms, 5000::float,
                          'TVIEW refresh timing critically slow'::text;
    ELSIF timing_ms > 1000 THEN
        RETURN QUERY SELECT 'WARNING'::text, 'timing_ms'::text, timing_ms, 1000::float,
                          'TVIEW refresh timing slow'::text;
    END IF;

    -- Cache alerts
    IF cache_rate < 0.5 THEN
        RETURN QUERY SELECT 'CRITICAL'::text, 'cache_hit_rate'::text, cache_rate, 0.5::float,
                          'TVIEW cache hit rate critically low'::text;
    ELSIF cache_rate < 0.8 THEN
        RETURN QUERY SELECT 'WARNING'::text, 'cache_hit_rate'::text, cache_rate, 0.8::float,
                          'TVIEW cache hit rate low'::text;
    END IF;
END;
$$ LANGUAGE plpgsql;
```

## Scaling Strategies

### Horizontal Scaling

Scale read workloads with read replicas:

```
Primary Database (Write)
├── tb_* tables (writes)
├── TVIEW triggers (automatic refresh)
└── Write-heavy operations

Read Replicas (Read)
├── tv_* tables (reads only)
├── Automatic replication
└── Read-heavy GraphQL queries
```

### Vertical Scaling

Optimize single-server performance:

```sql
-- Increase resources
ALTER SYSTEM SET shared_buffers = '8GB';
ALTER SYSTEM SET work_mem = '8MB';
ALTER SYSTEM SET maintenance_work_mem = '1GB';

-- SSD storage for TVIEWs
-- RAID 10 for redundancy
-- Connection pooling
```

### Application-Level Sharding

Shard at application level for extreme scale:

```sql
-- Shard by user ID ranges
CREATE TABLE tv_post_shard_1 AS
SELECT * FROM tv_post WHERE user_id >= '00000000-0000-0000-0000-000000000000'
  AND user_id < '40000000-0000-0000-0000-000000000000';

CREATE TABLE tv_post_shard_2 AS
SELECT * FROM tv_post WHERE user_id >= '40000000-0000-0000-0000-000000000000'
  AND user_id < '80000000-0000-0000-0000-000000000000';
```

## Maintenance Optimization

### Automated Maintenance

Schedule regular maintenance tasks:

```sql
-- Vacuum TVIEWs during low-traffic windows
CREATE OR REPLACE FUNCTION maintenance_vacuum_tviews()
RETURNS void AS $$
DECLARE
    rec record;
BEGIN
    FOR rec IN SELECT oid::regclass::text as table_name
               FROM pg_class
               WHERE relname LIKE 'tv_%' AND relkind = 'r' LOOP
        EXECUTE 'VACUUM ANALYZE ' || rec.table_name;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- Reindex TVIEWs periodically
CREATE OR REPLACE FUNCTION maintenance_reindex_tviews()
RETURNS void AS $$
DECLARE
    rec record;
BEGIN
    FOR rec IN SELECT indexname, tablename
               FROM pg_indexes
               WHERE tablename LIKE 'tv_%' LOOP
        EXECUTE 'REINDEX INDEX CONCURRENTLY ' || rec.indexname;
    END LOOP;
END;
$$ LANGUAGE plpgsql;
```

### Statistics Updates

Keep statistics current for query planning:

```sql
-- Update statistics more frequently for TVIEWs
ALTER TABLE tv_post SET (autovacuum_analyze_scale_factor = 0.05);
ALTER TABLE tv_post SET (autovacuum_analyze_threshold = 50);

-- Manual statistics update
ANALYZE tv_post, tv_user, tv_comment;
```

## Performance Troubleshooting

### Slow Query Analysis

Diagnose slow TVIEW queries:

```sql
-- Analyze query performance
EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT data FROM tv_post WHERE user_id = 'uuid-here' ORDER BY data->>'createdAt' DESC LIMIT 10;

-- Check index usage
SELECT * FROM pg_stat_user_indexes WHERE tablename = 'tv_post' ORDER BY idx_scan DESC;

-- Check table bloat
SELECT schemaname, tablename, n_dead_tup, n_live_tup,
       ROUND(n_dead_tup::float / GREATEST(n_live_tup, 1) * 100, 2) as bloat_ratio
FROM pg_stat_user_tables
WHERE schemaname = 'public' AND tablename LIKE 'tv_%';
```

### TVIEW Refresh Bottlenecks

Identify refresh performance issues:

```sql
-- Profile refresh operations
SELECT pg_tviews_queue_stats();

-- Check for cascade bottlenecks
SELECT pg_tviews_debug_queue();

-- Monitor system resources during refreshes
SELECT * FROM pg_stat_bgwriter;
SELECT * FROM pg_stat_database;
```

### Memory Tuning

Optimize memory usage:

```sql
-- Monitor memory usage
SELECT name, setting, unit FROM pg_settings WHERE name LIKE '%mem%';

-- Adjust based on workload
ALTER SYSTEM SET work_mem = '8MB';        -- Per-connection sort/hash memory
ALTER SYSTEM SET maintenance_work_mem = '1GB';  -- Index builds, vacuums
ALTER SYSTEM SET shared_buffers = '4GB';  -- Shared buffer cache
```

## Best Practices Summary

### Configuration
- Use statement-level triggers for bulk operations
- Configure appropriate memory settings
- Enable connection pooling
- Monitor cache hit rates (>80%)

### Indexing
- Index all lookup patterns (id, user_id, etc.)
- Use GIN indexes for JSONB fields
- Create composite indexes for complex queries
- Monitor index usage and remove unused indexes

### Monitoring
- Set up comprehensive health checks
- Monitor queue size and refresh timing
- Track cache performance
- Alert on performance degradation

### Maintenance
- Regular vacuum and analyze operations
- Monitor for table bloat
- Update statistics frequently
- Reindex during maintenance windows

### Scaling
- Use read replicas for read-heavy workloads
- Implement application-level caching
- Consider partitioning for large datasets
- Monitor and optimize cascade depth

## See Also

- [Monitoring Guide](monitoring.md) - Health checks and metrics
- [Troubleshooting Guide](troubleshooting.md) - Issue resolution
- [Benchmarks](../benchmarks/overview.md) - Performance testing methodology