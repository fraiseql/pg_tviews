# Index Optimization Guide

> **Trinity Pattern Reference**: All examples follow the pattern from [00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)
>
> **Quick Reminder**:
> - pk_{entity} = INTEGER (SERIAL) - Internal database primary key
> - id = UUID - External API identifier
> - fk_{parent} = INTEGER - Foreign key references (always to pk_ columns)

---

## Automatic Indexes

pg_tviews automatically creates:
- **PRIMARY KEY index** on `pk_<entity>` column (B-tree, INTEGER)
- No other indexes are created automatically

**Rationale**: pg_tviews doesn't know your query patterns, so manual index creation gives you full control.

---

## Recommended Manual Indexes

### 1. Foreign Key Indexes

**Why**: Speed up cascade updates and JOIN operations

```sql
-- For each fk_* column in tv_* tables
-- Trinity pattern: fk_* columns are always integers
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_comment_fk_post ON tv_comment(fk_post);
CREATE INDEX idx_tv_order_fk_customer ON tv_order(fk_customer);
```

**When to Create**: Always create for fk_* columns used in cascades

**Performance Impact**:
- Cascade update speedup: 10-100×
- Creation time (1M rows): ~30 seconds
- Size overhead: +15%

### 2. JSONB GIN Indexes

**Why**: Enable fast JSONB queries (`WHERE data @> '{}'`)

```sql
-- GIN index for JSONB containment queries
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);

-- Specific JSONB path index (PostgreSQL 14+)
-- Trinity pattern: JSONB keys use camelCase
CREATE INDEX idx_tv_post_data_title
ON tv_post USING GIN((data -> 'title'));

CREATE INDEX idx_tv_post_data_status
ON tv_post USING GIN((data -> 'status'));
```

**When to Create**: If you query JSONB data frequently

**Performance Impact**:
- JSONB query speedup: 10-100×
- Creation time (1M rows): ~2 minutes
- Size overhead: +30%

**Example Queries That Benefit**:
```sql
-- Containment query (needs GIN index)
SELECT * FROM tv_post
WHERE data @> '{"status": "published"}';

-- Path existence query (needs GIN index)
SELECT * FROM tv_post
WHERE data ? 'tags';

-- Path value query (needs specific path index or full GIN)
SELECT * FROM tv_post
WHERE data->>'status' = 'published';
```

### 3. UUID Filtering Indexes

**Why**: Speed up GraphQL queries by UUID

```sql
-- Index on id (UUID) column for GraphQL queries
-- Trinity pattern: id is UUID, pk_post is integer
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_user_id ON tv_user(id);
CREATE INDEX idx_tv_comment_id ON tv_comment(id);
```

**When to Create**: Always for GraphQL/API integration

**Performance Impact**:
- UUID lookup speedup: 50-500×
- Creation time (1M rows): ~45 seconds
- Size overhead: +20%

**⚠️ Important**:
- Index the `id` column (UUID), NOT the fk_* columns
- Foreign keys in Trinity pattern are integers (fk_user, fk_post), not UUIDs
- For foreign key lookups, use the integer indexes (see section 1)

### 4. Composite Indexes

**Why**: Optimize multi-column queries

```sql
-- For queries filtering by both fk_user and status
-- Trinity pattern: fk_user is integer, status is in JSONB
CREATE INDEX idx_tv_post_user_status
ON tv_post(fk_user, (data->>'status'));

-- For queries filtering by foreign key and date range
CREATE INDEX idx_tv_order_customer_date
ON tv_order(fk_customer, (data->>'createdAt'));
```

**When to Create**: Based on actual query patterns (use EXPLAIN ANALYZE)

**Performance Impact**:
- Multi-column query speedup: 50-200×
- Creation time: Similar to single-column indexes
- Size overhead: +25-35%

---

## Index Strategy by Use Case

### Small TVIEWs (<10K rows)

**Recommended Indexes**:
- PRIMARY KEY (automatic)
- fk_* columns (if used in cascades)

**Skip**:
- JSONB indexes (table scan is fast enough)
- UUID indexes (unless heavily queried)

**Rationale**: For small tables, sequential scans are often faster than index lookups.

### Medium TVIEWs (10K-1M rows)

**Recommended Indexes**:
- PRIMARY KEY (automatic)
- fk_* columns (always)
- GIN index on data column
- UUID id column

**Creation Script**:
```sql
-- Trinity pattern: tb_post -> tv_post (pk_post INT, id UUID, data JSONB)
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);
```

### Large TVIEWs (>1M rows)

**Recommended Indexes**:
- PRIMARY KEY (automatic)
- fk_* columns (always)
- GIN index on data column
- UUID id column
- Specific JSONB path indexes for frequent queries
- Consider partitioning (see [limits.md](../reference/limits.md))

**Creation Script**:
```sql
-- Trinity pattern: All entities use singular names
CREATE INDEX idx_tv_order_fk_customer ON tv_order(fk_customer);
CREATE INDEX idx_tv_order_id ON tv_order(id);
CREATE INDEX idx_tv_order_data_gin ON tv_order USING GIN(data);

-- Specific path indexes for hot queries
-- Trinity pattern: JSONB uses camelCase
CREATE INDEX idx_tv_order_data_status
ON tv_order USING GIN((data -> 'status'));

CREATE INDEX idx_tv_order_data_created
ON tv_order((data->>'createdAt'));
```

---

## Index Maintenance

### Check Index Usage

```sql
-- Find indexes ordered by usage (least used first)
SELECT
    pg_stat_user_indexes.schemaname,
    pg_stat_user_indexes.tablename,
    pg_stat_user_indexes.indexname,
    pg_stat_user_indexes.idx_scan as scans,
    pg_size_pretty(pg_relation_size(pg_stat_user_indexes.indexrelid)) as size
FROM pg_stat_user_indexes
WHERE pg_stat_user_indexes.tablename LIKE 'tv_%'
ORDER BY pg_stat_user_indexes.idx_scan ASC;
```

### Find Unused Indexes

```sql
-- Find indexes that have never been used (idx_scan = 0)
-- After significant runtime, these can be dropped
SELECT
    pg_stat_user_indexes.schemaname,
    pg_stat_user_indexes.tablename,
    pg_stat_user_indexes.indexname,
    pg_size_pretty(pg_relation_size(pg_stat_user_indexes.indexrelid)) as size
FROM pg_stat_user_indexes
WHERE pg_stat_user_indexes.tablename LIKE 'tv_%'
  AND pg_stat_user_indexes.idx_scan = 0
  AND pg_stat_user_indexes.indexname NOT LIKE '%_pkey';
```

### Check Index Bloat

```sql
-- Estimate index bloat
SELECT
    pg_class.relname as index_name,
    pg_size_pretty(pg_relation_size(pg_class.oid)) as size,
    ROUND(100 * pg_stat_user_indexes.idx_scan::numeric / NULLIF(pg_stat_user_tables.seq_scan + pg_stat_user_indexes.idx_scan, 0), 2) as usage_percent
FROM pg_class
JOIN pg_index ON pg_index.indexrelid = pg_class.oid
JOIN pg_stat_user_indexes ON pg_stat_user_indexes.indexrelid = pg_class.oid
JOIN pg_stat_user_tables ON pg_stat_user_tables.relid = pg_index.indrelid
WHERE pg_class.relname LIKE 'idx_tv_%';
```

### Reindex

```sql
-- Reindex a specific table (if bloated or corrupted)
REINDEX TABLE tv_your_entity;

-- Reindex all TVIEWs concurrently (PostgreSQL 12+)
DO $$
DECLARE
    tview_name TEXT;
BEGIN
    FOR tview_name IN
        SELECT pg_class.relname
        FROM pg_class
        WHERE pg_class.relname LIKE 'tv_%'
          AND pg_class.relkind = 'r'
    LOOP
        EXECUTE format('REINDEX TABLE CONCURRENTLY %I', tview_name);
    END LOOP;
END $$;
```

---

## Performance Impact

| Index Type | Creation Time (1M rows) | Size Overhead | Query Speedup | Maintenance Cost |
|------------|-------------------------|---------------|---------------|------------------|
| B-tree (int) | ~30 sec | +15% | 100-1000× | Low |
| B-tree (uuid) | ~45 sec | +20% | 50-500× | Low |
| GIN (jsonb) | ~2 min | +30% | 10-100× | Medium |
| Composite | ~40 sec | +25% | 50-200× | Medium |

**Notes**:
- Creation times assume standard HDD/SSD
- Size overhead is relative to table size
- Query speedup depends on selectivity
- Maintenance cost = UPDATE/INSERT overhead

---

## Automated Index Recommendation

Use this function to get index suggestions for your TVIEWs:

```sql
-- Trinity pattern: All TVIEWs have pk_{entity} (INT PK), id (UUID), data (JSONB)
CREATE OR REPLACE FUNCTION pg_tviews_suggest_indexes(entity_name TEXT)
RETURNS TABLE(index_suggestion TEXT, reason TEXT, estimated_benefit TEXT) AS $$
BEGIN
    -- Check for missing fk_* indexes (foreign keys are integers)
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_' ||
        information_schema.columns.column_name ||
        ' ON tv_' || entity_name || '(' ||
        information_schema.columns.column_name || ')' as index_suggestion,
        'Foreign key column without index (cascade performance)' as reason,
        '10-100× speedup for cascade updates' as estimated_benefit
    FROM information_schema.columns
    WHERE information_schema.columns.table_name = 'tv_' || entity_name
      AND information_schema.columns.column_name LIKE 'fk_%'
      AND information_schema.columns.column_name NOT IN (
        SELECT pg_attribute.attname
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
      );

    -- Suggest UUID index if missing
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_id ON tv_' ||
        entity_name || '(id)' as index_suggestion,
        'UUID column without index (GraphQL/API queries)' as reason,
        '50-500× speedup for ID lookups' as estimated_benefit
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
          AND pg_attribute.attname = 'id'
    );

    -- Suggest GIN index for large TVIEWs
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_data_gin ON tv_' ||
        entity_name || ' USING GIN(data)' as index_suggestion,
        'Large TVIEW without JSONB index (JSONB queries)' as reason,
        '10-100× speedup for JSONB containment queries' as estimated_benefit
    WHERE pg_relation_size(('tv_' || entity_name)::regclass) > 1024 * 1024 * 10  -- >10MB
      AND NOT EXISTS (
        SELECT 1
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
          AND pg_attribute.attname = 'data'
      );
END;
$$ LANGUAGE plpgsql;
```

**Usage**:
```sql
-- Get index suggestions for a specific TVIEW
SELECT * FROM pg_tviews_suggest_indexes('post');

-- Get suggestions for all TVIEWs
SELECT
    pg_tview_meta.entity,
    suggestions.*
FROM pg_tview_meta
CROSS JOIN LATERAL pg_tviews_suggest_indexes(pg_tview_meta.entity) as suggestions;
```

---

## Index Creation Best Practices

### 1. Create Indexes CONCURRENTLY

```sql
-- Avoid table locks during index creation
CREATE INDEX CONCURRENTLY idx_tv_post_fk_user ON tv_post(fk_user);
```

**Pros**: No downtime, table remains queryable
**Cons**: Takes longer, requires more resources

### 2. Create Indexes After Bulk Loading

```sql
-- 1. Bulk insert data
INSERT INTO tb_post SELECT * FROM external_data;

-- 2. Create TVIEW (without indexes)
CREATE TABLE tv_post AS SELECT ...;

-- 3. Create indexes on tv_post
CREATE INDEX CONCURRENTLY idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX CONCURRENTLY idx_tv_post_id ON tv_post(id);
CREATE INDEX CONCURRENTLY idx_tv_post_data_gin ON tv_post USING GIN(data);
```

**Benefit**: 2-3× faster than creating indexes first

### 3. Monitor Index Creation Progress

```sql
-- Check progress of CONCURRENTLY index creation
SELECT
    pg_stat_progress_create_index.phase,
    pg_stat_progress_create_index.blocks_done,
    pg_stat_progress_create_index.blocks_total,
    ROUND(100.0 * pg_stat_progress_create_index.blocks_done / NULLIF(pg_stat_progress_create_index.blocks_total, 0), 2) as percent_done
FROM pg_stat_progress_create_index;
```

### 4. Use FILLFACTOR for High-Update Tables

```sql
-- Leave space for updates to avoid page splits
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user) WITH (FILLFACTOR = 80);
```

**When to Use**: TVIEWs with frequent cascade updates (>1000 updates/sec)

---

## Troubleshooting

### Problem: Index creation is very slow

**Cause**: Large table, low memory, or high concurrency

**Solutions**:
```sql
-- Increase maintenance_work_mem (session-level)
SET maintenance_work_mem = '2GB';
CREATE INDEX ...;

-- Use parallel index creation (PostgreSQL 11+)
SET max_parallel_maintenance_workers = 4;
CREATE INDEX ...;
```

### Problem: Query not using index

**Diagnosis**:
```sql
-- Check if index exists
SELECT * FROM pg_indexes
WHERE tablename = 'tv_post' AND indexname = 'idx_tv_post_fk_user';

-- Check query plan
EXPLAIN ANALYZE
SELECT * FROM tv_post WHERE fk_user = 123;
```

**Common Causes**:
1. Index selectivity too low (table scan is faster)
2. Outdated statistics (`ANALYZE tv_post`)
3. Wrong data type (integer vs bigint)
4. Query pattern doesn't match index

### Problem: Too many indexes (slow UPDATEs)

**Diagnosis**:
```sql
-- Count indexes per TVIEW
SELECT
    pg_class.relname,
    COUNT(*) as index_count,
    pg_size_pretty(SUM(pg_relation_size(pg_index.indexrelid))) as total_index_size
FROM pg_class
JOIN pg_index ON pg_index.indrelid = pg_class.oid
WHERE pg_class.relname LIKE 'tv_%'
GROUP BY pg_class.relname
ORDER BY COUNT(*) DESC;
```

**Solution**: Drop unused indexes (see "Find Unused Indexes" above)

---

## See Also

- [Resource Limits](../reference/limits.md) - Capacity planning and scaling
- [Troubleshooting](troubleshooting.md) - Performance debugging
- [Monitoring](../../MONITORING.md) - Track index performance
- [Trinity Pattern Reference](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md) - Database schema conventions
