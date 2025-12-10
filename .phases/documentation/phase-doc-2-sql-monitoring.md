# Phase Doc-2: SQL Functions & Monitoring Documentation

**Phase**: Documentation Phase 2
**Priority**: 🔴 CRITICAL
**Estimated Time**: 4-6 hours
**Status**: NOT STARTED

## Objective

Document all SQL monitoring functions, views, and DDL commands (CREATE/DROP TVIEW). This enables beta testers to monitor production systems and understand the complete TVIEW lifecycle.

## Context

Phase 9 implemented comprehensive monitoring infrastructure (`sql/pg_tviews_monitoring.sql`) and statement-level triggers (`sql/tview_stmt_triggers.sql`), but none of this is documented. Beta testers need this to evaluate production readiness.

## Prerequisites

- Phase Doc-1 (API Reference) completed
- Access to `sql/pg_tviews_monitoring.sql`
- Access to `sql/tview_stmt_triggers.sql`
- PostgreSQL installation for testing

## Deliverables

1. **`docs/MONITORING.md`** - Complete monitoring guide
2. **`docs/DDL_REFERENCE.md`** - CREATE/DROP TVIEW syntax reference
3. **Updated `README.md`** - Add monitoring section

## Implementation Steps

### Step 1: Create Monitoring Guide Structure (30 min)

Create `docs/MONITORING.md`:

```markdown
# pg_tviews Monitoring Guide

**Version**: 0.1.0-beta.1
**Last Updated**: [DATE]

## Overview

This guide covers monitoring, metrics, and health checking for pg_tviews in production environments.

## Quick Start

```sql
-- Check system health
SELECT * FROM pg_tviews_health_check();

-- View real-time queue activity
SELECT * FROM pg_tviews_queue_realtime;

-- Check cache performance
SELECT * FROM pg_tviews_cache_stats;
```

## Monitoring Views

### [pg_tviews_queue_realtime](#pg_tviews_queue_realtime)
### [pg_tviews_cache_stats](#pg_tviews_cache_stats)
### [pg_tviews_performance_summary](#pg_tviews_performance_summary)
### [pg_tviews_statement_stats](#pg_tviews_statement_stats)

## Monitoring Functions

### [pg_tviews_health_check()](#pg_tviews_health_check)
### [pg_tviews_record_metrics()](#pg_tviews_record_metrics)
### [pg_tviews_cleanup_metrics()](#pg_tviews_cleanup_metrics)
### [pg_tviews_debug_queue()](#pg_tviews_debug_queue)

## Metrics Collection

[How to collect and store metrics]

## Alerting

[Recommended alerts and thresholds]

## Performance Analysis

[How to analyze performance data]

## Troubleshooting

[Common monitoring issues]

## See Also

- [API Reference](API_REFERENCE.md)
- [Debugging Guide](DEBUGGING.md)
- [Operations Guide](OPERATIONS.md)
```

### Step 2: Document Monitoring Views (90 min)

Extract view definitions from `sql/pg_tviews_monitoring.sql` and document:

**View 1: pg_tviews_queue_realtime**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 4-13
-- Purpose: Real-time view of refresh queue per session/transaction
```

Documentation template:
```markdown
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
------------+----------------+------------+----------------+-------------------------
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
```

**View 2: pg_tviews_cache_stats**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 76-96
-- Purpose: Cache hit/miss statistics for performance tuning
```

**View 3: pg_tviews_performance_summary**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 99-115
-- Purpose: Hourly aggregated performance metrics
```

**View 4: pg_tviews_statement_stats**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 63-73
-- Purpose: Integration with pg_stat_statements
```

For each view, document:
- Purpose
- All columns with types and descriptions
- Example queries with sample output
- Use cases
- Performance implications
- Notes/caveats

### Step 3: Document Monitoring Functions (60 min)

**Function 1: pg_tviews_health_check()**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 135-179
-- Returns: TABLE (check_name TEXT, status TEXT, details TEXT)
```

Documentation:
```markdown
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
2. **Metadata Tables**: Checks pg_tview_meta and pg_tview_pending_refreshes
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
```

**Function 2: pg_tviews_record_metrics()**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 33-60
-- Called automatically by refresh engine
```

**Function 3: pg_tviews_cleanup_metrics(days_old INT)**
```sql
-- Location: sql/pg_tviews_monitoring.sql lines 182-195
-- Maintenance function for metrics retention
```

**Function 4: pg_tviews_debug_queue()**
```sql
-- Also document in API_REFERENCE.md
-- Cross-reference here
```

### Step 4: Document Metrics Collection (45 min)

```markdown
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
```

### Step 5: Document Alerting Thresholds (45 min)

```markdown
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
```

### Step 6: Create DDL Reference Document (60 min)

Create `docs/DDL_REFERENCE.md`:

```markdown
# pg_tviews DDL Reference

**Version**: 0.1.0-beta.1

## Overview

This document describes the DDL (Data Definition Language) commands for creating and managing TVIEWs.

## CREATE TVIEW

### Syntax

```sql
CREATE TVIEW tv_<entity> AS
SELECT ...
```

**Important**: TVIEW names must follow the `tv_*` prefix convention.

### Naming Conventions

- **TVIEW name**: `tv_<entity>` (e.g., `tv_posts`, `tv_users`)
- **Entity name**: Derived from TVIEW name by removing `tv_` prefix
- **Source tables**: `tb_<entity>` (e.g., `tb_posts`, `tb_users`)
- **Backing view**: `v_<entity>` (automatically created)

### Required Columns

The SELECT statement must include:

1. **Primary Key**: Column named `pk_<entity>` of type BIGINT or UUID
   ```sql
   p.id as pk_post  -- For tv_post
   ```

2. **JSONB Data**: Column named `data` of type JSONB
   ```sql
   jsonb_build_object(
       'id', p.id,
       'title', p.title,
       -- ... other fields
   ) as data
   ```

### Complete Example

```sql
CREATE TVIEW tv_posts AS
SELECT
    p.id as pk_post,
    p.title,
    p.content,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'name', u.name,
            'email', u.email
        ),
        'comments', COALESCE(
            jsonb_agg(
                jsonb_build_object('id', c.id, 'text', c.text)
            ) FILTER (WHERE c.id IS NOT NULL),
            '[]'::jsonb
        )
    ) as data
FROM tb_posts p
JOIN tb_users u ON p.fk_user = u.id
LEFT JOIN tb_comments c ON c.fk_post = p.id
GROUP BY p.id, p.title, p.content, u.id, u.name, u.email;
```

### What Happens

When you CREATE TVIEW:

1. **Backing View Created**: `v_posts` is created with your SELECT
2. **Materialized Table Created**: `tv_posts` stores the cached data
3. **Dependencies Detected**: Analyzes FROM/JOIN to find source tables
4. **Triggers Installed**: Automatically installs triggers on source tables
5. **Initial Refresh**: Populates `tv_posts` with current data

### Supported SQL Features

✅ **Supported**:
- JOINs (INNER, LEFT, RIGHT, FULL)
- WHERE clauses
- GROUP BY / HAVING
- jsonb_build_object()
- jsonb_agg()
- COALESCE, FILTER
- Array aggregations (ARRAY_AGG, ARRAY(...))
- Subqueries in SELECT list
- CASE expressions

❌ **Not Supported**:
- UNION / INTERSECT / EXCEPT
- WITH (CTEs) at top level
- Window functions (may work, not optimized)
- DISTINCT ON
- Self-joins (may cause issues)
- Recursive queries

### Limitations

- Maximum 10 source tables per TVIEW (Phase 7 limit)
- Circular dependencies detected and rejected
- View definition must be parseable by inference engine
- Performance degrades with >5 levels of TVIEW cascades

## DROP TVIEW

### Syntax

```sql
DROP TVIEW tv_<entity>;
```

### What Happens

When you DROP TVIEW:

1. **Triggers Removed**: Uninstalls all triggers for this TVIEW
2. **Backing View Dropped**: `v_<entity>` is dropped
3. **Materialized Table Dropped**: `tv_<entity>` is dropped
4. **Metadata Cleaned**: Entry removed from `pg_tview_meta`
5. **Dependent TVIEWs**: Must be dropped first (no CASCADE support yet)

### Example

```sql
-- Simple drop
DROP TVIEW tv_posts;

-- Check before dropping
SELECT entity, table_oid, view_oid
FROM pg_tview_meta
WHERE entity = 'post';

-- If dependencies exist, drop them first
DROP TVIEW tv_dependent_view;
DROP TVIEW tv_posts;
```

### Cascade Behavior

⚠️ **No CASCADE support in beta**: If other TVIEWs depend on this one, DROP will fail.

**Workaround**: Drop dependent TVIEWs first, then drop this one.

```sql
-- Find dependencies
SELECT entity
FROM pg_tview_meta
WHERE ... -- TODO: Add dependency query

-- Drop in reverse dependency order
DROP TVIEW tv_level3;
DROP TVIEW tv_level2;
DROP TVIEW tv_level1;
```

## ALTER TVIEW

⚠️ **Not supported in beta**: Use DROP + CREATE to modify TVIEWs.

```sql
-- To modify a TVIEW:
DROP TVIEW tv_posts;
CREATE TVIEW tv_posts AS SELECT ... -- new definition
```

## Statement-Level Triggers

### Installation

```sql
-- Install statement-level triggers for better performance
SELECT pg_tviews_install_stmt_triggers();
```

**Benefits**:
- 100-500× faster for bulk operations
- Uses transition tables (OLD/NEW tables)
- One trigger fire per statement instead of per row

**When to Use**:
- Bulk INSERT/UPDATE/DELETE operations
- Data warehouse ETL processes
- Migration scripts

### Uninstallation

```sql
-- Revert to row-level triggers
SELECT pg_tviews_uninstall_stmt_triggers();
```

**When to Uninstall**:
- Small, frequent single-row operations
- Compatibility with older PostgreSQL versions
- Debugging trigger behavior

## Troubleshooting

### CREATE TVIEW Fails

**Error**: `InvalidSelectStatement`
```sql
ERROR:  Invalid SELECT statement: [details]
```
**Solution**: Check that SELECT follows requirements (pk_*, data column, supported SQL)

**Error**: `DependencyCycle`
```sql
ERROR:  Dependency cycle detected: post -> comment -> post
```
**Solution**: TVIEWs cannot have circular dependencies. Restructure dependencies.

### DROP TVIEW Fails

**Error**: `DependentObjectsExist`
```sql
ERROR:  Cannot drop tv_posts: other TVIEWs depend on it
```
**Solution**: Drop dependent TVIEWs first.

## See Also

- [API Reference](API_REFERENCE.md)
- [Operations Guide](OPERATIONS.md)
- [Debugging Guide](DEBUGGING.md)
```

### Step 7: Update README.md (30 min)

Add to README.md:

```markdown
## Monitoring

pg_tviews provides comprehensive monitoring for production deployments.

### Quick Monitoring

```sql
-- System health check
SELECT * FROM pg_tviews_health_check();

-- Real-time queue activity
SELECT * FROM pg_tviews_queue_realtime;

-- Cache performance
SELECT * FROM pg_tviews_cache_stats;

-- Performance trends
SELECT * FROM pg_tviews_performance_summary;
```

### Monitoring Documentation

For complete monitoring guide, see [Monitoring Guide](docs/MONITORING.md).

**Key Topics**:
- [Monitoring Views](docs/MONITORING.md#monitoring-views) - Real-time metrics
- [Health Checks](docs/MONITORING.md#monitoring-functions) - System health
- [Metrics Collection](docs/MONITORING.md#metrics-collection) - Historical data
- [Alerting](docs/MONITORING.md#recommended-alerts) - Thresholds and alerts
- [Performance Analysis](docs/MONITORING.md#performance-analysis) - Tuning guide

## DDL Reference

For complete CREATE/DROP TVIEW syntax, see [DDL Reference](docs/DDL_REFERENCE.md).

**Quick Reference**:
```sql
-- Create a TVIEW
CREATE TVIEW tv_posts AS
SELECT p.id as pk_post, ... as data
FROM tb_posts p ...;

-- Drop a TVIEW
DROP TVIEW tv_posts;

-- Install statement-level triggers (100-500× faster)
SELECT pg_tviews_install_stmt_triggers();
```
```

## Verification Steps

### 1. Test All Monitoring Views
```sql
-- Create test database
CREATE DATABASE tview_mon_test;
\c tview_mon_test
CREATE EXTENSION pg_tviews;

-- Test each view
SELECT * FROM pg_tviews_queue_realtime;
SELECT * FROM pg_tviews_cache_stats;
SELECT * FROM pg_tviews_performance_summary;
SELECT * FROM pg_tviews_statement_stats;

-- Test health check
SELECT * FROM pg_tviews_health_check();
```

### 2. Test Metrics Collection
```sql
-- Generate some metrics
CREATE TVIEW tv_test AS
SELECT 1 as pk_test, '{}'::jsonb as data;

INSERT INTO tb_source VALUES (1, 'test');
COMMIT;

-- Verify metrics recorded
SELECT * FROM pg_tviews_metrics ORDER BY recorded_at DESC LIMIT 5;

-- Test cleanup
SELECT pg_tviews_cleanup_metrics(0);  -- Delete all
```

### 3. Validate DDL Documentation
```sql
-- Test CREATE TVIEW with all documented features
CREATE TVIEW tv_example AS
SELECT ...;  -- Use example from DDL_REFERENCE.md

-- Verify it works
SELECT * FROM tv_example LIMIT 1;

-- Test DROP TVIEW
DROP TVIEW tv_example;

-- Verify cleanup
SELECT * FROM pg_tview_meta WHERE entity = 'example';  -- Should be empty
```

### 4. Test Statement-Level Triggers
```sql
-- Install
SELECT pg_tviews_install_stmt_triggers();

-- Verify installed
SELECT * FROM pg_tviews_health_check()
WHERE check_name = 'statement_triggers';

-- Uninstall
SELECT pg_tviews_uninstall_stmt_triggers();
```

## Acceptance Criteria

Phase Doc-2 is complete when:

- ✅ `docs/MONITORING.md` exists and documents:
  - [ ] All 4 monitoring views with examples
  - [ ] All monitoring functions
  - [ ] Metrics collection process
  - [ ] Recommended alerting thresholds
  - [ ] Performance analysis examples
- ✅ `docs/DDL_REFERENCE.md` exists and documents:
  - [ ] CREATE TVIEW full syntax
  - [ ] DROP TVIEW syntax
  - [ ] Naming conventions
  - [ ] Supported/unsupported SQL features
  - [ ] Limitations
  - [ ] Statement-level trigger management
- ✅ README.md updated with:
  - [ ] Monitoring section with quick examples
  - [ ] DDL reference section
  - [ ] Links to detailed documentation
- ✅ All examples tested and verified
- ✅ All views/functions return expected results
- ✅ Documentation reviewed for accuracy

## Success Metrics

- Beta testers can monitor production systems without asking questions
- Beta testers understand CREATE/DROP TVIEW syntax completely
- No ambiguity about what SQL features are supported
- Alert thresholds prevent false positives

## Dependencies

- `sql/pg_tviews_monitoring.sql`
- `sql/tview_stmt_triggers.sql`
- Test database for verification

## Estimated Breakdown

- Step 1 (Structure): 30 min
- Step 2 (Views): 90 min
- Step 3 (Functions): 60 min
- Step 4 (Metrics): 45 min
- Step 5 (Alerting): 45 min
- Step 6 (DDL Reference): 60 min
- Step 7 (README): 30 min
- Verification: 60 min

**Total**: 4-6 hours

## Next Phase

After completing Phase Doc-2, proceed to:
→ **Phase Doc-3**: Operations Guide (Backup, Restore, Connection Pooling)
