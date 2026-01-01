# A+ Documentation Quality Plan - Part 2

*Continuation of APLUS_DOCUMENTATION_PLAN.md*

---

## Phase C: Operational Excellence (continued)

### C3: Production Deployment Checklist (4 hours)

**Objective**: Create comprehensive pre-production checklist.

**Content**:

```markdown
# Production Deployment Checklist

## Pre-Deployment

### Environment Verification

- [ ] PostgreSQL version 15+ installed
- [ ] pg_tviews extension built for production
  ```bash
  cargo pgrx install --release --pg-version
  ```
- [ ] jsonb_delta extension installed (optional but recommended)
- [ ] Sufficient disk space (estimate 2× data size)
- [ ] Memory allocation adequate (check work_mem, shared_buffers)
- [ ] Connection pooler configured (PgBouncer/pgpool-II)

### Schema Preparation

- [ ] All source tables follow trinity pattern
  - pk_{entity} columns
  - id (UUID) columns
  - fk_{parent} columns
- [ ] Foreign key constraints defined
- [ ] Indexes on source tables optimized
- [ ] Row-level security policies reviewed (if using RLS)

### TVIEW Definition Review

- [ ] All TVIEW definitions in version control
- [ ] Dependencies mapped (no cycles)
- [ ] Cascade depth reasonable (<5 levels)
- [ ] JSONB structure matches application needs
- [ ] All required columns present

### Security

- [ ] Roles and permissions configured
  ```sql
  CREATE ROLE tview_admin;
  CREATE ROLE tview_reader;
  GRANT ...
  ```
- [ ] SSL/TLS enabled for connections
- [ ] Sensitive data excluded from JSONB
- [ ] Audit logging configured

### Performance

- [ ] Baseline performance metrics captured
- [ ] Expected QPS (queries per second) documented
- [ ] Load testing completed
- [ ] Indexes on tv_* tables planned
- [ ] Statement-level triggers decision made

---

## During Deployment

### Installation Steps

1. **Install Extension** (with downtime):
   ```bash
   # 1. Schedule maintenance window
   # 2. Stop application writes
   # 3. Install extension
   psql -d production -c "CREATE EXTENSION pg_tviews;"
   # 4. Verify
   psql -d production -c "SELECT pg_tviews_version();"
   ```

2. **Create TVIEWs**:
   ```bash
   # Run from version-controlled file
   psql -d production -f tview_definitions.sql
   ```

3. **Verify Installation**:
   ```sql
   -- Check all TVIEWs created
   SELECT entity FROM pg_tview_meta ORDER BY entity;

   -- Health check
   SELECT * FROM pg_tviews_health_check();

   -- Test query
   SELECT * FROM tv_posts LIMIT 5;
   ```

4. **Enable Statement Triggers** (if needed):
   ```sql
   SELECT pg_tviews_install_stmt_triggers();
   ```

5. **Create Indexes**:
   ```sql
   CREATE INDEX idx_tv_post_id ON tv_post(id);
   CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);
   CREATE INDEX idx_tv_post_created ON tv_post USING gin((data->'createdAt'));
   ```

---

## Post-Deployment

### Immediate Verification (First Hour)

- [ ] All TVIEWs responding to queries
- [ ] Write operations updating TVIEWs
- [ ] No error spikes in logs
- [ ] Performance within SLA
- [ ] Cache hit rates >80%

**Verification Commands**:
```sql
-- Test write → read cycle
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES ('Test Post', 1);
SELECT * FROM tv_post WHERE title = 'Test Post';
ROLLBACK;

-- Check metrics
SELECT * FROM pg_tviews_queue_realtime;
SELECT * FROM pg_tviews_cache_stats;
```

### Short-Term Monitoring (First 24 Hours)

- [ ] Monitor refresh latency
  ```sql
  SELECT AVG(refresh_duration_ms), MAX(refresh_duration_ms)
  FROM pg_tviews_performance_summary
  WHERE hour > now() - interval '1 hour';
  ```
- [ ] Monitor queue size
  ```sql
  SELECT MAX(queue_size) FROM pg_tviews_performance_summary
  WHERE hour > now() - interval '24 hours';
  ```
- [ ] Check error logs
  ```bash
  grep -i "pg_tviews" /var/log/postgresql/postgresql-*.log
  ```
- [ ] Verify data consistency
  ```sql
  -- Spot-check: TVIEW data matches source
  SELECT COUNT(*) FROM tv_post;
  SELECT COUNT(*) FROM tb_post;
  ```

### Medium-Term Validation (First Week)

- [ ] Performance baselines established
- [ ] Alert thresholds tuned
- [ ] No memory leaks detected
- [ ] Cache performance stable (>90% hit rate)
- [ ] Application team trained
- [ ] Runbooks documented

### Long-Term Health (First Month)

- [ ] Disaster recovery tested
- [ ] Backup/restore verified
- [ ] Capacity planning updated
- [ ] Performance trends analyzed
- [ ] Optimization opportunities identified

---

## Monitoring Setup

### Essential Metrics

```sql
-- Add to your monitoring system (Prometheus, Datadog, etc.)

-- Queue size (should be <50)
SELECT COUNT(*) AS queue_size FROM pg_tviews_queue_realtime;

-- Cache hit rate (should be >90%)
SELECT
    (SUM(cache_hits)::float / NULLIF(SUM(cache_hits + cache_misses), 0) * 100) AS hit_rate
FROM pg_tviews_cache_stats;

-- Refresh latency (should be <10ms)
SELECT
    AVG(refresh_duration_ms) AS avg_latency_ms,
    MAX(refresh_duration_ms) AS max_latency_ms,
    PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY refresh_duration_ms) AS p95_latency_ms
FROM pg_tviews_performance_summary
WHERE hour > now() - interval '1 hour';
```

### Alerting Rules

```yaml
# Example Prometheus alerts
groups:
  - name: pg_tviews
    rules:
      - alert: TViewHighLatency
        expr: pg_tviews_avg_latency_ms > 50
        for: 5m
        severity: warning

      - alert: TViewCriticalLatency
        expr: pg_tviews_p95_latency_ms > 500
        for: 2m
        severity: critical

      - alert: TViewLowCacheHitRate
        expr: pg_tviews_cache_hit_rate < 80
        for: 10m
        severity: warning

      - alert: TViewQueueBacklog
        expr: pg_tviews_queue_size > 100
        for: 5m
        severity: warning
```

---

## Rollback Plan

If deployment fails, follow this rollback procedure:

### Quick Rollback (Keep TVIEWs)

```sql
-- 1. Stop using TVIEWs in application
-- (revert application deployment)

-- 2. Keep extension installed for next attempt
-- No database changes needed
```

### Full Rollback (Remove Extension)

```sql
-- 1. Drop all TVIEWs
DROP EXTENSION pg_tviews CASCADE;

-- 2. Restore to traditional MVs (if migrating)
\i restore_materialized_views.sql

-- 3. Update application to use MVs
-- (revert application deployment)

-- 4. Resume cron jobs for MV refresh
```

### Data Loss Check

```sql
-- Verify no data lost (source tables unchanged)
SELECT COUNT(*) FROM tb_user;
SELECT COUNT(*) FROM tb_post;
-- Counts should match pre-deployment
```

---

## Common Deployment Issues

### Issue: Extension Fails to Load

**Symptom**:
```
ERROR: could not load library "pg_tviews"
```

**Fix**:
```bash
# Rebuild for correct PostgreSQL version
cargo pgrx install --release --pg17

# Restart PostgreSQL
sudo systemctl restart postgresql
```

### Issue: TVIEWs Not Updating

**Symptom**: Queries return stale data

**Fix**:
```sql
-- Check triggers installed
SELECT * FROM pg_trigger WHERE tgname LIKE 'tview%';

-- Check health
SELECT * FROM pg_tviews_health_check();

-- Manual refresh to test
SELECT pg_tviews_cascade('tb_post'::regclass::oid, 1);
```

### Issue: Performance Worse Than Expected

**Symptom**: Slow query response times

**Fix**:
```sql
-- 1. Enable statement triggers
SELECT pg_tviews_install_stmt_triggers();

-- 2. Add indexes
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- 3. Check cascade depth
SELECT MAX(depth) FROM pg_tviews_queue_realtime;
-- Should be <5

-- 4. Verify jsonb_delta installed
SELECT pg_tviews_check_jsonb_delta();
```

---

## Stakeholder Communication

### Pre-Deployment Email Template

```
Subject: pg_tviews Production Deployment - [Date]

Team,

We will be deploying pg_tviews to production on [DATE] at [TIME].

**What is pg_tviews?**
Incremental materialized view refresh system for always-fresh data.

**Deployment Window:**
[START TIME] - [END TIME] ([DURATION])

**Expected Impact:**
- [X] minutes downtime for installation
- Writes will be paused during deployment
- Reads continue on read replicas

**Benefits:**
- [X]× faster query performance
- Always-fresh data (no stale reads)
- Eliminated cron refresh jobs

**Rollback Plan:**
If issues arise, we can rollback in [Y] minutes.

**Monitoring:**
Metrics dashboard: [LINK]
On-call: [CONTACT]

Please report any issues to [EMAIL/SLACK].

Thank you,
[NAME]
```

### Post-Deployment Report Template

```
Subject: pg_tviews Production Deployment - Success

Team,

pg_tviews deployment completed successfully.

**Deployment Summary:**
- Start: [TIME]
- End: [TIME]
- Actual downtime: [DURATION]

**Results:**
- All [N] TVIEWs created successfully
- Query performance: [X]× improvement
- Cache hit rate: [Y]%
- Zero errors in first hour

**Monitoring:**
- Dashboard: [LINK]
- All metrics green
- Alerts configured

**Next Steps:**
- Continue monitoring for 24 hours
- Performance review meeting: [DATE]
- Team training session: [DATE]

Thank you for your support!

[NAME]
```

---

## Success Criteria

Deployment is successful when:

- [ ] All TVIEWs created without errors
- [ ] Health check returns all "OK"
- [ ] Queries return correct data
- [ ] Performance meets or exceeds SLA
- [ ] No application errors
- [ ] Monitoring dashboards green
- [ ] Team trained and confident
- [ ] Documentation updated

---

## Appendix: Environment-Specific Checklists

### Development Environment

- [ ] Extension installed from source
- [ ] Sample data loaded
- [ ] All TVIEWs created
- [ ] Basic tests pass
- [ ] Local monitoring setup

### Staging Environment

- [ ] Production-like data volume
- [ ] Load testing completed
- [ ] Failover tested
- [ ] Backup/restore verified
- [ ] Team trained on staging

### Production Environment

- [ ] All items from "Deployment Checklist" above
- [ ] Change management approval
- [ ] Rollback plan tested
- [ ] On-call schedule confirmed
- [ ] Stakeholders notified
```

**Deliverables**:
- Complete production deployment checklist
- Rollback procedures
- Monitoring setup guide
- Communication templates

**Acceptance Criteria**:
- [ ] All deployment steps documented
- [ ] Rollback procedures tested
- [ ] Monitoring configured
- [ ] Communication templates ready
- [ ] Success criteria defined

---

### C4: Performance Tuning Guide (6 hours)

**Objective**: Comprehensive guide to optimizing pg_tviews for different workloads.

**Content**:

```markdown
# Performance Tuning Guide

## Performance Tuning Overview

pg_tviews performance depends on:
1. Workload characteristics (read-heavy, write-heavy, bulk)
2. Schema design (cascade depth, JSONB size)
3. Configuration (caching, triggers, indexes)
4. Hardware (CPU, memory, disk I/O)

This guide helps you optimize for your specific workload.

---

## Workload Profiles

### Profile 1: Read-Heavy (90% reads, 10% writes)

**Characteristics**:
- Frequent queries on TVIEWs
- Infrequent updates to source tables
- Latency-sensitive applications

**Optimization Strategy**:

```sql
-- 1. Maximize query performance
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);
CREATE INDEX idx_tv_post_created ON tv_post USING gin((data->'createdAt'));

-- 2. Use native PostgreSQL caching
-- No special pg_tviews config needed

-- 3. Consider read replicas for query offloading
-- TVIEWs replicate automatically
```

**Expected Performance**:
- Query latency: <5ms
- Refresh latency: <10ms (infrequent)
- Cache hit rate: >95%

---

### Profile 2: Write-Heavy (10% reads, 90% writes)

**Characteristics**:
- Frequent updates to source tables
- High transaction volume
- Real-time data required

**Optimization Strategy**:

```sql
-- 1. Enable statement-level triggers for bulk operations
SELECT pg_tviews_install_stmt_triggers();

-- 2. Minimize indexes on TVIEWs (slower writes)
-- Only index what you query frequently
CREATE INDEX idx_tv_post_id ON tv_post(id);  -- Essential only

-- 3. Consider batching writes
-- Accumulate changes, commit in larger transactions

-- 4. Monitor queue size
SELECT * FROM pg_tviews_queue_realtime;
```

**Expected Performance**:
- Write throughput: 1000+ ops/sec
- Refresh latency: <2ms per row
- Queue size: <50 during normal operation

**postgresql.conf tuning**:
```ini
# Increase these for write-heavy workloads
work_mem = 64MB  # Larger for sorting during refresh
maintenance_work_mem = 256MB
shared_buffers = 4GB  # 25% of RAM
```

---

### Profile 3: Bulk Operations (Batch ETL, Data Migrations)

**Characteristics**:
- Large batch inserts/updates (1000+ rows)
- Periodic rather than continuous
- Can tolerate latency

**Optimization Strategy**:

```sql
-- 1. MUST enable statement-level triggers
SELECT pg_tviews_install_stmt_triggers();

-- 2. Increase batch thresholds
-- (if configurable in future versions)

-- 3. Consider partitioning large tables
-- pg_tviews works with partitioned tables

-- 4. Run during off-peak hours
```

**Example Batch Operation**:
```sql
BEGIN;

-- Insert 10,000 rows
COPY tb_post FROM '/data/posts.csv' CSV;

-- Statement trigger fires once (not 10,000 times)
-- Batch refresh processes all rows efficiently

COMMIT;
```

**Expected Performance**:
- Batch insert: 10,000 rows in 1-2 seconds
- TVIEW refresh: 100-500ms for full batch
- 100-500× faster than row-level triggers

---

### Profile 4: Balanced (50% reads, 50% writes)

**Characteristics**:
- Mixed workload
- Moderate transaction volume
- Balanced optimization needed

**Optimization Strategy**:

```sql
-- 1. Enable statement triggers for bulk ops
SELECT pg_tviews_install_stmt_triggers();

-- 2. Selective indexing
-- Index high-cardinality columns
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);

-- 3. Monitor and tune based on metrics
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours';
```

**Expected Performance**:
- Query latency: <10ms
- Write latency: <5ms
- Cache hit rate: >85%

---

## Configuration Tuning

### Statement-Level Triggers

**When to Enable**:
- Bulk operations (>10 rows per transaction)
- ETL pipelines
- Batch processing

**When to Disable**:
- High-frequency single-row updates
- Interactive applications
- Low-latency requirements

**Toggle**:
```sql
-- Enable
SELECT pg_tviews_install_stmt_triggers();

-- Disable
SELECT pg_tviews_uninstall_stmt_triggers();
```

**Performance Impact**:
- Bulk insert of 1000 rows:
  - Without: ~1000ms (1ms per row)
  - With: ~10ms (statement-level batch)
  - **100× improvement**

---

### Cache Tuning

**Graph Cache**:
Stores dependency graphs for faster cascade resolution.

```ini
# postgresql.conf (if exposed in future)
pg_tviews.graph_cache_size = 200  # Number of graphs to cache
```

**Table OID Cache**:
Maps table names to internal OIDs.

```ini
pg_tviews.table_cache_size = 1000  # Number of mappings
```

**Query Plan Cache**:
Caches prepared statements for refresh queries.

**Monitor Cache Performance**:
```sql
SELECT
    cache_type,
    hits,
    misses,
    (hits::float / NULLIF(hits + misses, 0) * 100)::numeric(5,2) AS hit_rate_pct
FROM pg_tviews_cache_stats;
```

**Target Hit Rates**:
- Graph cache: >90%
- Table cache: >95%
- Plan cache: >85%

**If hit rate is low**:
1. Increase cache sizes (if configurable)
2. Reduce schema volatility (fewer ALTER TABLE)
3. Wait for warm-up period (10-15 minutes after restart)

---

### Cascade Depth Limits

**Problem**: Deep cascades can cause performance issues.

**Example of Deep Cascade**:
```
company → division → department → team → user → post → comment
(7 levels deep)
```

**Tuning**:
```ini
# Prevent runaway cascades
pg_tviews.max_cascade_depth = 5  # Default: 10
```

**Best Practice**:
- Keep cascade depth ≤ 3 levels
- Flatten unnecessary intermediate levels
- Denormalize if needed

---

## Indexing Strategies

### Essential Indexes

**Always Index**:
```sql
-- UUID lookup (most common query)
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- Foreign key filtering
CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);
```

### JSONB Indexes

**For Text Search**:
```sql
-- Full-text search on title
CREATE INDEX idx_tv_post_title_gin
ON tv_post
USING gin(to_tsvector('english', data->>'title'));

-- Query:
SELECT * FROM tv_post
WHERE to_tsvector('english', data->>'title') @@ to_tsquery('postgres');
```

**For Range Queries**:
```sql
-- Date range queries
CREATE INDEX idx_tv_post_created_at
ON tv_post
USING btree((data->>'createdAt'));

-- Query:
SELECT * FROM tv_post
WHERE data->>'createdAt' > '2025-01-01';
```

**For Nested Fields**:
```sql
-- Index nested author name
CREATE INDEX idx_tv_post_author_name
ON tv_post
USING gin((data->'author'->>'name'));

-- Query:
SELECT * FROM tv_post
WHERE data->'author'->>'name' ILIKE '%john%';
```

### Index Trade-offs

**More Indexes**:
- ✅ Faster queries
- ❌ Slower writes
- ❌ More disk space

**Fewer Indexes**:
- ✅ Faster writes
- ✅ Less disk space
- ❌ Slower queries

**Rule of Thumb**:
- Read-heavy: More indexes
- Write-heavy: Fewer indexes
- Balanced: Index only frequent queries

---

## Query Optimization

### Avoid Sequential Scans

**❌ Slow (no index)**:
```sql
SELECT * FROM tv_post
WHERE data->>'title' ILIKE '%postgres%';
-- Seq Scan on tv_post (cost=0.00..1000.00)
```

**✅ Fast (with index)**:
```sql
-- First, create index:
CREATE INDEX idx_tv_post_title_gin
ON tv_post
USING gin(to_tsvector('english', data->>'title'));

-- Then query:
SELECT * FROM tv_post
WHERE to_tsvector('english', data->>'title') @@ to_tsquery('postgres');
-- Bitmap Index Scan on idx_tv_post_title_gin (cost=5.00..10.00)
```

### Limit Result Sets

```sql
-- Always use LIMIT for large result sets
SELECT * FROM tv_post
ORDER BY data->>'createdAt' DESC
LIMIT 100;
```

### Use Prepared Statements

```javascript
// Reuse query plans
const query = {
    name: 'get-post-by-id',
    text: 'SELECT data FROM tv_post WHERE id = $1',
    values: [postId]
};

// First execution: plan created and cached
// Subsequent executions: plan reused (faster)
```

---

## Hardware Considerations

### CPU

**Impact**: Refresh performance, query throughput

**Recommendations**:
- 4+ cores for small deployments (<10K rows)
- 8+ cores for medium deployments (<1M rows)
- 16+ cores for large deployments (>1M rows)

**Tuning**:
```ini
# Use all available cores
max_worker_processes = 8
max_parallel_workers = 8
max_parallel_workers_per_gather = 4
```

### Memory

**Impact**: Cache performance, sort operations

**Recommendations**:
- 4GB+ RAM for small deployments
- 16GB+ RAM for medium deployments
- 64GB+ RAM for large deployments

**Tuning**:
```ini
# Allocate 25% of RAM to PostgreSQL
shared_buffers = 4GB  # For 16GB total RAM

# Per-query memory
work_mem = 64MB
maintenance_work_mem = 256MB

# Cache effectiveness
effective_cache_size = 12GB  # 75% of RAM
```

### Disk I/O

**Impact**: Write throughput, refresh latency

**Recommendations**:
- SSD (NVMe preferred) for TVIEWs
- Separate volumes for WAL and data
- RAID 10 for redundancy + performance

**Tuning**:
```ini
# Write-ahead log optimization
wal_buffers = 16MB
checkpoint_timeout = 15min
checkpoint_completion_target = 0.9

# Async I/O
effective_io_concurrency = 200  # For SSD
```

---

## Monitoring & Profiling

### Identify Slow Queries

```sql
-- Install pg_stat_statements
CREATE EXTENSION pg_stat_statements;

-- Find slow TVIEW queries
SELECT
    query,
    calls,
    mean_exec_time,
    max_exec_time
FROM pg_stat_statements
WHERE query LIKE '%tv_%'
ORDER BY mean_exec_time DESC
LIMIT 10;
```

### Profile Refresh Performance

```sql
-- Enable timing
\timing on

-- Test single-row refresh
BEGIN;
UPDATE tb_post SET title = 'Test' WHERE pk_post = 1;
-- Note time (includes TVIEW refresh)
ROLLBACK;

-- Test bulk refresh
BEGIN;
UPDATE tb_post SET title = 'Bulk' WHERE pk_post <= 100;
-- Note time
ROLLBACK;
```

### Analyze Query Plans

```sql
EXPLAIN ANALYZE
SELECT * FROM tv_post WHERE id = 'uuid-here';

-- Look for:
-- - Index Scan (good) vs Seq Scan (bad)
-- - Execution time
-- - Rows scanned vs returned
```

---

## Real-World Optimization Examples

### Example 1: Slow Query After Adding TVIEW

**Problem**: Query that was fast on tb_* is slow on tv_*

**Diagnosis**:
```sql
EXPLAIN SELECT * FROM tv_post WHERE data->>'userId' = '123';
-- Seq Scan on tv_post (slow)
```

**Solution**:
```sql
-- Add index on filtered field
CREATE INDEX idx_tv_post_user_id ON tv_post((data->>'userId'));

EXPLAIN SELECT * FROM tv_post WHERE data->>'userId' = '123';
-- Index Scan using idx_tv_post_user_id (fast)
```

---

### Example 2: High Memory Usage During Bulk Insert

**Problem**: Memory usage spikes during ETL

**Diagnosis**:
```sql
SELECT * FROM pg_tviews_queue_realtime;
-- queue_size: 50,000 rows (huge!)
```

**Solution**:
```sql
-- 1. Enable statement triggers (batching)
SELECT pg_tviews_install_stmt_triggers();

-- 2. Reduce transaction size
-- Instead of:
BEGIN;
INSERT INTO tb_post ... 100,000 rows;
COMMIT;

-- Do:
FOR i IN 1..10 LOOP
    BEGIN;
    INSERT INTO tb_post ... 10,000 rows;
    COMMIT;
END LOOP;
```

---

### Example 3: Low Cache Hit Rate

**Problem**: Cache hit rate <60%

**Diagnosis**:
```sql
SELECT * FROM pg_tviews_cache_stats;
-- graph_cache_hits: 100
-- graph_cache_misses: 150
-- Hit rate: 40% (bad)
```

**Possible Causes**:
1. Recently restarted (warmup needed)
2. High schema volatility (ALTER TABLE frequently)
3. Cache size too small

**Solution**:
```sql
-- Wait 15 minutes after restart for warmup

-- If persistent:
-- 1. Reduce schema changes
-- 2. Increase cache size (if configurable)
pg_tviews.graph_cache_size = 500;  -- Increase from 200
```

---

## Troubleshooting Performance

### Performance Decision Tree

```
Is query slow?
├─ YES
│  └─ Run EXPLAIN ANALYZE
│     ├─ Seq Scan?
│     │  └─ Add index
│     ├─ Large result set?
│     │  └─ Add LIMIT
│     └─ Complex JOIN?
│        └─ Denormalize more in JSONB
└─ NO
   └─ Is refresh slow?
      ├─ YES
      │  └─ Check refresh metrics
      │     ├─ High cascade depth?
      │     │  └─ Flatten schema
      │     ├─ Large queue?
      │     │  └─ Enable statement triggers
      │     └─ Large JSONB?
      │        └─ Install jsonb_delta
      └─ NO
         └─ Performance OK ✓
```

### Common Anti-Patterns

**❌ Querying TVIEWs Without Indexes**:
```sql
-- 100M row table, no index
SELECT * FROM tv_product WHERE data->>'category' = 'Electronics';
-- Takes 30 seconds
```

**✅ Fix**: Add index
```sql
CREATE INDEX idx_tv_product_category
ON tv_product((data->>'category'));
-- Now takes 10ms
```

---

**❌ Deep Cascade Chains**:
```
company → division → dept → team → user → post → comment → like
```

**✅ Fix**: Flatten intermediate levels
```
company → user → post → comment
```

---

**❌ Not Using Statement Triggers for Bulk Ops**:
```sql
-- Without statement triggers:
INSERT INTO tb_post SELECT * FROM staging;  -- 10,000 rows
-- 10,000 × row-level triggers = 10 seconds
```

**✅ Fix**:
```sql
SELECT pg_tviews_install_stmt_triggers();
-- Now: 1 × statement-level trigger = 100ms
```

---

## Benchmarking Checklist

Before deploying to production:

- [ ] Benchmark single-row updates
- [ ] Benchmark bulk operations (100, 1K, 10K rows)
- [ ] Benchmark query performance on TVIEWs
- [ ] Measure cascade propagation time
- [ ] Test under load (concurrent users)
- [ ] Measure cache hit rates
- [ ] Profile memory usage
- [ ] Test failover scenarios

**Sample Benchmark Results Table**:

| Operation | Volume | Latency | Throughput |
|-----------|--------|---------|------------|
| Single insert | 1 row | 1.2 ms | 833 ops/sec |
| Bulk insert | 1K rows | 100 ms | 10K rows/sec |
| Query by ID | 1 row | 0.5 ms | 2K queries/sec |
| Query with filter | 100 rows | 5 ms | 200 queries/sec |
| Cascade (depth 3) | 1 row | 2 ms | 500 ops/sec |

---

## See Also

- [Monitoring Guide](monitoring.md)
- [Operations Guide](operations.md)
- [Benchmark Results](../benchmarks/results.md)
```

**Deliverables**:
- Complete performance tuning guide
- Workload-specific optimizations
- Configuration tuning reference
- Real-world optimization examples
- Performance troubleshooting decision tree

**Acceptance Criteria**:
- [ ] All workload profiles documented
- [ ] Configuration tuning explained
- [ ] Indexing strategies provided
- [ ] Real-world examples included
- [ ] Troubleshooting decision tree complete

---

## Phase D: Learning & Onboarding (16-24 hours)

### D1: Interactive Tutorials (8 hours)

**Objective**: Create step-by-step tutorials for common tasks.

**Tutorials to Create**:

1. **Tutorial 1: Your First TVIEW** (Beginner)
2. **Tutorial 2: Migrating from Materialized Views** (Intermediate)
3. **Tutorial 3: Building a Blog with pg_tviews** (Intermediate)
4. **Tutorial 4: E-commerce Product Catalog** (Advanced)
5. **Tutorial 5: GraphQL Integration** (Advanced)

**Example Structure** (Tutorial 1):

```markdown
# Tutorial 1: Your First TVIEW

**Time**: 15 minutes
**Level**: Beginner
**Prerequisites**: PostgreSQL 15+ installed

## What You'll Learn

- How to install pg_tviews
- Creating your first TVIEW
- Testing automatic refresh
- Querying JSONB data

## Setup

1. **Install extension**:
   ```bash
   cargo install --locked cargo-pgrx
   cargo pgrx init
   git clone https://github.com/your-org/pg_tviews.git
   cd pg_tviews
   cargo pgrx install --release
   ```

2. **Create database**:
   ```bash
   createdb tutorial
   psql tutorial -c "CREATE EXTENSION pg_tviews;"
   ```

3. **Verify installation**:
   ```bash
   psql tutorial -c "SELECT pg_tviews_version();"
   ```

Expected output:
```
 pg_tviews_version
-------------------
 0.1.0-beta.1
```

✅ **Checkpoint 1**: Extension installed successfully

## Step 1: Create Source Tables

Following the trinity pattern, create a simple blog schema:

```sql
psql tutorial <<'EOF'
-- Users table
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Posts table
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
EOF
```

✅ **Checkpoint 2**: Source tables created

## Step 2: Insert Sample Data

```sql
psql tutorial <<'EOF'
-- Create a user
INSERT INTO tb_user (identifier, name, email)
VALUES ('alice', 'Alice Johnson', 'alice@example.com');

-- Create some posts
INSERT INTO tb_post (identifier, title, content, fk_user)
VALUES
    ('hello-world', 'Hello World', 'Welcome to my blog!', 1),
    ('second-post', 'Second Post', 'Learning about pg_tviews.', 1);
EOF
```

Verify:
```sql
psql tutorial -c "SELECT name, title FROM tb_user u JOIN tb_post p ON u.pk_user = p.fk_user;"
```

Expected output:
```
     name      |    title
---------------+--------------
 Alice Johnson | Hello World
 Alice Johnson | Second Post
```

✅ **Checkpoint 3**: Sample data loaded

## Step 3: Create Your First TVIEW

Now create a TVIEW that materializes posts with author information:

```sql
psql tutorial <<'EOF'
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.identifier,
    p.fk_user,
    u.id AS user_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'createdAt', p.created_at,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        )
    ) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
EOF
```

✅ **Checkpoint 4**: TVIEW created

## Step 4: Query the TVIEW

```sql
psql tutorial -c "SELECT data FROM tv_post;"
```

Expected output (pretty-printed):
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "identifier": "hello-world",
  "title": "Hello World",
  "content": "Welcome to my blog!",
  "createdAt": "2025-12-11T10:00:00Z",
  "author": {
    "id": "650e8400-e29b-41d4-a716-446655440001",
    "identifier": "alice",
    "name": "Alice Johnson",
    "email": "alice@example.com"
  }
}
```

✅ **Checkpoint 5**: TVIEW returns data

## Step 5: Test Automatic Refresh

This is the magic! Update the source table and see TVIEW update automatically:

```sql
psql tutorial <<'EOF'
-- Update user's name
UPDATE tb_user SET name = 'Alice Smith' WHERE pk_user = 1;

-- Query TVIEW immediately
SELECT data->'author'->>'name' AS author_name FROM tv_post;
EOF
```

Expected output:
```
 author_name
-------------
 Alice Smith
 Alice Smith
```

**Notice**: The TVIEW updated automatically! No manual REFRESH needed.

✅ **Checkpoint 6**: Automatic refresh confirmed

## Step 6: Test INSERT

```sql
psql tutorial <<'EOF'
-- Insert new post
INSERT INTO tb_post (identifier, title, content, fk_user)
VALUES ('third-post', 'Third Post', 'This is automatically in the TVIEW!', 1);

-- Check TVIEW
SELECT COUNT(*) FROM tv_post;
EOF
```

Expected output:
```
 count
-------
     3
```

The new post is automatically in the TVIEW!

✅ **Checkpoint 7**: INSERT refresh confirmed

## Congratulations! 🎉

You've successfully:
- ✅ Created a TVIEW with the trinity pattern
- ✅ Verified automatic refresh on UPDATE
- ✅ Verified automatic refresh on INSERT
- ✅ Queried JSONB read models

## Next Steps

- Try [Tutorial 2: Migrating from Materialized Views](tutorial-2-migration.md)
- Learn about [Performance Tuning](../operations/performance-tuning.md)
- Explore [Advanced Patterns](../user-guides/developers.md)

## Cleanup

To remove the tutorial database:
```bash
dropdb tutorial
```

## Troubleshooting

**Problem**: TVIEW not updating

**Solution**:
```sql
-- Check triggers installed
SELECT * FROM pg_trigger WHERE tgname LIKE 'tview%';

-- Check health
SELECT * FROM pg_tviews_health_check();
```

**Problem**: Extension not found

**Solution**:
```bash
# Reinstall
cargo pgrx install --release
psql -c "CREATE EXTENSION pg_tviews;"
```
```

**Deliverables**:
- 5 interactive tutorials (beginner to advanced)
- Each with step-by-step checkpoints
- Sample code and expected output
- Troubleshooting sections

**Acceptance Criteria**:
- [ ] All 5 tutorials complete
- [ ] Code examples tested and work
- [ ] Checkpoints verify progress
- [ ] Troubleshooting included
- [ ] Time estimates accurate

---

### D2: Video Walkthroughs (8 hours)

**Objective**: Create screencasts demonstrating key features.

**Videos to Create**:

1. **Installation & Setup** (5 minutes)
2. **Creating Your First TVIEW** (10 minutes)
3. **Migration from Materialized Views** (15 minutes)
4. **Monitoring & Performance** (10 minutes)
5. **Troubleshooting Common Issues** (10 minutes)

**Video Script Template**:

```markdown
# Video: Installation & Setup

**Duration**: 5 minutes
**Level**: Beginner

## Script

[0:00-0:30] Introduction
"Welcome! In this video, we'll install pg_tviews and get it running in under 5 minutes."

[0:30-1:30] Prerequisites
"You'll need PostgreSQL 15 or later. Let's verify:"
```bash
psql --version
```
"Great, I have PostgreSQL 17."

[1:30-3:00] Installation
"First, install pgrx:"
```bash
cargo install --locked cargo-pgrx
cargo pgrx init
```

"Now clone and build pg_tviews:"
```bash
git clone https://github.com/your-org/pg_tviews.git
cd pg_tviews
cargo pgrx install --release
```

[3:00-4:00] Verification
"Create a test database and enable the extension:"
```bash
createdb test_pg_tviews
psql test_pg_tviews
```

```sql
CREATE EXTENSION pg_tviews;
SELECT pg_tviews_version();
```

[4:00-5:00] Wrap-up
"And that's it! pg_tviews is installed. In the next video, we'll create our first TVIEW."

## Recording Checklist

- [ ] Screen resolution: 1920x1080
- [ ] Terminal font size: 16pt minimum
- [ ] Speak clearly, moderate pace
- [ ] Show commands and output
- [ ] Pause for viewer to follow along
- [ ] Upload to YouTube with timestamps
- [ ] Add captions

## Resources Needed

- Screen recording software (OBS Studio)
- Microphone
- Video editing software (optional)
- YouTube channel
```

**Deliverables**:
- 5 video walkthroughs (total ~50 minutes)
- Video scripts with timestamps
- YouTube uploads with descriptions
- Embedded in documentation

**Acceptance Criteria**:
- [ ] All 5 videos recorded
- [ ] Clear audio and video quality
- [ ] Captions added
- [ ] Uploaded to YouTube
- [ ] Linked from docs

---

(Continued in APLUS_DOCUMENTATION_PLAN_PART3.md due to length...)
