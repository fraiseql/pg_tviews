# Phase 7: Performance Optimizations & Edge Case Handling

**Status:** PLANNED (Post-Phase 6)
**Prerequisites:** Phase 6 (A+B+C+D) Complete ✅
**Estimated Time:** 1-2 weeks
**Priority:** Medium (Production enhancements)

---

## Objective

Implement performance optimizations and handle edge cases identified during Phase 6 implementation:

1. **Graph Caching**: Cache EntityDepGraph to avoid pg_tview_meta queries per transaction
2. **entity_for_table() Caching**: Cache table OID → entity mapping
3. **Savepoint Support**: Handle SAVEPOINT/ROLLBACK TO correctly
4. **Configurable Limits**: Add GUC settings for propagation depth and other tunables
5. **Monitoring & Observability**: Add instrumentation for queue metrics

---

## Context

Phase 6 implementation revealed several optimization opportunities:

### Performance Bottlenecks
- **Graph Loading**: ~5ms per transaction (queries pg_tview_meta every time)
- **entity_for_table()**: ~0.1ms per trigger (queries pg_class every time)
- **Total Overhead**: ~10ms per transaction with 50 triggers

### Edge Cases
- **Savepoint rollback**: Queue not cleaned (documented limitation)
- **Deep chains**: Hardcoded 100-iteration limit
- **No visibility**: Can't monitor queue size or iteration count

### Production Needs
- **Observability**: Need metrics for queue size, refresh count, timing
- **Configurability**: Need tunables for different workload patterns
- **Edge case handling**: Need savepoint support for frameworks that use them

---

## Sub-Phases

Phase 7 is divided into 5 sub-tasks:

| Sub-Phase | Focus | Time | Dependencies |
|-----------|-------|------|--------------|
| **7A** | Graph Caching | 1 day | Phase 6D ✅ |
| **7B** | entity_for_table() Caching | 1 day | Phase 6B ✅ |
| **7C** | Savepoint Support | 2-3 days | Phase 6C ✅ |
| **7D** | GUC Configuration | 1 day | Phase 6C/6D ✅ |
| **7E** | Monitoring & Observability | 2 days | Phase 6C ✅ |

---

## Phase 7A: Graph Caching

### Problem
```rust
fn handle_pre_commit() -> TViewResult<()> {
    // Loads graph on EVERY commit
    let graph = EntityDepGraph::load()?;  // ~5ms per transaction
    // ...
}
```

**Impact**: 5ms overhead per transaction, scales with entity count

### Solution
```rust
use once_cell::sync::Lazy;
use std::sync::Mutex;

static ENTITY_GRAPH_CACHE: Lazy<Mutex<Option<EntityDepGraph>>> = Lazy::new(|| {
    Mutex::new(None)
});

impl EntityDepGraph {
    pub fn load_cached() -> TViewResult<Self> {
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();

        if let Some(graph) = cache.as_ref() {
            return Ok(graph.clone());
        }

        let graph = Self::load()?;
        *cache = Some(graph.clone());
        Ok(graph)
    }

    pub fn invalidate_cache() {
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();
        *cache = None;
    }
}
```

**Invalidation Points:**
- CREATE TVIEW → invalidate
- DROP TVIEW → invalidate
- ALTER TVIEW (future) → invalidate

**Expected Improvement**: 5ms → 0.001ms (5000× faster)

---

## Phase 7B: entity_for_table() Caching

### Problem
```rust
// Called on EVERY trigger fire
let entity = entity_for_table(table_oid)?;  // ~0.1ms (queries pg_class)
```

**Impact**: 0.1ms per trigger × 100 triggers = 10ms overhead per transaction

### Solution
```rust
static TABLE_ENTITY_CACHE: Lazy<Mutex<HashMap<Oid, String>>> = Lazy::new(|| {
    Mutex::new(HashMap::new())
});

pub fn entity_for_table_cached(table_oid: Oid) -> TViewResult<Option<String>> {
    // Fast path: check cache
    {
        let cache = TABLE_ENTITY_CACHE.lock().unwrap();
        if let Some(entity) = cache.get(&table_oid) {
            return Ok(Some(entity.clone()));
        }
    }

    // Slow path: query and cache
    let entity = entity_for_table_uncached(table_oid)?;

    if let Some(ref e) = entity {
        let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
        cache.insert(table_oid, e.clone());
    }

    Ok(entity)
}
```

**Expected Improvement**: 0.1ms → 0.001ms per trigger (100× faster)

---

## Phase 7C: Savepoint Support

### Problem
```sql
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Queue: {("user", 1)}

SAVEPOINT sp1;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
-- Queue: {("user", 1), ("post", 1)}

ROLLBACK TO sp1;
-- Queue NOT cleaned → still has ("post", 1) ❌

COMMIT;
-- Tries to refresh ("post", 1) but update was rolled back
```

### Solution
```rust
// Track savepoint depth and queue snapshots
thread_local! {
    static SAVEPOINT_DEPTH: RefCell<usize> = RefCell::new(0);
    static QUEUE_SNAPSHOTS: RefCell<Vec<HashSet<RefreshKey>>> = RefCell::new(Vec::new());
}

unsafe extern "C" fn tview_subxact_callback(
    event: pg_sys::SubXactEvent,
    _subxid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut c_void,
) {
    match event {
        pg_sys::SubXactEvent_SUBXACT_EVENT_START_SUB => {
            // Savepoint created: snapshot current queue
            SAVEPOINT_DEPTH.with(|d| *d.borrow_mut() += 1);
            TX_REFRESH_QUEUE.with(|q| {
                let snapshot = q.borrow().clone();
                QUEUE_SNAPSHOTS.with(|s| s.borrow_mut().push(snapshot));
            });
        }
        pg_sys::SubXactEvent_SUBXACT_EVENT_ABORT_SUB => {
            // ROLLBACK TO: restore queue snapshot
            SAVEPOINT_DEPTH.with(|d| *d.borrow_mut() -= 1);
            if let Some(snapshot) = QUEUE_SNAPSHOTS.with(|s| s.borrow_mut().pop()) {
                TX_REFRESH_QUEUE.with(|q| *q.borrow_mut() = snapshot);
            }
        }
        pg_sys::SubXactEvent_SUBXACT_EVENT_COMMIT_SUB => {
            // Savepoint committed: discard snapshot
            SAVEPOINT_DEPTH.with(|d| *d.borrow_mut() -= 1);
            QUEUE_SNAPSHOTS.with(|s| s.borrow_mut().pop());
        }
        _ => {}
    }
}
```

**Complexity**: Medium (requires PostgreSQL subxact API)

---

## Phase 7D: GUC Configuration

### Configuration Settings

Add PostgreSQL GUC (Grand Unified Configuration) settings:

```sql
-- Maximum propagation depth (default: 100)
SET pg_tviews.max_propagation_depth = 200;

-- Enable/disable graph caching (default: true)
SET pg_tviews.enable_graph_cache = true;

-- Enable/disable entity_for_table caching (default: true)
SET pg_tviews.enable_table_cache = true;

-- Log level for TVIEW operations (default: 'info')
SET pg_tviews.log_level = 'debug';  -- debug, info, warning, error

-- Enable performance metrics (default: false)
SET pg_tviews.enable_metrics = true;
```

### Implementation
```rust
use pgrx::GucRegistry;

static MAX_PROPAGATION_DEPTH: GucSetting<i32> = GucSetting::new(100);
static ENABLE_GRAPH_CACHE: GucSetting<bool> = GucSetting::new(true);
static ENABLE_TABLE_CACHE: GucSetting<bool> = GucSetting::new(true);
static LOG_LEVEL: GucSetting<String> = GucSetting::new("info".to_string());
static ENABLE_METRICS: GucSetting<bool> = GucSetting::new(false);

#[pg_guard]
pub extern "C" fn _PG_init() {
    GucRegistry::define_int_guc(
        "pg_tviews.max_propagation_depth",
        "Maximum propagation iteration depth",
        "Prevents infinite loops in dependency chains",
        &MAX_PROPAGATION_DEPTH,
        10,
        1000,
        GucContext::Userset,
        GucFlags::empty(),
    );

    // ... other GUCs ...
}
```

---

## Phase 7E: Monitoring & Observability

### Metrics to Track

1. **Queue Size**: Number of pending refreshes
2. **Refresh Count**: Total refreshes per transaction
3. **Iteration Count**: Propagation loop iterations
4. **Timing**: Time spent in commit handler
5. **Cache Hit Rate**: Graph cache and table cache hits

### SQL Functions

```sql
-- Get current queue statistics
CREATE FUNCTION pg_tviews_queue_stats()
RETURNS TABLE (
    queue_size INT,
    pending_refreshes INT,
    savepoint_depth INT,
    cache_hits BIGINT,
    cache_misses BIGINT
);

-- Debug: View current queue contents
CREATE FUNCTION pg_tviews_debug_queue()
RETURNS TABLE (
    entity TEXT,
    pk BIGINT
);

-- Get refresh statistics for last N transactions
CREATE FUNCTION pg_tviews_refresh_stats()
RETURNS TABLE (
    total_refreshes BIGINT,
    avg_iterations FLOAT,
    max_iterations INT,
    avg_timing_ms FLOAT,
    max_timing_ms FLOAT
);
```

### Implementation
```rust
// Track metrics in thread-local storage
thread_local! {
    static METRICS: RefCell<QueueMetrics> = RefCell::new(QueueMetrics::default());
}

struct QueueMetrics {
    total_refreshes: u64,
    total_iterations: u64,
    total_timing_ms: f64,
    max_iterations: usize,
    graph_cache_hits: u64,
    graph_cache_misses: u64,
    table_cache_hits: u64,
    table_cache_misses: u64,
}

// Update metrics during commit processing
fn handle_pre_commit() -> TViewResult<()> {
    let start = std::time::Instant::now();

    // ... existing logic ...

    // Update metrics
    METRICS.with(|m| {
        let mut metrics = m.borrow_mut();
        metrics.total_refreshes += processed.len() as u64;
        metrics.total_iterations += iteration as u64;
        metrics.max_iterations = metrics.max_iterations.max(iteration);
        metrics.total_timing_ms += start.elapsed().as_secs_f64() * 1000.0;
    });
}
```

---

## Files to Create

### Phase 7A: Graph Caching
- `src/queue/graph.rs` - Modify: Add `load_cached()`, `invalidate_cache()`
- `src/queue/cache.rs` - New: Cache infrastructure

### Phase 7B: entity_for_table() Caching
- `src/catalog.rs` - Modify: Add `entity_for_table_cached()`
- `src/queue/cache.rs` - Modify: Add table cache

### Phase 7C: Savepoint Support
- `src/queue/xact.rs` - Modify: Add `tview_subxact_callback()`
- `src/queue/state.rs` - Modify: Add savepoint tracking

### Phase 7D: GUC Configuration
- `src/config.rs` - New: GUC settings
- `src/lib.rs` - Modify: Register GUCs in _PG_init()

### Phase 7E: Monitoring
- `src/metrics.rs` - New: Metrics tracking
- `src/lib.rs` - Modify: Add SQL functions for metrics

---

## Testing Strategy

### Unit Tests
- Cache hit/miss behavior
- Savepoint snapshot/restore
- GUC setting parsing
- Metrics accumulation

### Integration Tests
```sql
-- Test 1: Graph cache invalidation
CREATE TVIEW tv_test AS SELECT ...;
-- First transaction: cache miss
BEGIN; UPDATE tb_test ...; COMMIT;
-- Second transaction: cache hit
BEGIN; UPDATE tb_test ...; COMMIT;
DROP TVIEW tv_test;
-- Third transaction: cache miss (invalidated)
BEGIN; UPDATE tb_test2 ...; COMMIT;

-- Test 2: Savepoint support
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
SAVEPOINT sp1;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
ROLLBACK TO sp1;
COMMIT;
-- Queue should only have ("user", 1), not ("post", 1)

-- Test 3: GUC configuration
SET pg_tviews.max_propagation_depth = 10;
-- Create deep dependency chain (>10 levels)
BEGIN; UPDATE ...; COMMIT;
-- Should error with PropagationDepthExceeded

-- Test 4: Metrics
SELECT * FROM pg_tviews_queue_stats();
-- Should show current queue state
```

### Performance Tests
```sql
-- Benchmark: Graph caching impact
-- Run 1000 transactions, measure avg timing
-- Expected: 5ms → 0.1ms (50× faster)

-- Benchmark: Table cache impact
-- Run 1000 triggers, measure avg timing
-- Expected: 0.1ms → 0.001ms per trigger (100× faster)
```

---

## Acceptance Criteria

### Phase 7A: Graph Caching
- ✅ EntityDepGraph cached after first load
- ✅ Cache invalidated on CREATE/DROP TVIEW
- ✅ Transaction timing improved by 5ms
- ✅ Cache hit rate > 95% in production workloads

### Phase 7B: entity_for_table() Caching
- ✅ Table OID → entity mapping cached
- ✅ Cache invalidated on CREATE/DROP TVIEW
- ✅ Trigger overhead reduced from 0.1ms to 0.001ms
- ✅ Cache hit rate > 99% in production workloads

### Phase 7C: Savepoint Support
- ✅ SAVEPOINT creates queue snapshot
- ✅ ROLLBACK TO restores queue snapshot
- ✅ RELEASE SAVEPOINT discards snapshot
- ✅ Nested savepoints handled correctly
- ✅ Integration tests pass

### Phase 7D: GUC Configuration
- ✅ All GUC settings registered
- ✅ Settings changeable at runtime (SET command)
- ✅ Settings persisted per session
- ✅ Invalid values rejected with clear errors
- ✅ Documentation updated

### Phase 7E: Monitoring
- ✅ pg_tviews_queue_stats() returns current metrics
- ✅ pg_tviews_debug_queue() shows queue contents
- ✅ pg_tviews_refresh_stats() returns historical data
- ✅ Metrics overhead < 1% of refresh time
- ✅ Metrics viewable via pg_stat_statements integration

---

## Performance Targets

| Metric | Before Phase 7 | After Phase 7 | Improvement |
|--------|----------------|---------------|-------------|
| Graph load | 5ms per txn | 0.001ms per txn | **5000× faster** |
| Trigger overhead | 0.1ms per trigger | 0.001ms per trigger | **100× faster** |
| 100-trigger txn | 10ms overhead | 0.1ms overhead | **100× faster** |
| Total txn overhead | ~10-15ms | ~0.1-0.2ms | **50-100× faster** |

**Expected Production Impact:**
- High-frequency workloads: 50-100× faster
- Low-frequency workloads: Minimal improvement (already fast)
- Memory usage: +5MB for caches (negligible)

---

## Rollout Plan

### Phase 7A + 7B (Caching)
1. **Week 1**: Implement graph caching
2. **Week 1**: Implement table caching
3. **Week 1**: Integration testing
4. **Week 1**: Performance benchmarking
5. **Week 1**: Deploy to staging, monitor for 2 days
6. **Week 1**: Deploy to production

### Phase 7C (Savepoints)
1. **Week 2**: Implement SubXactCallback
2. **Week 2**: Test nested savepoints
3. **Week 2**: Integration testing
4. **Week 2**: Deploy to staging, monitor for 2 days
5. **Week 2**: Deploy to production

### Phase 7D + 7E (Config & Monitoring)
1. **Week 2**: Implement GUC settings
2. **Week 2**: Implement metrics tracking
3. **Week 2**: Add SQL functions
4. **Week 2**: Documentation
5. **Week 2**: Deploy to staging, monitor for 2 days
6. **Week 2**: Deploy to production

---

## Known Limitations After Phase 7

1. **Prepared Transactions (2PC)**: Still not supported (Phase 8)
2. **Parallel Refresh**: Still single-threaded (Phase 9)
3. **Statement-Level Triggers**: Still row-level (Phase 10)

---

## Success Metrics

- **Performance**: 50-100× faster commit overhead
- **Stability**: No regressions in existing functionality
- **Observability**: Production metrics visible via SQL
- **Configurability**: Tunables for different workload patterns
- **Edge Cases**: Savepoints handled correctly

---

## Read Next

- `.phases/phase-7a-graph-caching.md` - Detailed implementation plan
- `.phases/phase-7b-table-caching.md` - Detailed implementation plan
- `.phases/phase-7c-savepoint-support.md` - Detailed implementation plan
- `.phases/phase-7d-guc-configuration.md` - Detailed implementation plan
- `.phases/phase-7e-monitoring.md` - Detailed implementation plan
