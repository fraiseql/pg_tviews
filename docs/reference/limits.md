# Resource Limits and Recommendations

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

## Overview

This document outlines tested resource limits, PostgreSQL configuration recommendations, and capacity planning guidelines for pg_tviews deployments.

## Tested Limits

Based on benchmark testing and production experience:

| Resource | Tested Limit | Recommended Max | Notes |
|----------|--------------|-----------------|-------|
| TVIEW Size | 10M rows | 5M rows | Beyond 5M, consider partitioning |
| Dependency Depth | 10 levels | 5 levels | Hard limit: 10, soft limit: 5 for performance |
| Cascade Width | 50 TVIEWs | 20 TVIEWs | One base table → N TVIEWs |
| JSONB Data Size | 10 MB | 1 MB | Per-row JSONB document size |
| Concurrent Refresh | 100 sessions | 50 sessions | Parallel cascade updates |
| Batch Size | 100K rows | 10K rows | Single UPDATE affecting TVIEWs |

## PostgreSQL Configuration Recommendations

### Small Deployments (<100K rows per TVIEW)

```sql
-- postgresql.conf
work_mem = 64MB
shared_buffers = 256MB
effective_cache_size = 1GB
maintenance_work_mem = 128MB
```

### Medium Deployments (100K-1M rows)

```sql
work_mem = 128MB
shared_buffers = 1GB
effective_cache_size = 4GB
maintenance_work_mem = 512MB
max_parallel_workers_per_gather = 4
```

### Large Deployments (>1M rows)

```sql
work_mem = 256MB
shared_buffers = 4GB
effective_cache_size = 16GB
maintenance_work_mem = 2GB
max_parallel_workers_per_gather = 8
random_page_cost = 1.1  -- For SSD storage
```

## Capacity Planning

### Estimating TVIEW Size

```sql
-- Current size
-- Trinity pattern: tv_your_entity has pk_your_entity (int), id (UUID), data (JSONB)
SELECT pg_size_pretty(pg_relation_size('tv_your_entity'));

-- With indexes (includes pk_your_entity primary key, id index)
SELECT pg_size_pretty(pg_total_relation_size('tv_your_entity'));

-- Projection: If base table has 1M rows
-- Estimated TVIEW size ≈ base_table_size × 1.5 to 2.0
-- (due to JSONB denormalization + UUID storage)
```

### Estimating Cascade Performance

```
Single-row cascade time = 5-8ms (with jsonb_delta)
Batch cascade (N rows) ≈ 5ms + (N × 0.5ms)
Example: 1000 rows ≈ 505ms
```

## Scaling Recommendations

### Horizontal Scaling

- **Read Replicas**: Use read replicas for TVIEW queries, primary handles writes
- **Connection Pooling**: Implement PgBouncer for connection management
- **Load Balancing**: Distribute read queries across replicas

### Vertical Scaling

- **More RAM**: Larger shared_buffers, work_mem for better performance
- **More CPU**: Increase max_parallel_workers for concurrent operations
- **Faster Storage**: Reduce random_page_cost for SSD deployments

### Partitioning Strategy

For TVIEWs >5M rows:

```sql
-- Partition by date (for time-series data)
-- Note: Use singular names (tv_event, not tv_events)
CREATE TABLE tv_event_y2025_m01 PARTITION OF tv_event
FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');

-- Partition by hash (for general data)
-- Note: Use singular names (tv_user, not tv_users)
CREATE TABLE tv_user_0 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 0);

CREATE TABLE tv_user_1 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 1);
```

## Performance Benchmarks

### Single TVIEW Operations

| Operation | 1K rows | 10K rows | 100K rows | 1M rows |
|-----------|---------|----------|-----------|---------|
| TVIEW Creation | <1s | <5s | <30s | <5min |
| Single-row Update | <10ms | <10ms | <50ms | <100ms |
| Batch Update (100) | <100ms | <500ms | <2s | <10s |
| Full Scan | <100ms | <1s | <10s | <2min |

### Cascade Performance

| Cascade Depth | 1 Level | 3 Levels | 5 Levels |
|---------------|---------|----------|----------|
| 100 rows | <200ms | <500ms | <1s |
| 1K rows | <2s | <5s | <10s |
| 10K rows | <20s | <50s | <2min |

## Memory Requirements

### Per-Connection Memory

```
Base memory per connection: 2-5MB
work_mem per sort/hash: Configured value
JSONB processing: 2-10MB for large documents
Cascade state: 1-5MB per active cascade
```

### Total Memory Calculation

```sql
-- Estimate total memory usage
SELECT
  (SELECT count(*) FROM pg_stat_activity WHERE state = 'active') * 5 as base_mb,
  current_setting('work_mem')::int / 1024 as work_mem_mb,
  (SELECT count(*) FROM pg_tview_meta) * 2 as cascade_overhead_mb
FROM pg_settings
WHERE name = 'work_mem';
```

## Monitoring Thresholds

### Alert Levels

- **Queue Size**: Warning > 100, Critical > 1000
- **Update Latency**: Warning > 1s, Critical > 5s
- **Memory Usage**: Warning > 70%, Critical > 85%
- **Error Rate**: Warning > 1%, Critical > 5%
- **Cascade Depth**: Warning > 3, Critical > 5

### Health Check Queries

```sql
-- Quick health assessment
SELECT
    CASE WHEN queue_size > 100 THEN 'WARNING' ELSE 'OK' END as queue_status,
    CASE WHEN memory_usage > 0.8 THEN 'WARNING' ELSE 'OK' END as memory_status,
    CASE WHEN error_count > 0 THEN 'ERROR' ELSE 'OK' END as error_status
FROM (
    SELECT
        (SELECT queue_size FROM pg_tviews_queue_realtime LIMIT 1) as queue_size,
        (SELECT sum(total_bytes) / (SELECT setting::bigint * 1024 * 1024
                                   FROM pg_settings WHERE name = 'shared_buffers')
         FROM pg_backend_memory_contexts) as memory_usage,
        (SELECT count(*) FROM pg_log WHERE message LIKE '%ERROR%' AND log_time > now() - interval '1 hour') as error_count
) checks;
```

## Backup and Recovery

### Backup Strategy

```bash
# Metadata-only backup (fast, small)
pg_dump -t pg_tview_meta -t pg_tview_helpers > tview_metadata.sql

# Full database backup
pg_dump dbname > full_backup.sql

# Incremental WAL shipping
# Configure for PITR recovery
```

### Recovery Time Objectives

- **RTO (Recovery Time Objective)**: < 15 minutes for metadata, < 4 hours for full data
- **RPO (Recovery Point Objective)**: < 5 minutes data loss acceptable
- **Backup Frequency**: Metadata hourly, full database daily

## Troubleshooting High Resource Usage

### CPU Usage Issues

```sql
-- Identify CPU-intensive queries
SELECT
    pg_stat_activity.pid,
    pg_stat_activity.query,
    pg_stat_activity.state,
    extract(epoch from (now() - pg_stat_activity.query_start)) as duration_seconds
FROM pg_stat_activity
WHERE pg_stat_activity.state = 'active'
ORDER BY duration_seconds DESC
LIMIT 10;
```

### Memory Issues

```sql
-- Check memory usage by connection
SELECT
    pg_stat_activity.pid,
    pg_stat_activity.usename,
    pg_stat_activity.client_addr,
    pg_size_pretty(pg_backend_memory_contexts.total_bytes) as memory_used,
    pg_backend_memory_contexts.name as context_name
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
ORDER BY pg_backend_memory_contexts.total_bytes DESC;
```

### Disk I/O Issues

```sql
-- Monitor I/O statistics
SELECT
    schemaname,
    tablename,
    seq_scan,
    seq_tup_read,
    idx_scan,
    idx_tup_fetch,
    n_tup_ins,
    n_tup_upd,
    n_tup_del
FROM pg_stat_user_tables
WHERE schemaname = 'public'
ORDER BY n_tup_upd DESC;
```

## See Also

- [Performance Tuning](../operations/performance-tuning.md) - Detailed optimization guides
- [Monitoring Guide](../operations/monitoring.md) - Health check and metrics
- [Troubleshooting Guide](../operations/troubleshooting.md) - Production procedures