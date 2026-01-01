# Performance Analysis

> **Trinity Pattern Reference**: All examples follow [00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

This guide covers tools and techniques for analyzing and optimizing pg_tviews performance.

---

## Quick Performance Check

```sql
-- Get overview of all TVIEWs
SELECT * FROM pg_tviews_performance_stats();

-- Sample output:
--  entity |  table_size | total_size | row_count | index_count
-- --------+-------------+------------+-----------+-------------
--  post   | 50 MB       | 75 MB      |   100000  |     4
--  user   | 10 MB       | 15 MB      |    20000  |     3
--  comment| 80 MB       | 120 MB     |   500000  |     5
```

---

## Cascade Dependency Analysis

### View Dependency Chain

```sql
-- Show what depends on 'user' entity
-- Trinity pattern: Entities are singular (user, not users)
SELECT * FROM pg_tviews_show_cascade_path('user');

-- Sample output:
--  depth | entity_name | depends_on
-- -------+-------------+------------
--      0 | user        | user
--      1 | post        | user
--      2 | comment     | post
--      3 | notification| comment
```

**Interpretation**:
- Depth 0: The root entity (user)
- Depth 1: Direct dependents (post depends on user)
- Depth 2: Second-level (comment depends on post)
- Depth 3+: Deeper cascade levels

**Use Cases**:
- Identify cascade bottlenecks
- Plan index strategies
- Understand update propagation
- Optimize TVIEW structure

### Analyze Cascade Impact

```sql
-- Count total cascade impact when updating an entity
WITH RECURSIVE cascade AS (
    SELECT entity, 0 as depth
    FROM pg_tview_meta
    WHERE entity = 'user'

    UNION ALL

    SELECT m.entity, c.depth + 1
    FROM cascade c
    JOIN pg_tview_meta m ON ('tv_' || c.entity)::regclass::oid = ANY(m.dependencies)
    WHERE c.depth < 10
)
SELECT
    CASCADE.depth,
    COUNT(*) as entities_at_depth,
    array_agg(CASCADE.entity) as entity_names
FROM CASCADE
GROUP BY CASCADE.depth
ORDER BY CASCADE.depth;

-- Trinity pattern: All entities use singular names
```

---

## Query Plan Analysis

### Manual EXPLAIN Analysis

```sql
-- Analyze a TVIEW refresh query
-- Trinity pattern: v_post has pk_post (int), id (UUID), data (JSONB)
EXPLAIN (ANALYZE, BUFFERS)
SELECT v_post.pk_post, v_post.id, v_post.data
FROM v_post
WHERE v_post.pk_post = 123;

-- Look for:
-- - Seq Scan (bad for large tables) vs Index Scan (good)
-- - Buffers: shared hit (good) vs read (disk I/O, slow)
-- - Execution Time vs Planning Time ratio
```

### Identify Missing Indexes

```sql
-- Find sequential scans on large TVIEWs
SELECT
    pg_stat_user_tables.schemaname,
    pg_stat_user_tables.relname,
    pg_stat_user_tables.seq_scan,
    pg_stat_user_tables.seq_tup_read,
    pg_stat_user_tables.idx_scan,
    pg_size_pretty(pg_relation_size(pg_stat_user_tables.relid)) as table_size
FROM pg_stat_user_tables
WHERE pg_stat_user_tables.relname LIKE 'tv_%'
  AND pg_stat_user_tables.seq_scan > 1000  -- High seq scan count
  AND pg_relation_size(pg_stat_user_tables.relid) > 10*1024*1024  -- >10MB
ORDER BY pg_stat_user_tables.seq_tup_read DESC;
```

---

## Performance Bottleneck Identification

### Slowest TVIEWs by Size

```sql
-- Trinity pattern: All TVIEWs named tv_{entity} (singular)
SELECT
    pg_class.relname,
    pg_size_pretty(pg_relation_size(pg_class.oid)) as table_size,
    pg_size_pretty(pg_total_relation_size(pg_class.oid)) as total_size,
    (pg_total_relation_size(pg_class.oid) - pg_relation_size(pg_class.oid))::bigint as index_size_bytes,
    ROUND(100.0 * (pg_total_relation_size(pg_class.oid) - pg_relation_size(pg_class.oid)) / NULLIF(pg_relation_size(pg_class.oid), 0), 2) as index_overhead_percent
FROM pg_class
WHERE pg_class.relname LIKE 'tv_%'
  AND pg_class.relkind = 'r'
ORDER BY pg_relation_size(pg_class.oid) DESC
LIMIT 10;
```

### Check JSONB Column Sizes

```sql
-- Find TVIEWs with large JSONB documents
-- Trinity pattern: All TVIEWs have 'data' column (JSONB)
SELECT
    'tv_' || pg_tview_meta.entity as tview_name,
    pg_size_pretty(AVG(pg_column_size(tv.data))) as avg_jsonb_size,
    pg_size_pretty(MAX(pg_column_size(tv.data))) as max_jsonb_size,
    COUNT(*) as row_count
FROM pg_tview_meta
CROSS JOIN LATERAL (
    SELECT data FROM ('tv_' || pg_tview_meta.entity)::regclass LIMIT 1000
) tv(data)
GROUP BY pg_tview_meta.entity
ORDER BY AVG(pg_column_size(tv.data)) DESC;
```

### Identify Cascade Hotspots

```sql
-- Find entities that trigger the most cascades
SELECT
    pg_tview_meta.entity,
    array_length(pg_tview_meta.dependencies, 1) as direct_dependencies,
    (
        SELECT COUNT(*)
        FROM pg_tview_meta m2
        WHERE ('tv_' || pg_tview_meta.entity)::regclass::oid = ANY(m2.dependencies)
    ) as dependent_tviews
FROM pg_tview_meta
ORDER BY dependent_tviews DESC, direct_dependencies DESC;
```

---

## Real-Time Performance Monitoring

### Active Cascade Operations

```sql
-- Show currently running pg_tviews operations
SELECT
    pg_stat_activity.pid,
    pg_stat_activity.usename,
    pg_stat_activity.application_name,
    pg_stat_activity.state,
    NOW() - pg_stat_activity.query_start as duration,
    pg_stat_activity.query
FROM pg_stat_activity
WHERE pg_stat_activity.query LIKE '%tv_%'
  AND pg_stat_activity.state != 'idle'
  AND pg_stat_activity.pid != pg_backend_pid()
ORDER BY pg_stat_activity.query_start;
```

### Cache Hit Rates

```sql
-- Check buffer cache effectiveness for TVIEWs
SELECT
    pg_statio_user_tables.relname,
    pg_statio_user_tables.heap_blks_read as disk_reads,
    pg_statio_user_tables.heap_blks_hit as cache_hits,
    ROUND(100.0 * pg_statio_user_tables.heap_blks_hit / NULLIF(pg_statio_user_tables.heap_blks_hit + pg_statio_user_tables.heap_blks_read, 0), 2) as cache_hit_ratio
FROM pg_statio_user_tables
WHERE pg_statio_user_tables.relname LIKE 'tv_%'
ORDER BY cache_hit_ratio ASC;

-- Aim for >95% cache hit ratio
-- <90% indicates insufficient shared_buffers
```

---

## Optimization Recommendations

### Based on Performance Stats

```sql
-- Get automated recommendations
DO $$
DECLARE
    rec RECORD;
BEGIN
    -- Check each TVIEW
    FOR rec IN
        SELECT * FROM pg_tviews_performance_stats()
    LOOP
        -- Large table without enough indexes
        IF rec.table_size > '100 MB' AND rec.index_count < 3 THEN
            RAISE NOTICE 'Entity %: Large table (%) with only % indexes - consider adding indexes',
                rec.entity, rec.table_size, rec.index_count;
        END IF;

        -- Very large table
        IF rec.table_size > '1 GB' THEN
            RAISE NOTICE 'Entity %: Very large (%) - consider partitioning',
                rec.entity, rec.table_size;
        END IF;

        -- High index overhead
        IF pg_total_relation_size(('tv_' || rec.entity)::regclass) >
           2 * pg_relation_size(('tv_' || rec.entity)::regclass) THEN
            RAISE NOTICE 'Entity %: Index size exceeds table size - review unused indexes',
                rec.entity;
        END IF;
    END LOOP;
END $$;
```

---

## Performance Tuning Checklist

### ✅ For All TVIEWs

- [ ] `pk_{entity}` column has PRIMARY KEY (automatic)
- [ ] `fk_{parent}` columns have indexes
- [ ] `id` column (UUID) has index for API queries
- [ ] ANALYZE run after bulk loading

### ✅ For Large TVIEWs (>100K rows)

- [ ] GIN index on `data` column (JSONB)
- [ ] Specific path indexes for hot queries
- [ ] Vacuum regularly (autovacuum configured)
- [ ] Monitor cache hit ratio (>95%)

### ✅ For Very Large TVIEWs (>1M rows)

- [ ] Consider partitioning by date or hash
- [ ] Composite indexes for multi-column queries
- [ ] Increase `work_mem` for sorts
- [ ] Test with EXPLAIN ANALYZE

### ✅ For Deep Cascades (>3 levels)

- [ ] Verify all fk_* columns indexed
- [ ] Consider flattening if possible
- [ ] Monitor cascade execution time
- [ ] Use statement-level triggers for bulk ops

---

## Benchmarking

### Cascade Update Benchmark

```sql
-- Benchmark a cascade update
-- Trinity pattern: pk_user is integer, id is UUID
BEGIN;

-- Capture start time
\timing on

-- Perform update
UPDATE tb_user
SET name = tb_user.name || ' (updated)'
WHERE tb_user.pk_user = 1;

-- Commit triggers cascade
COMMIT;

\timing off

-- Analyze what happened
SELECT * FROM pg_tviews_show_cascade_path('user');
```

### Bulk Update Benchmark

```sql
-- Benchmark bulk cascade
\timing on

BEGIN;

-- Update 1000 rows
UPDATE tb_post
SET title = tb_post.title || ' [bulk]'
WHERE tb_post.pk_post BETWEEN 1 AND 1000;

COMMIT;

\timing off

-- Calculate rows/second
-- Compare with and without indexes
```

---

## Common Performance Issues

### Issue 1: Slow Cascades

**Symptoms**: Updates take >1 second

**Diagnosis**:
```sql
-- Check for missing indexes
SELECT * FROM pg_tviews_show_cascade_path('your_entity');

-- Verify fk_* indexes exist
SELECT
    pg_indexes.tablename,
    pg_indexes.indexname,
    pg_indexes.indexdef
FROM pg_indexes
WHERE pg_indexes.tablename LIKE 'tv_%'
  AND pg_indexes.indexdef LIKE '%fk_%';
```

**Solutions**:
1. Create indexes on fk_* columns
2. Install jsonb_delta extension (1.5-3× speedup)
3. Use statement-level triggers for bulk ops

### Issue 2: High Memory Usage

**Symptoms**: OOM errors during cascades

**Diagnosis**:
```sql
-- Check work_mem
SHOW work_mem;

-- Check cascade size
SELECT * FROM pg_tviews_performance_stats();
```

**Solutions**:
1. Increase `work_mem` (session or global)
2. Batch large updates
3. Add indexes to reduce sort memory

### Issue 3: Poor JSONB Query Performance

**Symptoms**: Queries on `data` column are slow

**Diagnosis**:
```sql
EXPLAIN ANALYZE
SELECT * FROM tv_post
WHERE data @> '{"status": "published"}';
```

**Solutions**:
1. Create GIN index: `CREATE INDEX ... USING GIN(data)`
2. Create specific path index: `CREATE INDEX ... USING GIN((data -> 'status'))`
3. Use B-tree for equality: `CREATE INDEX ...((data->>'status'))`

---

## See Also

- [Index Optimization](index-optimization.md) - Detailed index strategies
- [Resource Limits](../reference/limits.md) - Capacity planning
- [Monitoring](../../MONITORING.md) - Production monitoring
- [Troubleshooting](troubleshooting.md) - Debug performance issues
