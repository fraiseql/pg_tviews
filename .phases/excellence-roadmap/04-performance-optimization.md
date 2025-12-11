# Phase 4: Performance & Optimization

**Goal**: 88/100 → 95/100
**Effort**: 15-25 hours
**Priority**: Medium

> **⚠️ TRINITY PATTERN REQUIRED**: All performance examples MUST follow the trinity pattern.
> **See**: [00-TRINITY-PATTERN-REFERENCE.md](./00-TRINITY-PATTERN-REFERENCE.md)
>
> **Index Pattern**:
> ```sql
> -- Always create these indexes for every TVIEW:
> CREATE INDEX idx_tv_{entity}_id ON tv_{entity}(id);           -- UUID
> CREATE INDEX idx_tv_{entity}_fk_{parent} ON tv_{entity}(fk_{parent});  -- INTEGER
> ```

---


1. Add index optimization guide
2. Implement query plan analysis tools
3. Create performance tuning utilities
4. Add cache size configuration
5. Optimize prepared statement handling
6. Document performance best practices

### Task Breakdown

#### Task 4.1: Index Optimization Guide (P2)
**Effort**: 3-4 hours

**New File**: `docs/operations/index-optimization.md`

```markdown
# Index Optimization Guide

## Automatic Indexes

pg_tviews automatically creates:
- PRIMARY KEY index on `pk_<entity>` column
- (Other indexes TBD - verify in code)

## Recommended Manual Indexes

### 1. Foreign Key Indexes

**Why**: Speed up cascade updates and JOIN operations

```sql
-- For each fk_* column in tv_* tables
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_comment_fk_post ON tv_comment(fk_post);
```

**When to Create**: Always create for fk_* columns used in cascades

### 2. JSONB GIN Indexes

**Why**: Enable fast JSONB queries (WHERE data @> '{}')

```sql
-- GIN index for JSONB containment queries
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);

-- Specific JSONB path index (PostgreSQL 14+)
CREATE INDEX idx_tv_post_data_title
ON tv_post USING GIN((data -> 'title'));
```

**When to Create**: If you query JSONB data frequently

### 3. UUID Filtering Indexes

**Why**: Speed up GraphQL queries by UUID

```sql
-- Index on id (UUID) column for GraphQL queries
-- Note: id is UUID, pk_post is integer
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- NO need for UUID foreign key indexes - use integer fk_* columns instead
-- Foreign keys in Trinity pattern are integers (fk_user, not user_id)
-- See: Foreign Key Indexes section above
```

**When to Create**: Always for GraphQL Cascade integration - but only on `id` (UUID), not on FKs

### 4. Composite Indexes

**Why**: Optimize multi-column queries

```sql
-- For queries filtering by both fk_user and status
CREATE INDEX idx_tv_post_user_status
ON tv_post(fk_user, (data->>'status'));
```

**When to Create**: Based on actual query patterns

## Index Strategy by Use Case

### Small TVIEWs (<10K rows)
- PRIMARY KEY (automatic)
- fk_* columns
- Skip JSONB indexes (table scan is fast enough)

### Medium TVIEWs (10K-1M rows)
- PRIMARY KEY (automatic)
- fk_* columns
- GIN index on data column
- UUID id column

### Large TVIEWs (>1M rows)
- PRIMARY KEY (automatic)
- fk_* columns
- GIN index on data column
- UUID id column
- Specific JSONB path indexes for frequent queries
- Consider partitioning (see docs/operations/partitioning.md)

## Index Maintenance

```sql
-- Check index usage
SELECT schemaname, tablename, indexname, idx_scan
FROM pg_stat_user_indexes
WHERE tablename LIKE 'tv_%'
ORDER BY idx_scan ASC;

-- Find unused indexes (idx_scan = 0 after significant runtime)
SELECT schemaname, tablename, indexname,
       pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE tablename LIKE 'tv_%'
  AND idx_scan = 0
  AND indexname NOT LIKE '%_pkey';

-- Reindex if bloated
REINDEX TABLE tv_your_entity;
```

## Performance Impact

| Index Type | Creation Time (1M rows) | Size Overhead | Query Speedup |
|------------|-------------------------|---------------|---------------|
| B-tree (int) | ~30 sec | +15% | 100-1000× |
| B-tree (uuid) | ~45 sec | +20% | 50-500× |
| GIN (jsonb) | ~2 min | +30% | 10-100× |

## Automated Index Recommendation

```sql
-- Function to suggest indexes (to implement)
-- Trinity pattern: All TVIEWs have pk_{entity} (int PK), id (UUID), data (JSONB)
CREATE OR REPLACE FUNCTION pg_tviews_suggest_indexes(entity_name TEXT)
RETURNS TABLE(index_suggestion TEXT, reason TEXT) AS $$
BEGIN
    -- Check for missing fk_* indexes (foreign keys are integers)
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_' ||
        information_schema.columns.column_name ||
        ' ON tv_' || entity_name || '(' ||
        information_schema.columns.column_name || ')' as index_suggestion,
        'Foreign key column without index (fk_* columns are integers)' as reason
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
        'UUID column without index (for GraphQL queries)' as reason
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
          AND pg_attribute.attname = 'id'
    );
END;
$$ LANGUAGE plpgsql;
```
```

**Acceptance Criteria**:
- [ ] Index recommendations documented
- [ ] Index strategy by TVIEW size
- [ ] Index maintenance queries provided
- [ ] Performance impact measured
- [ ] Index suggestion function implemented

---

#### Task 4.2: Query Plan Analysis Tools
**Effort**: 4-5 hours

**New File**: `src/lib.rs` (add function)

```rust
/// Analyze cascade update query plan
///
/// Returns EXPLAIN output for a cascade update operation
#[pg_extern]
fn pg_tviews_analyze_cascade(
    entity: &str,
    pk_value: i64
) -> TableIterator<'static, (name!(query_plan, String),)> {
    let tview_name = format!("tv_{}", entity);
    let view_name = format!("v_{}", entity);

    // Get the refresh query that would execute
    // Trinity pattern: v_{entity} view returns pk_{entity}, id (UUID), data (JSONB)
    let refresh_query = format!(
        "SELECT {}.pk_{}, {}.id, {}.data FROM {} WHERE {}.pk_{} = {}",
        view_name, entity, view_name, view_name, view_name, view_name, entity, pk_value
    );

    // EXPLAIN ANALYZE the query
    let explain_query = format!("EXPLAIN (ANALYZE, BUFFERS) {}", refresh_query);

    let results = Spi::connect(|client| {
        let rows = client.select(&explain_query, None, None)?;
        let mut plans = Vec::new();

        for row in rows {
            if let Some(plan_line) = row[1].value::<String>()? {
                plans.push((plan_line,));
            }
        }

        Ok::<_, spi::Error>(plans)
    }).unwrap_or_default();

    TableIterator::new(results)
}

/// Show cascade dependency path
///
/// Returns the dependency chain for a given entity
#[pg_extern]
fn pg_tviews_show_cascade_path(entity: &str) -> TableIterator<'static, (
    name!(depth, i32),
    name!(entity_name, String),
    name!(dependency_type, String),
)> {
    // Query dependency graph from metadata
    let query = format!(
        "WITH RECURSIVE dep_tree AS (
            SELECT entity, 0 as depth, ARRAY[entity] as path
            FROM pg_tview_meta
            WHERE entity = '{}'

            UNION ALL

            SELECT m.entity, dt.depth + 1, dt.path || m.entity
            FROM dep_tree dt
            JOIN pg_tview_meta m ON m.dependencies && ARRAY[('tv_' || dt.entity)::regclass::oid]
            WHERE NOT (m.entity = ANY(dt.path))  -- Prevent cycles
              AND dt.depth < 10
        )
        SELECT depth, entity_name, 'cascade' as dependency_type
        FROM dep_tree
        ORDER BY depth",
        entity.replace("'", "''")
    );

    let results = Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut paths = Vec::new();

        for row in rows {
            let depth = row["depth"].value::<i32>()?.unwrap_or(0);
            let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
            let dep_type = row["dependency_type"].value::<String>()?.unwrap_or_default();

            paths.push((depth, entity_name, dep_type));
        }

        Ok::<_, spi::Error>(paths)
    }).unwrap_or_default();

    TableIterator::new(results)
}
```

**Documentation**:

**File**: `docs/operations/performance-analysis.md` (new)

```markdown
# Performance Analysis

## Cascade Update Analysis

```sql
-- Analyze a specific cascade operation
-- Note: pk_post is integer (SERIAL), id is UUID
SELECT * FROM pg_tviews_analyze_cascade('post', 123);

-- Sample output:
                    query_plan
----------------------------------------------------------
 Seq Scan on v_post  (cost=0.00..15.50 rows=1 width=48)
   Filter: (pk_post = 123)
   Buffers: shared hit=8
 Planning Time: 0.123 ms
 Execution Time: 0.456 ms

-- Trinity pattern: v_post view has pk_post (int), id (UUID), data (JSONB)
```

## Dependency Path Visualization

```sql
-- Show cascade dependency chain
SELECT * FROM pg_tviews_show_cascade_path('user');

-- Sample output:
 depth | entity_name | dependency_type
-------+-------------+-----------------
     0 | user        | cascade
     1 | post        | cascade
     2 | comment     | cascade
     3 | notification| cascade
```

## Performance Bottleneck Identification

```sql
-- Find slowest cascades
SELECT entity,
       COUNT(*) as cascade_count,
       AVG(refresh_duration_ms) as avg_ms,
       MAX(refresh_duration_ms) as max_ms
FROM pg_tview_performance_log  -- To implement
WHERE timestamp > NOW() - INTERVAL '1 hour'
GROUP BY entity
ORDER BY avg_ms DESC;
```
```

**Acceptance Criteria**:
- [ ] Cascade analysis function implemented
- [ ] Dependency path visualization function
- [ ] Query plan output formatted
- [ ] Performance bottleneck queries documented
- [ ] Examples in documentation

---

#### Task 4.3: Cache Size Configuration
**Effort**: 3-4 hours

**New GUC Parameter**:

**File**: `src/config/mod.rs`

```rust
use pgrx::prelude::*;

/// Graph cache size (number of entries)
static mut GRAPH_CACHE_SIZE: usize = 1000;

/// Prepared statement cache size
static mut PREPARED_STMT_CACHE_SIZE: usize = 500;

pub fn init_guc() {
    unsafe {
        GucRegistry::define_int_guc(
            "pg_tviews.graph_cache_size",
            "Maximum number of entries in dependency graph cache",
            "Controls memory usage vs. performance trade-off",
            &raw mut GRAPH_CACHE_SIZE as *mut i32,
            100,     // min
            100000,  // max
            1000,    // default
            GucContext::Suset,
            GucFlags::default(),
        );

        GucRegistry::define_int_guc(
            "pg_tviews.prepared_stmt_cache_size",
            "Maximum number of cached prepared statements",
            "Higher values reduce planning overhead",
            &raw mut PREPARED_STMT_CACHE_SIZE as *mut i32,
            50,      // min
            10000,   // max
            500,     // default
            GucContext::Suset,
            GucFlags::default(),
        );
    }
}

pub fn get_graph_cache_size() -> usize {
    unsafe { GRAPH_CACHE_SIZE }
}

pub fn get_prepared_stmt_cache_size() -> usize {
    unsafe { PREPARED_STMT_CACHE_SIZE }
}
```

**File**: `src/lib.rs` (call in _PG_init)

```rust
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    crate::config::init_guc();
}
```

**Documentation**:

**File**: `docs/reference/configuration.md` (new)

```markdown
# Configuration Parameters

## pg_tviews.graph_cache_size

**Type**: Integer
**Default**: 1000
**Range**: 100 to 100000
**Context**: Superuser (requires restart)

Controls the size of the dependency graph cache. Higher values improve performance for complex dependency chains but use more memory.

**Memory Usage**: ~100 bytes per entry
**Recommendation**:
- Small deployments (<50 TVIEWs): 500
- Medium deployments (50-200 TVIEWs): 1000 (default)
- Large deployments (>200 TVIEWs): 5000

```sql
-- Set in postgresql.conf
pg_tviews.graph_cache_size = 5000

-- Or ALTER SYSTEM
ALTER SYSTEM SET pg_tviews.graph_cache_size = 5000;
SELECT pg_reload_conf();
```

## pg_tviews.prepared_stmt_cache_size

**Type**: Integer
**Default**: 500
**Range**: 50 to 10000
**Context**: Superuser (requires restart)

Controls prepared statement cache size. Higher values reduce query planning overhead.

**Memory Usage**: ~500 bytes per entry
**Recommendation**:
- Low concurrency (<10 connections): 200
- Medium concurrency (10-50 connections): 500 (default)
- High concurrency (>50 connections): 2000

```sql
ALTER SYSTEM SET pg_tviews.prepared_stmt_cache_size = 2000;
SELECT pg_reload_conf();
```
```

**Acceptance Criteria**:
- [ ] GUC parameters registered
- [ ] Cache sizes configurable
- [ ] Configuration documented
- [ ] Memory usage estimates provided
- [ ] Recommendations by deployment size

---

#### Task 4.4: Performance Best Practices
**Effort**: 3-4 hours

**New File**: `docs/user-guides/performance-best-practices.md`

```markdown
# Performance Best Practices

## 1. Install jsonb_ivm Extension

**Impact**: 1.5-3× faster cascade updates

```sql
CREATE EXTENSION jsonb_ivm;
CREATE EXTENSION pg_tviews;

-- Verify
SELECT pg_tviews_check_jsonb_ivm();  -- Should return true
```

## 2. Create Appropriate Indexes

**Impact**: 100-1000× faster queries

```sql
-- Always index foreign keys
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);

-- Index UUID columns for GraphQL
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- GIN index for JSONB queries
CREATE INDEX idx_tv_post_data ON tv_post USING GIN(data);
```

## 3. Enable Statement-Level Triggers for Bulk Operations

**Impact**: 2-5× faster for bulk updates

```sql
SELECT pg_tviews_install_stmt_triggers();
```

**Use when**: Frequently updating >100 rows at once

## 4. Optimize JSONB Structure

**Impact**: Smaller storage, faster queries

```sql
-- ❌ BAD: Deeply nested, redundant data
-- This would be inside a CREATE TABLE tv_post AS SELECT ... FROM ...
jsonb_build_object(
    'post', jsonb_build_object(
        'id', tb_post.id,  -- UUID
        'title', tb_post.title,
        'author', jsonb_build_object(
            'id', tb_user.id,  -- UUID
            'name', tb_user.name,
            'email', tb_user.email,
            'address', jsonb_build_object(...)  -- Too deep!
        )
    )
)

-- ✅ GOOD: Flat structure, essential data only
-- Note: id is UUID, pk_post and fk_user are integers
jsonb_build_object(
    'id', tb_post.id,           -- UUID for GraphQL
    'title', tb_post.title,
    'userId', tb_user.id,        -- UUID for GraphQL (from JOIN)
    'userName', tb_user.name,
    'authorPk', tb_post.fk_user  -- Integer FK for cascade
)
```

## 5. Limit Dependency Depth

**Impact**: Faster cascades, easier debugging

**Recommendation**: Keep dependency chains ≤5 levels

```sql
-- Check current depth
SELECT entity, array_length(dependency_paths, 1) as max_depth
FROM pg_tview_meta
ORDER BY max_depth DESC;

-- If depth >5, consider flattening
-- Instead of: tv_a → tv_b → tv_c → tv_d → tv_e
-- Use: tv_a → tv_e (denormalize intermediate data)
```

## 6. Batch Related Updates

**Impact**: Fewer cascade operations

```sql
-- ❌ BAD: Many small transactions
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 1;
COMMIT;
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 2;
COMMIT;

-- ✅ GOOD: Single transaction
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post IN (1, 2, 3, ...);
COMMIT;

-- Note: pk_post is integer (SERIAL), id is UUID
-- For GraphQL queries by UUID, use: WHERE tb_post.id = '<uuid>'
```

## 7. Monitor and Tune work_mem

**Impact**: Faster sorts and joins in cascade queries

```sql
-- Check current setting
SHOW work_mem;

-- Increase for complex cascades (per session)
SET work_mem = '128MB';

-- Or globally in postgresql.conf
work_mem = 128MB
```

## 8. Use Partitioning for Large TVIEWs

**Impact**: Faster queries, easier maintenance

```sql
-- Partition by date range
-- Note: Trinity pattern - pk_event is SERIAL, id is UUID
CREATE TABLE tv_event (
    pk_event BIGSERIAL,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL,
    data JSONB
) PARTITION BY RANGE (created_at);

-- Create partitions (note: singular naming)
CREATE TABLE tv_event_2025_01 PARTITION OF tv_event
FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');

CREATE TABLE tv_event_2025_02 PARTITION OF tv_event
FOR VALUES FROM ('2025-02-01') TO ('2025-03-01');
```

## 9. Regular Maintenance

**Impact**: Consistent performance over time

```bash
# Weekly: Vacuum and analyze
vacuumdb --analyze --verbose dbname

# Monthly: Reindex if needed
psql -c "REINDEX DATABASE dbname"

# Check for bloat
SELECT schemaname, tablename,
       pg_size_pretty(pg_relation_size(schemaname||'.'||tablename)) as size
FROM pg_tables
WHERE tablename LIKE 'tv_%'
ORDER BY pg_relation_size(schemaname||'.'||tablename) DESC;
```

## 10. Profile Before Optimizing

**Impact**: Focus effort on actual bottlenecks

```sql
-- Enable timing
\timing on

-- Profile a cascade
-- Note: pk_post is integer (SERIAL), id is UUID
EXPLAIN (ANALYZE, BUFFERS)
UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 1;

-- Or by UUID for GraphQL
EXPLAIN (ANALYZE, BUFFERS)
UPDATE tb_post SET title = 'New' WHERE tb_post.id = 'uuid-here';

-- Check slow queries
SELECT query, mean_exec_time, calls
FROM pg_stat_statements
WHERE query LIKE '%tv_%'
ORDER BY mean_exec_time DESC
LIMIT 10;
```

## Performance Checklist

Before going to production:
- [ ] jsonb_ivm extension installed
- [ ] Indexes created on pk_*, fk_*, id columns
- [ ] GIN indexes on data columns (for TVIEWs >10K rows)
- [ ] Statement-level triggers enabled (for bulk operations)
- [ ] Dependency depth ≤5 levels
- [ ] work_mem tuned (≥64MB for medium deployments)
- [ ] Autovacuum configured
- [ ] Monitoring enabled (pg_stat_statements)
```

**Acceptance Criteria**:
- [ ] 10 best practices documented
- [ ] Performance impact quantified for each
- [ ] Code examples provided
- [ ] Performance checklist included
- [ ] Profiling tools explained

---

### Phase 4 Acceptance Criteria

- [ ] Index optimization guide with recommendations
- [ ] Query plan analysis tools implemented
- [ ] Cache configuration parameters added
- [ ] Performance best practices documented
- [ ] Tuning utilities created and documented
- [ ] Performance score: 95/100 ✅

---

## Timeline and Milestones

### Week 1-2: Documentation Excellence (Phase 1)
- Fix unqualified column references
- Standardize examples
- Create migration guides
- Add security documentation

**Milestone**: Documentation score 95/100 ✅

### Week 3-4: Testing & Quality (Phase 2)
- Fix test build issues
- Add concurrent DDL tests
- Implement stress tests
- Improve test assertions

**Milestone**: Testing score 95/100 ✅

### Week 5: Production Readiness (Phase 3)
- Complete monitoring infrastructure
- Create operational runbooks
- Implement audit logging
- Document disaster recovery

**Milestone**: Production Readiness score 98/100 ✅

### Week 6: Performance & Optimization (Phase 4)
- Index optimization guide
- Query analysis tools
- Cache configuration
- Best practices documentation

**Milestone**: Performance score 95/100 ✅

---

## Success Criteria

### Target Scores

| Category | Current | Target | Status |
|----------|---------|--------|--------|
| Code Correctness | 92/100 | 98/100 | Phase 2 |
| Architecture | 90/100 | 96/100 | Phase 3 |
| Documentation | 85/100 | 95/100 | **Phase 1** |
| Testing | 82/100 | 95/100 | **Phase 2** |
| Performance | 88/100 | 95/100 | Phase 4 |
| Production Ready | 84/100 | 98/100 | **Phase 3** |
| **OVERALL** | **87/100** | **95/100** | All Phases |

### Definition of "Excellent"

Each category achieves 95-100/100 when:

**Code Correctness (98/100)**:
- 0 P0/P1 issues
- <5 TODO comments
- Test build works with all configurations
- No panics in any code path
- 90%+ code coverage

**Architecture (96/100)**:
- All monitoring implemented
- Audit logging complete
- Resource limits documented
- Disaster recovery tested

**Documentation (95/100)**:
- All examples verified working
- Security guide complete
- Migration guides tested
- API reference 100% complete

**Testing (95/100)**:
- Concurrent tests passing
- 1M+ row stress tests
- 85%+ code coverage
- All edge cases covered

**Performance (95/100)**:
- Optimization guide complete
- Analysis tools available
- Configuration documented
- Best practices established

**Production Readiness (98/100)**:
- Monitoring complete
- Runbooks tested
- Audit trail enabled
- Recovery procedures verified

---

## Risk Assessment

### Low Risk Tasks
- Documentation fixes (Phase 1)
- Test improvements (Phase 2)
- Best practices guides (Phase 4)

### Medium Risk Tasks
- Monitoring implementation (Phase 3)
- Stress tests (Phase 2)
- Query analysis tools (Phase 4)

### High Risk Tasks
- None identified (all tasks are enhancements, not rewrites)

### Mitigation Strategies
- Incremental changes with frequent testing
- Feature flags for new functionality
- Comprehensive rollback procedures
- Beta testing period before 1.0 release

---

## Post-Excellence Maintenance

After achieving 95/100 across all categories:

### Monthly
- Review and resolve new TODOs
- Update documentation for API changes
- Run stress tests on new hardware

### Quarterly
- Security audit
- Performance benchmarking
- Disaster recovery drill
- Dependency updates

### Annually
- Comprehensive QA re-assessment
- Capacity planning review
- Architectural review

---

**End of Excellence Roadmap**

*This roadmap will evolve as pg_tviews matures. Adjust priorities based on user feedback and production needs.*

---

**Previous Phase**: [03-production-readiness.md](./03-production-readiness.md)
**Back to Index**: [README.md](./README.md)
