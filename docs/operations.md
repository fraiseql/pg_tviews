# pg_tviews Operations Guide

**Version**: 0.1.0-alpha
**Last Updated**: December 10, 2025

## Overview

This guide provides operational procedures for production deployment and maintenance of pg_tviews. It covers installation, backup/restore, connection pooling, upgrades, and day-to-day operations.

## Installation

### Prerequisites

- **PostgreSQL**: 15.0 or later
- **Rust**: 1.70+ (for building from source)
- **pgrx**: 0.12.8+ (for building from source)
- **System packages**: `postgresql-server-dev-15` (or appropriate version)

### Installation Methods

#### Method 1: Pre-built Extension (Recommended)

```bash
# Download and install the extension
# (Installation commands will be provided with releases)

# Enable in database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

#### Method 2: Build from Source

```bash
# Clone repository
git clone https://github.com/fraiseql/pg_tviews.git
cd pg_tviews

# Build and install
cargo pgrx install --release

# Enable in database
psql -d your_database -c "CREATE EXTENSION pg_tviews;"
```

### Post-Installation Setup

#### 1. Verify Installation

```sql
-- Check extension version
SELECT pg_tviews_version();

-- Run health check
SELECT * FROM pg_tviews_health_check();
```

#### 2. Optional: Install jsonb_ivm

```bash
# Install jsonb_ivm for 1.5-3× performance improvement
git clone https://github.com/fraiseql/jsonb_ivm.git
cd jsonb_ivm
cargo pgrx install --release
```

#### 3. Configure Monitoring (Recommended)

```sql
-- Install monitoring infrastructure
\i sql/pg_tviews_monitoring.sql

-- Install statement-level triggers for bulk operations
SELECT pg_tviews_install_stmt_triggers();
```

### Database Permissions

The extension requires appropriate permissions:

```sql
-- Grant usage to application user
GRANT USAGE ON SCHEMA public TO app_user;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO app_user;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO app_user;

-- For monitoring (optional)
GRANT SELECT ON pg_tviews_queue_realtime TO app_user;
GRANT SELECT ON pg_tviews_cache_stats TO app_user;
GRANT EXECUTE ON FUNCTION pg_tviews_health_check() TO app_user;
```

## Backup and Restore

### TVIEW Backup Strategy

pg_tviews requires a specific backup strategy because TVIEWs have both **definition** and **data** components.

#### Components to Backup

1. **Extension Installation**: `CREATE EXTENSION pg_tviews;`
2. **TVIEW Definitions**: DDL statements to recreate TVIEWs
3. **Metadata Tables**: `pg_tview_meta`, `pg_tview_helpers`
4. **Monitoring Data**: `pg_tviews_metrics` (optional)
5. **Base Table Data**: Source tables that TVIEWs depend on

### Backup Procedures

#### Method 1: pg_dump with Custom Format (Recommended)

```bash
# Step 1: Create backup directory
mkdir -p /var/backups/pg_tviews/$(date +%Y%m%d)
cd /var/backups/pg_tviews/$(date +%Y%m%d)

# Step 2: Dump TVIEW definitions separately
pg_dump -d your_database \
  --schema-only \
  --extension=pg_tviews \
  --table=pg_tview_meta \
  --table=pg_tview_helpers \
  -f tview_definitions.sql

# Step 3: Dump base table data
pg_dump -d your_database \
  --data-only \
  --table=tb_* \
  --table=tv_* \
  -f tview_data.sql

# Step 4: Full database dump (for reference)
pg_dump -d your_database \
  --format=custom \
  --compress=9 \
  -f full_backup.dump
```

#### Method 2: Logical Backup with TVIEW Recreation

```sql
-- Step 1: Extract TVIEW definitions
CREATE TEMP TABLE tview_backup AS
SELECT
    entity,
    'CREATE TVIEW tv_' || entity || ' AS ' ||
    (SELECT pg_get_viewdef('v_' || entity)) as ddl_statement
FROM pg_tview_meta;

-- Step 2: Export definitions
COPY tview_backup TO '/tmp/tview_definitions.sql' WITH CSV;

-- Step 3: Backup metadata
pg_dump -d your_database \
  --table=pg_tview_meta \
  --table=pg_tview_helpers \
  -f metadata_backup.sql
```

### Restore Procedures

#### Complete Restore Process

```bash
# Step 1: Restore base PostgreSQL installation
# (Standard pg_restore process)

# Step 2: Enable pg_tviews extension
psql -d your_database -c "CREATE EXTENSION pg_tviews;"

# Step 3: Restore metadata tables
psql -d your_database -f metadata_backup.sql

# Step 4: Recreate TVIEWs
psql -d your_database -f tview_definitions.sql

# Step 5: Restore base table data
psql -d your_database -f tview_data.sql

# Step 6: Verify restoration
psql -d your_database -c "SELECT * FROM pg_tviews_health_check();"
```

#### Selective TVIEW Restore

```sql
-- Restore specific TVIEW
CREATE TABLE tv_post AS
SELECT pk_post, data FROM tv_post_backup;

-- Rebuild dependencies
SELECT pg_tviews_install_stmt_triggers();
```

### Point-in-Time Recovery (PITR)

#### Considerations for TVIEWs

- **Transaction Consistency**: TVIEWs maintain consistency with base tables
- **Trigger State**: Triggers must be reinstalled after PITR
- **Cache Invalidation**: Graph cache needs rebuilding

#### PITR Procedure

```bash
# Step 1: Perform standard PITR
# (Follow PostgreSQL PITR procedures)

# Step 2: Re-enable pg_tviews
psql -d your_database -c "CREATE EXTENSION pg_tviews;"

# Step 3: Restore TVIEW definitions
psql -d your_database -f tview_definitions_backup.sql

# Step 4: Reinstall triggers
psql -d your_database -c "SELECT pg_tviews_install_stmt_triggers();"

# Step 5: Validate consistency
psql -d your_database -c "
  SELECT entity, COUNT(*) as row_count
  FROM pg_tview_meta m
  JOIN (SELECT (regexp_match(relname, '^tv_(.*)'))[1] as entity,
               reltuples::bigint as cnt
        FROM pg_class
        WHERE relname LIKE 'tv_%') t ON t.entity = m.entity;
"
```

### Backup Validation

#### Pre-backup Checks

```sql
-- Verify all TVIEWs are healthy
SELECT * FROM pg_tviews_health_check()
WHERE status != 'OK';

-- Check for pending operations
SELECT * FROM pg_tviews_queue_realtime;

-- Validate TVIEW definitions
SELECT entity,
       CASE WHEN pg_get_viewdef('v_' || entity) IS NOT NULL
            THEN 'OK' ELSE 'ERROR' END as view_status
FROM pg_tview_meta;
```

#### Post-backup Validation

```sql
-- Test restore in staging environment
createdb tview_restore_test;
psql -d tview_restore_test -f backup.sql

-- Verify TVIEWs work
psql -d tview_restore_test -c "
  SELECT entity, COUNT(*) as rows
  FROM pg_tview_meta m
  JOIN pg_class c ON c.relname = 'tv_' || m.entity;
"
```

## Connection Pooling

### Overview

pg_tviews uses **thread-local state** for refresh queues, which has implications for connection pooling:

- **Same Connection**: Operations in the same transaction must use the same connection
- **Transaction Isolation**: Queue state is isolated per connection
- **DISCARD ALL**: Required when returning connections to pool

### PgBouncer Configuration

#### Recommended Configuration

```ini
# pgbouncer.ini
[databases]
your_database = host=localhost port=5432 dbname=your_database

[pgbouncer]
listen_port = 6432
listen_addr = 127.0.0.1
auth_type = md5
auth_file = /etc/pgbouncer/userlist.txt
pool_mode = transaction
max_client_conn = 1000
default_pool_size = 20
reserve_pool_size = 5

# Critical for pg_tviews: Reset connection state
server_reset_query = DISCARD ALL
```

#### Why DISCARD ALL?

```sql
-- DISCARD ALL resets:
-- 1. Session variables (SET LOCAL)
-- 2. Temporary tables
-- 3. Prepared statements
-- 4. pg_tviews thread-local queue state

DISCARD ALL;
```

#### Pool Sizing Guidelines

```ini
# For read-heavy workloads
pool_mode = transaction
default_pool_size = 50
reserve_pool_size = 10

# For write-heavy workloads (more TVIEW refreshes)
pool_mode = transaction
default_pool_size = 30
reserve_pool_size = 15
min_pool_size = 5
```

### pgpool-II Configuration

#### Basic Configuration

```ini
# pgpool.conf
listen_addresses = 'localhost'
port = 9999
socket_dir = '/var/run/pgpool'
pcp_socket_dir = '/var/run/pgpool'

backend_hostname0 = 'localhost'
backend_port0 = 5432
backend_weight0 = 1
backend_data_directory0 = '/var/lib/postgresql/15/main'

# Connection pooling
num_init_children = 32
max_pool = 4
child_life_time = 300
child_max_connections = 100
connection_life_time = 0
client_idle_limit = 0

# Load balancing
load_balance_mode = on
ignore_leading_white_space = on
white_function_list = 'pg_tviews_*'
black_function_list = 'pg_tviews_cascade,pg_tviews_insert,pg_tviews_delete'
```

#### pg_tviews-Specific Settings

```ini
# Functions that modify data should go to primary
black_function_list = 'pg_tviews_cascade,pg_tviews_insert,pg_tviews_delete,pg_tviews_commit_prepared,pg_tviews_rollback_prepared'

# Read-only functions can be load balanced
white_function_list = 'pg_tviews_version,pg_tviews_check_jsonb_ivm,pg_tviews_queue_stats,pg_tviews_debug_queue,pg_tviews_analyze_select,pg_tviews_infer_types,pg_tviews_health_check'

# Reset query for connection cleanup
reset_query = 'DISCARD ALL'
```

### Connection Pooler Compatibility

| Pooler | Compatibility | Notes |
|--------|---------------|-------|
| **PgBouncer** | ✅ Full | Use `DISCARD ALL` in server_reset_query |
| **pgpool-II** | ✅ Full | Configure black/white function lists |
| **AWS RDS Proxy** | ⚠️ Limited | May not support DISCARD ALL |
| **Azure Database Proxy** | ⚠️ Limited | Check DISCARD ALL support |
| **Google Cloud SQL Proxy** | ❌ Not Compatible | No session state management |

### Troubleshooting Connection Pooling

#### Issue: Queue State Lost

**Symptoms**:
```sql
-- Queue appears empty after connection pool switch
SELECT * FROM pg_tviews_queue_realtime; -- Returns no rows
```

**Solution**: Ensure `DISCARD ALL` is configured:
```ini
# pgbouncer.ini
server_reset_query = DISCARD ALL
```

#### Issue: Transaction Errors

**Symptoms**:
```sql
ERROR: TVIEW queue state lost across connection switch
```

**Solution**: Use transaction pooling mode:
```ini
# pgbouncer.ini
pool_mode = transaction
```

#### Issue: Performance Degradation

**Symptoms**:
- Slow response times
- Queue buildup

**Debug**:
```sql
-- Check connection pool utilization
SHOW POOLS;

-- Monitor queue per connection
SELECT session, transaction_id, queue_size
FROM pg_tviews_queue_realtime
ORDER BY queue_size DESC;
```

**Solutions**:
- Increase pool size
- Use session pooling for TVIEW-heavy workloads
- Implement connection pinning for TVIEW operations

### Best Practices

#### 1. Monitor Pool Health

```sql
-- Check pool status
SELECT
    datname,
    cl_active,
    cl_waiting,
    sv_active,
    sv_idle,
    sv_used,
    sv_tested,
    sv_login
FROM pg_stat_database
JOIN pg_stat_activity ON datname = current_database();
```

#### 2. Connection Pool Metrics

```sql
-- Monitor TVIEW operations per connection
SELECT
    application_name,
    COUNT(*) as operations,
    AVG(queue_size) as avg_queue,
    MAX(queue_size) as max_queue
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
GROUP BY application_name;
```

#### 3. Application-Level Pooling

```python
# Python example with connection pinning
import psycopg2
from psycopg2 import pool

# Create pool with TVIEW-aware settings
pool = psycopg2.pool.ThreadedConnectionPool(
    minconn=5,
    maxconn=20,
    host="localhost",
    port=6432,  # PgBouncer port
    database="your_db",
    user="your_user",
    password="your_password"
)

# Pin connection for TVIEW operations
def with_tview_connection():
    conn = pool.getconn()
    try:
        # Perform TVIEW operations
        conn.cursor().execute("INSERT INTO posts ...")
        conn.cursor().execute("SELECT * FROM tv_post")
        conn.commit()
    finally:
        pool.putconn(conn)
```

## Upgrades

### Version Compatibility

#### Supported Upgrade Paths

| From Version | To Version | Method | Notes |
|-------------|------------|--------|-------|
| 0.1.0-alpha | 0.1.0-beta.1 | Extension update | Full compatibility |
| 0.1.0-beta.x | 0.1.0-rc.1 | Extension update | Metadata migration may be required |
| 0.1.0-rc.x | 1.0.0 | Extension update | Breaking changes possible |

### Upgrade Procedure

#### Step 1: Pre-Upgrade Checks

```sql
-- Verify current version
SELECT pg_tviews_version();

-- Check system health
SELECT * FROM pg_tviews_health_check();

-- Backup TVIEW definitions
CREATE TABLE tview_backup_pre_upgrade AS
SELECT entity, pg_get_viewdef('v_' || entity) as definition
FROM pg_tview_meta;
```

#### Step 2: Extension Upgrade

```bash
# Method 1: Using pgrx (development)
cargo pgrx install --release

# Method 2: Package manager (production)
# Follow system-specific package update procedures

# Restart PostgreSQL if required
sudo systemctl restart postgresql
```

#### Step 3: Post-Upgrade Validation

```sql
-- Verify new version
SELECT pg_tviews_version();

-- Check health after upgrade
SELECT * FROM pg_tviews_health_check();

-- Validate TVIEWs still work
SELECT entity,
       CASE WHEN pg_get_viewdef('v_' || entity) IS NOT NULL
            THEN 'OK' ELSE 'ERROR' END as status
FROM pg_tview_meta;

-- Test basic operations
INSERT INTO tb_test (id, data) VALUES (1, '{}');
SELECT * FROM tv_test WHERE pk_test = 1;
```

#### Step 4: Reinstall Components

```sql
-- Reinstall statement-level triggers
SELECT pg_tviews_install_stmt_triggers();

-- Reinstall monitoring (if updated)
-- \i sql/pg_tviews_monitoring.sql
```

### Rollback Procedure

#### Emergency Rollback

```bash
# Step 1: Stop application
# (Prevent new operations during rollback)

# Step 2: Downgrade extension
ALTER EXTENSION pg_tviews UPDATE TO '0.1.0-alpha';

# Step 3: Restart PostgreSQL
sudo systemctl restart postgresql

# Step 4: Validate rollback
SELECT pg_tviews_version();
SELECT * FROM pg_tviews_health_check();
```

#### Data Consistency Check

```sql
-- Verify TVIEW data consistency after rollback
SELECT
    t.entity,
    COUNT(tv.*) as tview_rows,
    COUNT(bt.*) as base_rows
FROM pg_tview_meta t
JOIN pg_class tv ON tv.relname = 'tv_' || t.entity
JOIN pg_class bt ON bt.relname = 'tb_' || t.entity
GROUP BY t.entity;
```

### Breaking Changes Handling

#### Version 0.1.0-beta Changes

- **Function signatures**: Some functions may have parameter changes
- **Metadata format**: Internal metadata structure updates
- **Performance characteristics**: Different caching behavior

#### Migration Scripts

```sql
-- Example migration for breaking changes
-- (Will be provided with releases)

-- Migrate metadata format
UPDATE pg_tview_meta SET
    metadata_version = 'beta'
WHERE metadata_version = 'alpha';

-- Rebuild caches
SELECT pg_tviews_install_stmt_triggers();
```

## Performance Tuning

### Memory Configuration

```postgresql
# postgresql.conf
# Increase work_mem for complex TVIEW queries
work_mem = 64MB

# Increase maintenance_work_mem for TVIEW creation
maintenance_work_mem = 256MB

# Shared buffers (general PostgreSQL tuning)
shared_buffers = 256MB
```

### TVIEW-Specific Tuning

#### Cache Configuration

```sql
-- Monitor cache performance
SELECT * FROM pg_tviews_cache_stats;

-- Cache hit rates should be > 80%
SELECT
    'graph_cache_hit_rate' as metric,
    AVG(CASE WHEN graph_cache_hit THEN 1.0 ELSE 0.0 END) * 100 as value
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour';
```

#### Queue Tuning

```sql
-- Monitor queue performance
SELECT
    AVG(queue_size) as avg_queue_size,
    MAX(queue_size) as max_queue_size,
    AVG(timing_ms) as avg_timing_ms
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour';
```

### Index Recommendations

```sql
-- Primary key indexes (automatically created)
-- Ensure base tables have primary keys
SELECT tablename, indexname
FROM pg_indexes
WHERE tablename LIKE 'tb_%' AND indexname LIKE '%pkey';

-- Foreign key indexes for performance
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_posts_user_fk
ON tb_post (fk_user);

-- TVIEW query optimization
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_tv_post_author_name
ON tv_post ((data->'author'->>'name'));
```

### Bulk Operation Optimization

```sql
-- For bulk inserts/updates, use statement-level triggers
SELECT pg_tviews_install_stmt_triggers();

-- Monitor bulk operation performance
SELECT
    CASE WHEN bulk_refresh_count > 0 THEN 'bulk' ELSE 'individual' END as operation_type,
    AVG(timing_ms) as avg_timing,
    COUNT(*) as operation_count
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '1 hour'
GROUP BY CASE WHEN bulk_refresh_count > 0 THEN 'bulk' ELSE 'individual' END;
```

## Maintenance Tasks

### Daily Tasks

#### 1. Health Monitoring

```sql
-- Automated health check
SELECT * FROM pg_tviews_health_check()
WHERE status != 'OK';
```

#### 2. Queue Monitoring

```sql
-- Check for stuck queues
SELECT * FROM pg_tviews_queue_realtime
WHERE queue_size > 100
  AND last_enqueued < now() - interval '5 minutes';
```

#### 3. Performance Trends

```sql
-- Daily performance summary
SELECT
    date_trunc('hour', recorded_at) as hour,
    AVG(timing_ms) as avg_timing,
    COUNT(*) as operations
FROM pg_tviews_metrics
WHERE recorded_at >= CURRENT_DATE
GROUP BY date_trunc('hour', recorded_at)
ORDER BY hour;
```

### Weekly Tasks

#### 1. Metrics Cleanup

```sql
-- Clean old metrics (keep 7 days)
SELECT pg_tviews_cleanup_metrics(7);
```

#### 2. Cache Analysis

```sql
-- Analyze cache efficiency
SELECT
    cache_type,
    entries,
    CASE
        WHEN cache_type = 'graph_cache' AND entries < 10 THEN 'WARNING: Low cache size'
        WHEN cache_type = 'prepared_statements' AND entries < 20 THEN 'WARNING: Few prepared statements'
        ELSE 'OK'
    END as status
FROM pg_tviews_cache_stats;
```

#### 3. TVIEW Validation

```sql
-- Validate all TVIEWs are accessible
SELECT
    'tv_' || entity as table_name,
    CASE WHEN EXISTS (
        SELECT 1 FROM information_schema.tables
        WHERE table_name = 'tv_' || entity
    ) THEN 'EXISTS' ELSE 'MISSING' END as status
FROM pg_tview_meta;
```

### Monthly Tasks

#### 1. Comprehensive Backup Test

```bash
# Test backup restoration
createdb backup_test
pg_restore -d backup_test production_backup.dump
psql -d backup_test -c "SELECT * FROM pg_tviews_health_check()"
dropdb backup_test
```

#### 2. Performance Baseline

```sql
-- Establish performance baseline
CREATE TABLE performance_baseline AS
SELECT
    date_trunc('month', now()) as baseline_month,
    AVG(timing_ms) as avg_timing_baseline,
    AVG(queue_size) as avg_queue_baseline,
    COUNT(*) as total_operations
FROM pg_tviews_metrics
WHERE recorded_at >= date_trunc('month', now() - interval '1 month')
  AND recorded_at < date_trunc('month', now());
```

#### 3. Index Maintenance

```sql
-- Reindex TVIEW tables
REINDEX TABLE CONCURRENTLY tv_post;
REINDEX TABLE CONCURRENTLY tv_user;

-- Update statistics
ANALYZE tv_post, tv_user;
```

### Automated Maintenance

#### Cron Jobs

```bash
# Daily health check
0 6 * * * psql -d your_db -c "SELECT * FROM pg_tviews_health_check() WHERE status != 'OK'" > /var/log/pg_tviews_health.log 2>&1

# Weekly metrics cleanup
0 2 * * 0 psql -d your_db -c "SELECT pg_tviews_cleanup_metrics(30)"

# Monthly backup validation
0 3 1 * * /usr/local/bin/test_pg_tviews_backup.sh
```

#### Monitoring Integration

```sql
-- Create monitoring views for alerting
CREATE VIEW monitoring_alerts AS
SELECT
    'queue_size' as alert_type,
    queue_size as current_value,
    100 as threshold,
    CASE WHEN queue_size > 100 THEN 'CRITICAL' ELSE 'OK' END as status
FROM pg_tviews_queue_realtime
WHERE queue_size > 0

UNION ALL

SELECT
    'timing' as alert_type,
    timing_ms as current_value,
    500 as threshold,
    CASE WHEN timing_ms > 500 THEN 'WARNING' ELSE 'OK' END as status
FROM pg_tviews_metrics
WHERE recorded_at > now() - interval '5 minutes';
```

## Production Deployment Checklist

### Pre-Deployment

- [ ] **Infrastructure Requirements**
  - [ ] PostgreSQL 15.0+
  - [ ] Sufficient disk space (2x data size for backups)
  - [ ] Connection pooling configured
  - [ ] Monitoring systems in place

- [ ] **Extension Setup**
  - [ ] pg_tviews extension installed
  - [ ] jsonb_ivm extension installed (optional but recommended)
  - [ ] Monitoring infrastructure deployed
  - [ ] Statement-level triggers installed

- [ ] **Security**
  - [ ] Database user permissions configured
  - [ ] Connection pooling authentication set up
  - [ ] Backup encryption configured
  - [ ] Network security policies in place

- [ ] **Backup Strategy**
  - [ ] Backup procedures documented
  - [ ] Backup testing completed
  - [ ] Point-in-time recovery tested
  - [ ] Backup retention policy defined

### Deployment Day

- [ ] **Pre-Deployment Validation**
  - [ ] Health checks pass: `SELECT * FROM pg_tviews_health_check()`
  - [ ] TVIEW definitions validated
  - [ ] Performance baseline established
  - [ ] Rollback plan documented

- [ ] **Deployment Steps**
  - [ ] Application traffic diverted (if needed)
  - [ ] Extension installed/upgraded
  - [ ] TVIEWs recreated (if needed)
  - [ ] Triggers reinstalled
  - [ ] Health checks pass

- [ ] **Post-Deployment Validation**
  - [ ] Application functionality verified
  - [ ] Performance meets expectations
  - [ ] Monitoring alerts configured
  - [ ] Backup procedures tested

### Ongoing Operations

- [ ] **Monitoring**
  - [ ] Health checks automated
  - [ ] Performance metrics collected
  - [ ] Alert thresholds configured
  - [ ] Backup success monitored

- [ ] **Maintenance**
  - [ ] Metrics cleanup scheduled
  - [ ] Index maintenance planned
  - [ ] Upgrade procedures documented
  - [ ] Incident response procedures ready

- [ ] **Documentation**
  - [ ] Runbooks updated
  - [ ] Contact information current
  - [ ] Escalation procedures documented
  - [ ] Post-mortem process defined

## Troubleshooting

### Common Issues

#### Extension Won't Install

**Error**: `extension "pg_tviews" does not exist`

**Solutions**:
```bash
# Check if extension is built
find /usr/share/postgresql/15/extension -name "*pg_tviews*"

# Rebuild and install
cargo pgrx install --release

# Check PostgreSQL logs
tail -f /var/log/postgresql/postgresql-15-main.log
```

#### TVIEWs Not Refreshing

**Symptoms**: Changes to base tables don't appear in TVIEWs

**Debug**:
```sql
-- Check triggers
SELECT tgname, tgrelid::regclass
FROM pg_trigger
WHERE tgname LIKE '%tview%';

-- Check queue
SELECT * FROM pg_tviews_debug_queue();

-- Test manual refresh
SELECT pg_tviews_cascade('tb_post'::regclass::oid, 123);
```

**Solutions**:
- Reinstall triggers: `SELECT pg_tviews_install_stmt_triggers();`
- Check permissions on base tables
- Verify TVIEW definitions are valid

#### Performance Degradation

**Symptoms**: Slow queries, high CPU usage

**Debug**:
```sql
-- Check cache performance
SELECT * FROM pg_tviews_cache_stats;

-- Monitor queue buildup
SELECT * FROM pg_tviews_queue_realtime;

-- Analyze slow operations
SELECT * FROM pg_tviews_performance_summary LIMIT 5;
```

**Solutions**:
- Install jsonb_ivm extension
- Use statement-level triggers for bulk operations
- Add indexes on frequently queried JSONB fields
- Increase PostgreSQL memory settings

#### Connection Pool Issues

**Symptoms**: "queue state lost" errors

**Debug**:
```sql
-- Check pool configuration
SHOW POOLS;  -- PgBouncer

-- Verify DISCARD ALL
SHOW server_reset_query;  -- PgBouncer
```

**Solutions**:
- Configure `server_reset_query = DISCARD ALL` in PgBouncer
- Use transaction pooling mode
- Consider session pinning for TVIEW operations

### Emergency Procedures

#### Complete TVIEW Recreation

```sql
-- Emergency: Drop and recreate all TVIEWs
BEGIN;

-- Extract definitions
CREATE TEMP TABLE tview_defs AS
SELECT entity, pg_get_viewdef('v_' || entity) as definition
FROM pg_tview_meta;

-- Drop all TVIEWs
SELECT pg_tviews_drop(entity, true) FROM pg_tview_meta;

-- Recreate TVIEWs
SELECT pg_tviews_create(entity, definition) FROM tview_defs;

COMMIT;
```

#### System Recovery

```sql
-- Complete system reset
BEGIN;

-- Drop extension
DROP EXTENSION pg_tviews CASCADE;

-- Recreate extension
CREATE EXTENSION pg_tviews;

-- Restore from backup
-- (Follow backup restore procedures)

COMMIT;
```

## See Also

- [API Reference](API_REFERENCE.md)
- [Monitoring Guide](MONITORING.md)
- [DDL Reference](DDL_REFERENCE.md)