# Performance Best Practices

> **Trinity Pattern Reference**: All examples follow [00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

This guide provides proven strategies for optimal pg_tviews performance.

---

## Table of Contents

1. [Schema Design](#schema-design)
2. [Index Strategy](#index-strategy)
3. [Query Optimization](#query-optimization)
4. [Bulk Operations](#bulk-operations)
5. [Memory Management](#memory-management)
6. [Monitoring & Maintenance](#monitoring--maintenance)

---

## Schema Design

### ✅ DO: Keep JSONB Documents Lean

```sql
-- ✅ GOOD: Minimal JSONB (< 1 MB per row)
CREATE TABLE tv_post AS
SELECT
    tb_post.pk_post,
    tb_post.id,
    jsonb_build_object(
        'id', tb_post.id,
        'title', tb_post.title,
        'excerpt', LEFT(tb_post.content, 200),  -- Not full content
        'authorId', tb_user.id
    ) as data
FROM tb_post
JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

-- ❌ BAD: Large JSONB with full content
CREATE TABLE tv_post AS
SELECT
    tb_post.pk_post,
    tb_post.id,
    jsonb_build_object(
        'id', tb_post.id,
        'title', tb_post.title,
        'fullContent', tb_post.content,  -- Could be multi-MB
        'fullHtml', tb_post.rendered_html,  -- Even larger
        'allComments', (SELECT jsonb_agg(...) FROM tb_comment ...)  -- Nested data
    ) as data
FROM tb_post;
```

**Why**: Large JSONB documents slow down cascades and increase memory usage.

**Target**: Keep data column < 100 KB per row (ideally < 10 KB)

### ✅ DO: Limit Cascade Depth

```sql
-- ✅ GOOD: Shallow hierarchy (2-3 levels)
tb_user -> tv_user -> tv_post -> tv_comment

-- ❌ BAD: Deep hierarchy (>5 levels)
tb_user -> tv_user -> tv_post -> tv_comment -> tv_reply -> tv_notification -> tv_feed
```

**Why**: Each level multiplies cascade overhead.

**Target**: Max 3-4 cascade levels for optimal performance

### ✅ DO: Use INTEGER Foreign Keys

```sql
-- ✅ GOOD: Trinity pattern (integer FKs)
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    fk_user BIGINT NOT NULL,  -- INTEGER FK
    title TEXT
);

-- ❌ BAD: UUID foreign keys
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,  -- UUID FK (slower joins)
    title TEXT
);
```

**Why**: INTEGER joins are 2-3× faster than UUID joins.

**Benefit**: Faster cascades, smaller indexes

---

## Index Strategy

### ✅ DO: Index All Foreign Keys

```sql
-- Trinity pattern: All fk_* columns should be indexed
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_comment_fk_post ON tv_comment(fk_post);
CREATE INDEX idx_tv_notification_fk_user ON tv_notification(fk_user);
```

**Impact**: 10-100× faster cascade updates

**When**: Always, for every TVIEW with dependencies

### ✅ DO: Index UUID Columns for API Queries

```sql
-- Trinity pattern: id is UUID, pk_* is integer
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_user_id ON tv_user(id);
```

**Impact**: 50-500× faster GraphQL/API lookups

**When**: Always for API-exposed entities

### ✅ DO: Use GIN Indexes for JSONB Queries

```sql
-- For containment queries (@>, ?, ?&, ?|)
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);

-- For specific path queries (more selective)
CREATE INDEX idx_tv_post_status ON tv_post USING GIN((data -> 'status'));
```

**Impact**: 10-100× faster JSONB queries

**When**: TVIEWs >10K rows with frequent JSONB queries

### ❌ DON'T: Over-Index Small Tables

```sql
-- ❌ BAD: 5 indexes on 1000-row table
CREATE INDEX idx_tv_tag_fk_category ON tv_tag(fk_category);
CREATE INDEX idx_tv_tag_id ON tv_tag(id);
CREATE INDEX idx_tv_tag_data_gin ON tv_tag USING GIN(data);
CREATE INDEX idx_tv_tag_name ON tv_tag((data->>'name'));
CREATE INDEX idx_tv_tag_created ON tv_tag((data->>'createdAt'));

-- ✅ GOOD: Minimal indexes for small table
CREATE INDEX idx_tv_tag_fk_category ON tv_tag(fk_category);  -- For cascades
CREATE INDEX idx_tv_tag_id ON tv_tag(id);  -- For API lookups
-- That's it! Table scan is fast for 1000 rows
```

**Why**: Index maintenance overhead exceeds query benefits for small tables.

**Rule**: For tables <10K rows, only index fk_* and id columns

---

## Query Optimization

### ✅ DO: Query by Primary Key (INTEGER)

```sql
-- ✅ FAST: Query by pk_* (integer, indexed)
SELECT * FROM tv_post WHERE pk_post = 123;

-- ⚠️ SLOWER: Query by id (UUID, requires index)
SELECT * FROM tv_post WHERE id = 'a1b2c3...';
```

**Why**: INTEGER primary keys are smaller and faster than UUID indexes.

**When**: Internal queries, cascade operations

### ✅ DO: Use Prepared Statements

```sql
-- Prepare once
PREPARE get_post AS
SELECT * FROM tv_post WHERE pk_post = $1;

-- Execute many times
EXECUTE get_post(123);
EXECUTE get_post(456);
EXECUTE get_post(789);
```

**Impact**: 2-5× faster (eliminates planning overhead)

**When**: Repeated queries with different parameters

### ✅ DO: Leverage JSONB Operators

```sql
-- ✅ GOOD: Use GIN-indexable operators
SELECT * FROM tv_post WHERE data @> '{"status": "published"}';  -- Containment
SELECT * FROM tv_post WHERE data ? 'tags';  -- Key existence

-- ❌ BAD: Functions prevent index usage
SELECT * FROM tv_post WHERE jsonb_extract_path_text(data, 'status') = 'published';
```

**Why**: Operator form uses GIN indexes, function form doesn't.

---

## Bulk Operations

### ✅ DO: Batch Large Updates

```sql
-- ✅ GOOD: Batch of 1000
DO $$
DECLARE
    batch_size INT := 1000;
    offset_val INT := 0;
BEGIN
    LOOP
        UPDATE tb_post
        SET title = tb_post.title || ' [updated]'
        WHERE tb_post.pk_post IN (
            SELECT tb_post.pk_post
            FROM tb_post
            ORDER BY tb_post.pk_post
            LIMIT batch_size OFFSET offset_val
        );

        EXIT WHEN NOT FOUND;
        offset_val := offset_val + batch_size;
        COMMIT;
    END LOOP;
END $$;

-- ❌ BAD: Update 1M rows in single transaction
UPDATE tb_post SET title = tb_post.title || ' [updated]';  -- Huge cascade!
```

**Why**: Batch commits prevent lock contention and memory exhaustion.

**Target**: 1000-10000 rows per batch

### ✅ DO: Disable Triggers for Bulk Loading

```sql
-- For initial data load only
BEGIN;

ALTER TABLE tb_post DISABLE TRIGGER ALL;

COPY tb_post FROM '/data/posts.csv' CSV;

ALTER TABLE tb_post ENABLE TRIGGER ALL;

-- Rebuild TVIEWs after load
TRUNCATE tv_post;
INSERT INTO tv_post SELECT * FROM v_post;

COMMIT;
```

**Impact**: 5-10× faster bulk loading

**When**: Initial data import, NOT incremental updates

### ❌ DON'T: Cascade Unrelated Updates

```sql
-- ❌ BAD: Updates timestamp, triggers unnecessary cascade
UPDATE tb_post
SET updated_at = NOW()
WHERE pk_post = 123;
-- Cascade executes even though JSONB data unchanged!

-- ✅ GOOD: Only update if data changes
UPDATE tb_post
SET updated_at = NOW(), title = 'New Title'
WHERE pk_post = 123
  AND title != 'New Title';  -- Prevent no-op updates
```

**Why**: Unnecessary cascades waste resources.

---

## Memory Management

### ✅ DO: Configure work_mem Appropriately

```sql
-- For typical workloads
ALTER SYSTEM SET work_mem = '64MB';  -- Per operation

-- For large cascades (session-level)
SET work_mem = '256MB';
UPDATE tb_user SET name = 'Updated';  -- Large cascade
RESET work_mem;
```

**Guidelines**:
- Small TVIEWs (<100K rows): 64 MB
- Medium TVIEWs (100K-1M): 128-256 MB
- Large TVIEWs (>1M): 256-512 MB

### ✅ DO: Monitor Memory Usage

```sql
-- Check current memory usage
SELECT
    pg_stat_activity.pid,
    pg_stat_activity.usename,
    pg_stat_activity.query,
    pg_size_pretty(SUM(pg_backend_memory_contexts.total_bytes)) as memory
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
WHERE pg_stat_activity.state = 'active'
GROUP BY pg_stat_activity.pid, pg_stat_activity.usename, pg_stat_activity.query;
```

### ❌ DON'T: Store Large BLOBs in JSONB

```sql
-- ❌ BAD: Base64-encoded image in JSONB
jsonb_build_object(
    'image', encode(tb_post.image_data, 'base64')  -- Multi-MB string!
)

-- ✅ GOOD: Reference to external storage
jsonb_build_object(
    'imageUrl', '/api/images/' || tb_post.id
)
```

**Why**: JSONB is for structured data, not binary blobs.

---

## Monitoring & Maintenance

### ✅ DO: Run ANALYZE Regularly

```sql
-- After bulk operations
ANALYZE tv_post;

-- For all TVIEWs
DO $$
DECLARE
    tview_name TEXT;
BEGIN
    FOR tview_name IN
        SELECT 'tv_' || entity FROM pg_tview_meta
    LOOP
        EXECUTE 'ANALYZE ' || tview_name;
    END LOOP;
END $$;
```

**Impact**: Ensures optimal query plans

**When**: After bulk loading, after significant updates (>10% rows)

### ✅ DO: Monitor Index Usage

```sql
-- Find unused indexes
SELECT
    pg_stat_user_indexes.schemaname,
    pg_stat_user_indexes.tablename,
    pg_stat_user_indexes.indexname,
    pg_size_pretty(pg_relation_size(pg_stat_user_indexes.indexrelid)) as size,
    pg_stat_user_indexes.idx_scan
FROM pg_stat_user_indexes
WHERE pg_stat_user_indexes.tablename LIKE 'tv_%'
  AND pg_stat_user_indexes.idx_scan = 0
  AND pg_stat_user_indexes.indexname NOT LIKE '%_pkey'
ORDER BY pg_relation_size(pg_stat_user_indexes.indexrelid) DESC;

-- Drop unused indexes (saves space + update overhead)
-- DROP INDEX idx_tv_post_unused;
```

**Benefit**: Reduce storage and write overhead

### ✅ DO: Vacuum Regularly

```sql
-- Check bloat
SELECT
    pg_class.relname,
    pg_size_pretty(pg_relation_size(pg_class.oid)) as size,
    n_dead_tup,
    ROUND(100.0 * n_dead_tup / NULLIF(n_live_tup + n_dead_tup, 0), 2) as dead_pct
FROM pg_stat_user_tables
JOIN pg_class ON pg_class.relname = pg_stat_user_tables.relname
WHERE pg_stat_user_tables.relname LIKE 'tv_%'
ORDER BY n_dead_tup DESC;

-- Manual vacuum if autovacuum is lagging
VACUUM ANALYZE tv_post;
```

**When**: After bulk deletes/updates, if autovacuum is insufficient

---

## Performance Checklist

### For New TVIEWs

- [ ] JSONB data column <100 KB per row
- [ ] Cascade depth ≤ 4 levels
- [ ] All fk_* columns indexed
- [ ] UUID id column indexed (if API-exposed)
- [ ] Run ANALYZE after initial population

### For Large TVIEWs (>100K rows)

- [ ] GIN index on data column
- [ ] Specific JSONB path indexes for hot queries
- [ ] work_mem ≥ 128 MB
- [ ] Cache hit ratio >95%
- [ ] Autovacuum configured

### For Very Large TVIEWs (>1M rows)

- [ ] Consider partitioning
- [ ] Composite indexes for multi-column queries
- [ ] Monitor sequential scans (should be rare)
- [ ] Prepared statements for repeated queries
- [ ] Regular REINDEX to prevent bloat

### For Deep Cascades (>3 levels)

- [ ] All intermediate TVIEWs indexed on fk_* columns
- [ ] Monitor cascade execution time
- [ ] Consider flattening if possible
- [ ] Batch large updates (1000-10K rows)

---

## Anti-Patterns to Avoid

### ❌ Don't: Create TVIEWs on TVIEWs

```sql
-- ❌ BAD: TVIEW depends on another TVIEW
CREATE TABLE tv_post_summary AS
SELECT
    tv_post.pk_post,
    tv_post.id,
    jsonb_build_object(
        'title', tv_post.data->>'title',  -- Querying TVIEW
        'excerpt', LEFT(tv_post.data->>'content', 100)
    ) as data
FROM tv_post;  -- Depends on tv_post, not tb_post

-- ✅ GOOD: TVIEW depends on base table
CREATE TABLE tv_post_summary AS
SELECT
    tb_post.pk_post,
    tb_post.id,
    jsonb_build_object(
        'title', tb_post.title,  -- Direct from base table
        'excerpt', LEFT(tb_post.content, 100)
    ) as data
FROM tb_post;
```

**Why**: Creates unnecessary cascade chains and coupling.

### ❌ Don't: Use SELECT * in TVIEW Definition

```sql
-- ❌ BAD: SELECT *
CREATE TABLE tv_post AS
SELECT
    *  -- Returns all columns, including sensitive ones
FROM tb_post;

-- ✅ GOOD: Explicit columns
CREATE TABLE tv_post AS
SELECT
    tb_post.pk_post,
    tb_post.id,
    jsonb_build_object(
        'id', tb_post.id,
        'title', tb_post.title
        -- Explicit: no password_hash, no internal_notes
    ) as data
FROM tb_post;
```

**Why**: Security, clarity, and forward compatibility.

### ❌ Don't: Mix Business Logic in TVIEWs

```sql
-- ❌ BAD: Complex business logic
CREATE TABLE tv_order AS
SELECT
    tb_order.pk_order,
    tb_order.id,
    jsonb_build_object(
        'id', tb_order.id,
        'total', tb_order.subtotal + tb_order.tax + tb_order.shipping
                 - COALESCE(tb_discount.amount, 0)
                 + CASE WHEN tb_order.rush_delivery THEN 10 ELSE 0 END,  -- Complex!
        'status', CASE
            WHEN tb_order.paid_at IS NOT NULL AND tb_order.shipped_at IS NULL THEN 'processing'
            WHEN tb_order.shipped_at IS NOT NULL THEN 'shipped'
            ELSE 'pending'
        END
    ) as data
FROM tb_order
LEFT JOIN tb_discount ON tb_order.fk_discount = tb_discount.pk_discount;

-- ✅ GOOD: Pre-compute in base table or application
CREATE TABLE tv_order AS
SELECT
    tb_order.pk_order,
    tb_order.id,
    jsonb_build_object(
        'id', tb_order.id,
        'total', tb_order.total_amount,  -- Pre-computed
        'status', tb_order.status  -- Pre-computed
    ) as data
FROM tb_order;
```

**Why**: Simplifies TVIEW, moves logic to appropriate layer.

---

## PostgreSQL Configuration Recommendations

### For Small Deployments (<100K rows per TVIEW)

```ini
# postgresql.conf
shared_buffers = 256MB
work_mem = 64MB
maintenance_work_mem = 128MB
effective_cache_size = 1GB
```

### For Medium Deployments (100K-1M rows)

```ini
shared_buffers = 1GB
work_mem = 128MB
maintenance_work_mem = 512MB
effective_cache_size = 4GB
max_parallel_workers_per_gather = 4
```

### For Large Deployments (>1M rows)

```ini
shared_buffers = 4GB
work_mem = 256MB
maintenance_work_mem = 2GB
effective_cache_size = 16GB
max_parallel_workers_per_gather = 8
random_page_cost = 1.1  # For SSD
```

---

## See Also

- [Index Optimization](index-optimization.md) - Detailed indexing strategies
- [Performance Analysis](performance-analysis.md) - Tools and diagnostics
- [Resource Limits](../reference/limits.md) - Capacity planning
- [Monitoring](../../MONITORING.md) - Production monitoring setup
- [Troubleshooting](troubleshooting.md) - Debug performance issues
