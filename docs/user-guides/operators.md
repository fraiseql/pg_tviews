# Operator Guide

Production deployment and operations guide for running pg_tviews in production environments.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

This guide helps database operators and DevOps engineers deploy and maintain pg_tviews in production. You'll learn about installation, monitoring, backup/restore, and operational best practices.

## Production Installation

### Prerequisites

- **PostgreSQL**: 15, 16, or 17 (17 recommended for latest features)
- **System Resources**: 2GB RAM minimum, 4GB recommended
- **Storage**: SSD storage recommended for performance
- **Network**: Low-latency connection to application servers

### Multi-Server Installation

For production environments with multiple PostgreSQL servers:

```bash
# On build server
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews
cargo pgrx install --release

# Copy extension files to production servers
# Files to copy:
# - pg_tviews.so → $libdir/
# - pg_tviews.control → $sharedir/extension/
# - pg_tviews--*.sql → $sharedir/extension/

# Verify on each server
psql -d your_db -c "CREATE EXTENSION pg_tviews;"
psql -d your_db -c "SELECT pg_tviews_version();"
```

### Docker Deployment

```dockerfile
# Dockerfile for pg_tviews-enabled PostgreSQL
FROM postgres:17

# Copy extension files (build externally)
COPY --from=pgtviews-builder /usr/lib/postgresql/17/lib/pg_tviews.so /usr/lib/postgresql/17/lib/
COPY --from=pgtviews-builder /usr/share/postgresql/17/extension/pg_tviews* /usr/share/postgresql/17/extension/

# Initialize with extension
COPY init.sql /docker-entrypoint-initdb.d/
```

```sql
-- init.sql
CREATE EXTENSION pg_tviews;
```

### Connection Pooling

pg_tviews works with popular connection poolers:

#### PgBouncer Configuration

```ini
# pgbouncer.ini
[databases]
mydb = host=localhost port=5432 dbname=mydb

[pgbouncer]
pool_mode = transaction
server_reset_query = DISCARD ALL  # pg_tviews handles this automatically
max_client_conn = 1000
default_pool_size = 20
```

#### Pgpool-II Configuration

```ini
# pgpool.conf
connection_cache = on
reset_query_list = 'DISCARD ALL'
max_pool = 4
num_init_children = 32
```

## Database Configuration

### Memory Settings

```sql
-- For datasets up to 100GB
ALTER SYSTEM SET shared_buffers = '2GB';
ALTER SYSTEM SET work_mem = '64MB';
ALTER SYSTEM SET maintenance_work_mem = '512MB';
ALTER SYSTEM SET wal_buffers = '16MB';

-- For larger datasets, scale accordingly
-- shared_buffers = 25% of RAM (max 8GB)
-- work_mem = 2-4MB per connection
```

### WAL Configuration

```sql
-- Ensure WAL is configured for your workload
ALTER SYSTEM SET wal_level = replica;
ALTER SYSTEM SET max_wal_senders = 10;
ALTER SYSTEM SET wal_keep_size = '1GB';

-- For high-write workloads
ALTER SYSTEM SET checkpoint_segments = 32;
ALTER SYSTEM SET checkpoint_completion_target = 0.9;
```

### Autovacuum Tuning

```sql
-- Tune autovacuum for TVIEW tables
ALTER TABLE tv_post SET (autovacuum_vacuum_scale_factor = 0.1);
ALTER TABLE tv_post SET (autovacuum_analyze_scale_factor = 0.05);

-- For high-write tables
ALTER TABLE tb_post SET (autovacuum_vacuum_scale_factor = 0.02);
ALTER TABLE tb_post SET (autovacuum_analyze_scale_factor = 0.01);
```

## Monitoring Setup

### Health Checks

Implement comprehensive health monitoring:

```sql
-- Basic health check
CREATE OR REPLACE FUNCTION health_check()
RETURNS jsonb AS $$
DECLARE
    result jsonb;
BEGIN
    -- Extension health
    SELECT jsonb_build_object(
        'extension_version', pg_tviews_version(),
        'jsonb_ivm_available', pg_tviews_check_jsonb_ivm(),
        'server_version', version(),
        'current_time', now()
    ) INTO result;

    RETURN result;
END;
$$ LANGUAGE plpgsql;

-- TVIEW-specific health check
CREATE OR REPLACE FUNCTION tview_health_check()
RETURNS jsonb AS $$
DECLARE
    health_record record;
    result jsonb := '{}';
BEGIN
    -- Get pg_tviews health
    SELECT * INTO health_record FROM pg_tviews_health_check();

    result := result || jsonb_build_object('tview_health', row_to_json(health_record));

    -- Check TVIEW counts vs base tables
    SELECT result || jsonb_build_object('table_counts',
        jsonb_build_object(
            'tv_post_count', (SELECT COUNT(*) FROM tv_post),
            'tb_post_count', (SELECT COUNT(*) FROM tb_post),
            'tv_user_count', (SELECT COUNT(*) FROM tv_user),
            'tb_user_count', (SELECT COUNT(*) FROM tb_user)
        )
    ) INTO result;

    RETURN result;
END;
$$ LANGUAGE plpgsql;
```

### Performance Metrics

Set up continuous performance monitoring:

```sql
-- Create metrics table
CREATE TABLE tview_metrics (
    id BIGSERIAL PRIMARY KEY,
    collected_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    queue_stats jsonb,
    cache_stats jsonb,
    performance_summary jsonb
);

-- Collect metrics every 5 minutes
CREATE OR REPLACE FUNCTION collect_tview_metrics()
RETURNS void AS $$
BEGIN
    INSERT INTO tview_metrics (queue_stats, cache_stats, performance_summary)
    VALUES (
        pg_tviews_queue_stats(),
        (SELECT jsonb_object_agg(table_name, cache_info)
         FROM (
             SELECT schemaname || '.' || tablename as table_name,
                    jsonb_build_object('reltuples', reltuples, 'relpages', relpages)
             FROM pg_class c
             JOIN pg_namespace n ON c.relnamespace = n.oid
             WHERE n.nspname = 'public' AND c.relname LIKE 'tv_%'
         ) cache_info),
        (SELECT jsonb_agg(row_to_json(ps))
         FROM pg_tviews_performance_summary ps
         WHERE hour > now() - interval '1 hour')
    );
END;
$$ LANGUAGE plpgsql;

-- Schedule collection (requires pg_cron or similar)
-- SELECT cron.schedule('collect-tview-metrics', '*/5 * * * *', 'SELECT collect_tview_metrics();');
```

### Alerting Queries

Set up alerts for common issues:

```sql
-- Alert: High queue size
SELECT CASE
    WHEN (pg_tviews_queue_stats()->>'queue_size')::int > 1000
    THEN 'CRITICAL: TVIEW refresh queue > 1000'
    WHEN (pg_tviews_queue_stats()->>'queue_size')::int > 100
    THEN 'WARNING: TVIEW refresh queue > 100'
    ELSE 'OK'
END as queue_status;

-- Alert: Slow refreshes
SELECT CASE
    WHEN (pg_tviews_queue_stats()->>'total_timing_ms')::float > 5000
    THEN 'WARNING: TVIEW refresh time > 5 seconds'
    ELSE 'OK'
END as timing_status;

-- Alert: Low cache hit rate
SELECT CASE
    WHEN (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float < 0.8
    THEN 'WARNING: Graph cache hit rate < 80%'
    ELSE 'OK'
END as cache_status;

-- Alert: TVIEW/base table count mismatch
SELECT CASE
    WHEN ABS(tv_count - tb_count) > (tb_count * 0.01)  -- 1% tolerance
    THEN format('WARNING: TVIEW %s count mismatch: TVIEW=%s, Base=%s',
                entity, tv_count, tb_count)
    ELSE 'OK'
END as consistency_status
FROM (
    SELECT 'post' as entity,
           (SELECT COUNT(*) FROM tv_post) as tv_count,
           (SELECT COUNT(*) FROM tb_post) as tb_count
) counts;
```

## Backup and Recovery

### Logical Backups

pg_tviews works with standard PostgreSQL backup tools:

```bash
# pg_dump (includes TVIEWs)
pg_dump -Fc mydb > mydb_backup.dump

# Restore
pg_restore -d mydb mydb_backup.dump
```

### Physical Backups

TVIEWs are included in physical backups (streaming replication, PITR):

```bash
# Base backup
pg_basebackup -D /var/lib/postgresql/backup -Ft -z -P

# WAL archiving setup
archive_command = 'cp %p /var/lib/postgresql/archive/%f'
restore_command = 'cp /var/lib/postgresql/archive/%f %p'
```

### Point-in-Time Recovery

pg_tviews supports PITR with TVIEW consistency:

```sql
-- Recover to specific time
recovery_target_time = '2025-12-11 14:30:00'
recovery_target_action = 'promote'

-- TVIEWs will be consistent with recovered base tables
-- No manual refresh needed
```

### High Availability

#### Streaming Replication

pg_tviews works with PostgreSQL streaming replication:

```sql
-- On primary
ALTER SYSTEM SET wal_level = replica;
ALTER SYSTEM SET max_wal_senders = 3;

-- On standby
primary_conninfo = 'host=primary dbname=mydb user=repl password=secret'

-- TVIEWs are replicated automatically
-- Triggers fire on primary, TVIEWs updated on standby
```

#### Failover Considerations

```sql
-- Check replication lag
SELECT client_addr, state, sent_lsn, write_lsn, flush_lsn, replay_lsn
FROM pg_stat_replication;

-- Manual failover
-- 1. Stop primary
-- 2. Promote standby: pg_ctl promote
-- 3. Redirect applications to new primary
-- 4. TVIEWs remain consistent (no manual intervention needed)
```

## Maintenance Tasks

### Regular Maintenance

```sql
-- Daily: Update statistics
ANALYZE tv_post, tv_user, tv_comment;

-- Weekly: Reindex TVIEWs (if needed)
REINDEX TABLE CONCURRENTLY tv_post;

-- Monthly: Check for bloat
SELECT schemaname, tablename, n_dead_tup, n_live_tup
FROM pg_stat_user_tables
WHERE n_dead_tup > n_live_tup * 0.1;  -- >10% bloat

-- Clean up bloat if needed
VACUUM FULL tv_post;  -- During maintenance window
```

### TVIEW Maintenance

```sql
-- Check TVIEW metadata health
SELECT entity, table_oid, view_oid, trigger_count
FROM pg_tview_meta;

-- Verify triggers exist
SELECT tgname, tgtype, tgenabled
FROM pg_trigger
WHERE tgname LIKE 'tview%';

-- Recreate triggers if missing (rare)
SELECT pg_tviews_create('tv_post', 'SELECT ...');  -- Recreate TVIEW
```

### Performance Maintenance

```sql
-- Check index usage
SELECT schemaname, tablename, indexname, idx_scan, idx_tup_read, idx_tup_fetch
FROM pg_stat_user_indexes
WHERE tablename LIKE 'tv_%'
ORDER BY idx_scan DESC;

-- Rebuild unused indexes
DROP INDEX CONCURRENTLY unused_index;
CREATE INDEX CONCURRENTLY new_index ON tv_post(user_id, (data->>'createdAt'));

-- Update query plans
SELECT pg_stat_statements_reset();  -- If extension available
```

## Troubleshooting Production Issues

### High CPU Usage

**Symptoms**: High CPU, slow queries, queue backlog

**Diagnosis**:
```sql
-- Check queue size
SELECT pg_tviews_queue_stats();

-- Check cascade depth
SELECT pg_tviews_debug_queue();

-- Check for long-running refreshes
SELECT * FROM pg_stat_activity
WHERE query LIKE '%pg_tviews%' AND state = 'active';
```

**Solutions**:
```sql
-- Enable statement-level triggers for bulk operations
SELECT pg_tviews_install_stmt_triggers();

-- Check for missing indexes
EXPLAIN ANALYZE SELECT data FROM tv_post WHERE user_id = 'uuid';

-- Reduce cascade depth by restructuring relationships
```

### Memory Issues

**Symptoms**: Out of memory errors, swap usage, slow performance

**Diagnosis**:
```sql
-- Check memory usage
SELECT name, setting, unit
FROM pg_settings
WHERE name IN ('shared_buffers', 'work_mem', 'maintenance_work_mem');

-- Check for memory leaks in TVIEW processes
SELECT * FROM pg_tviews_performance_summary
ORDER BY collected_at DESC LIMIT 10;
```

**Solutions**:
```sql
-- Increase work_mem for complex queries
ALTER SYSTEM SET work_mem = '128MB';

-- Add memory limits
ALTER SYSTEM SET work_mem = '64MB';  -- Per connection limit

-- Monitor and restart if needed
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE query LIKE '%pg_tviews%' AND now() - query_start > interval '5 minutes';
```

### Lock Contention

**Symptoms**: Slow updates, deadlock errors, timeout errors

**Diagnosis**:
```sql
-- Check for locks
SELECT locktype, mode, granted, relation::regclass
FROM pg_locks
WHERE relation::regclass::text LIKE 'tv_%';

-- Check for deadlocks
SELECT * FROM pg_stat_database_conflicts;
```

**Solutions**:
```sql
-- Use shorter transactions
BEGIN;
-- Do updates
COMMIT;

-- Implement retry logic in application
-- Use optimistic locking where appropriate
```

### Data Inconsistency

**Symptoms**: TVIEW data doesn't match base tables

**Diagnosis**:
```sql
-- Compare counts
SELECT 'tv_post' as table, COUNT(*) as count FROM tv_post
UNION ALL
SELECT 'tb_post', COUNT(*) FROM tb_post;

-- Check for missing triggers
SELECT tgname FROM pg_trigger WHERE tgname LIKE 'tview%';
```

**Solutions**:
```sql
-- Recreate TVIEW
DROP TABLE tv_post;
CREATE TABLE tv_post AS SELECT ...;

-- Manual refresh if needed
SELECT pg_tviews_cascade('tb_post'::regclass::oid, pk_value);
```

## Scaling Strategies

### Read Scaling

```sql
-- Use read replicas for TVIEW queries
-- TVIEWs are automatically updated on replicas
-- Configure hot standby feedback

-- hot_standby_feedback = on
-- Prevents query conflicts on replicas
```

### Write Scaling

```sql
-- Partition large TVIEWs
CREATE TABLE tv_post_y2025 PARTITION OF tv_post
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

-- Use statement-level triggers for bulk loads
SELECT pg_tviews_install_stmt_triggers();
```

### Connection Pooling at Scale

```ini
# Advanced PgBouncer config
[pgbouncer]
pool_mode = transaction
max_client_conn = 10000
default_pool_size = 50
reserve_pool_size = 10
reserve_pool_timeout = 5
max_db_connections = 100
max_user_connections = 1000
```

## Security Considerations

### Access Control

```sql
-- Grant minimal permissions
GRANT SELECT ON tv_post TO readonly_user;
GRANT SELECT ON tv_user TO readonly_user;

-- No direct DML on TVIEWs
REVOKE INSERT, UPDATE, DELETE ON tv_post FROM public;

-- Function permissions
GRANT EXECUTE ON FUNCTION pg_tviews_health_check() TO monitoring_user;
```

### Audit Logging

```sql
-- Enable audit logging for TVIEW changes
ALTER SYSTEM SET log_statement = 'ddl';
ALTER SYSTEM SET log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h ';

-- Monitor DDL changes
SELECT * FROM pg_stat_user_tables
WHERE schemaname = 'public' AND tablename LIKE 'tv_%';
```

## Disaster Recovery

### Recovery Planning

```sql
-- Document recovery procedures
-- Test recovery regularly
-- Keep multiple backup copies
-- Monitor backup success/failure
```

### Emergency Procedures

```sql
-- Quick TVIEW recreation
DROP TABLE tv_post;
CREATE TABLE tv_post AS SELECT ... FROM tv_post_backup;

-- Manual data repair
UPDATE tv_post SET data = data || '{"status": "repaired"}'::jsonb
WHERE id = 'problematic-id';
```

## See Also

- [Installation Guide](../getting-started/installation.md) - Setup instructions
- [Monitoring Guide](../operations/monitoring.md) - Detailed monitoring setup
- [Troubleshooting Guide](../operations/troubleshooting.md) - Issue resolution
- [Performance Tuning](../operations/performance-tuning.md) - Optimization strategies