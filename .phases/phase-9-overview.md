# Phase 9: Production Hardening & Statement-Level Optimization

**Status:** PLANNED (Post-Phase 8)
**Prerequisites:** Phase 6 Complete ✅, Phase 7 Recommended, Phase 8 Optional
**Estimated Time:** 2-3 weeks
**Priority:** Medium (Production optimization)

---

## Objective

Optimize for production workloads and add statement-level trigger support:

1. **Statement-Level Triggers**: Replace row-level triggers with statement-level for bulk operations
2. **Transition Tables**: Use PostgreSQL's AFTER EACH STATEMENT with transition tables
3. **Bulk Refresh API**: Efficient refresh of multiple rows in single operation
4. **Query Plan Caching**: Cache query plans for refresh operations
5. **Connection Pooling Integration**: Work correctly with pgBouncer, PgPool-II

---

## Context

### Current Architecture Limitations (Phase 6)

**Row-Level Triggers:**
```sql
-- Current: One trigger fire per row
UPDATE tb_user SET name = name || '_updated' WHERE pk_user <= 1000;
-- → 1000 trigger fires
-- → 1000 enqueue_refresh() calls
-- → Overhead: 1000 × 0.001ms = 1ms
```

**Statement-Level Triggers (Phase 9):**
```sql
-- Phase 9: One trigger fire per statement
UPDATE tb_user SET name = name || '_updated' WHERE pk_user <= 1000;
-- → 1 trigger fire
-- → Bulk enqueue from transition table
-- → Overhead: ~0.1ms (10× faster)
```

### PostgreSQL Transition Tables (PG 10+)

```sql
CREATE TRIGGER tview_stmt_trigger
AFTER UPDATE ON tb_user
REFERENCING OLD TABLE AS old_table NEW TABLE AS new_table
FOR EACH STATEMENT
EXECUTE FUNCTION tview_stmt_trigger_handler();
```

**Benefits:**
- Access all changed rows in single trigger
- Bulk operations more efficient
- Reduced context switching

---

## Sub-Phases

Phase 9 is divided into 5 sub-tasks:

| Sub-Phase | Focus | Time | Dependencies |
|-----------|-------|------|--------------|
| **9A** | Statement-Level Triggers | 3 days | Phase 6B ✅ |
| **9B** | Bulk Refresh API | 2 days | Phase 6D ✅ |
| **9C** | Query Plan Caching | 2 days | Phase 6C ✅ |
| **9D** | Connection Pooling Support | 2-3 days | Phase 6A ✅ |
| **9E** | Production Monitoring | 2 days | Phase 7E (optional) |

---

## Phase 9A: Statement-Level Triggers

### Current Row-Level Trigger (Phase 6B)

```rust
#[pg_trigger]
fn pg_tview_trigger_handler(trigger: &PgTrigger) -> Result<...> {
    // Called ONCE PER ROW
    let table_oid = trigger.relation()?.oid();
    let pk_value = extract_pk(trigger)?;  // Single PK

    let entity = entity_for_table(table_oid)?;
    enqueue_refresh(&entity, pk_value)?;  // Enqueue one key

    Ok(None)
}
```

**Problem**: 1000-row UPDATE → 1000 trigger calls → 1000 enqueue operations

### New Statement-Level Trigger (Phase 9A)

```rust
#[pg_trigger]
fn pg_tview_stmt_trigger_handler(trigger: &PgTrigger) -> Result<...> {
    // Called ONCE PER STATEMENT
    let table_oid = trigger.relation()?.oid();
    let entity = entity_for_table(table_oid)?;

    // Access transition table to get all changed PKs
    let changed_pks = extract_pks_from_transition_table(trigger)?;

    // Bulk enqueue
    enqueue_refresh_bulk(&entity, changed_pks)?;

    Ok(None)
}

fn extract_pks_from_transition_table(trigger: &PgTrigger) -> TViewResult<Vec<i64>> {
    // Transition table names are hardcoded in the CREATE TRIGGER statement
    // REFERENCING OLD TABLE AS old_table NEW TABLE AS new_table
    let transition_table_name = match trigger.tg_event()? {
        TrigEvent::Insert => "new_table",  // INSERTs only have NEW
        TrigEvent::Delete => "old_table",  // DELETEs only have OLD
        TrigEvent::Update => "new_table",  // UPDATEs: use NEW for changed rows
        _ => return Ok(Vec::new()),
    };

    // Query transition table for PKs
    let pk_column = get_pk_column_name(trigger.relation()?.oid())?;

    // IMPORTANT: Transition table references don't need quote_ident()
    // They are special PostgreSQL identifiers visible only in trigger context
    let query = format!(
        "SELECT DISTINCT {} FROM {}",
        quote_identifier(&pk_column),
        transition_table_name  // No quoting - it's a special reference
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[&pk_column].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}
```

**SQL Trigger Definition:**

```sql
-- Statement-level trigger with transition tables
CREATE TRIGGER pg_tview_stmt_trigger
AFTER INSERT OR UPDATE OR DELETE ON tb_user
REFERENCING OLD TABLE AS old_table NEW TABLE AS new_table
FOR EACH STATEMENT
EXECUTE FUNCTION pg_tview_stmt_trigger_handler();
```

**Key Points:**
1. Transition table names (`old_table`, `new_table`) are **hardcoded** in the trigger DDL
2. These are **special references** visible only within the trigger function
3. INSERT operations only provide `new_table`
4. DELETE operations only provide `old_table`
5. UPDATE operations provide both, but we use `new_table` for consistency

### Bulk Enqueue

```rust
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) -> TViewResult<()> {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();

        // Insert all keys at once (HashSet deduplicates automatically)
        for pk in pks {
            queue.insert(RefreshKey {
                entity: entity.to_string(),
                pk,
            });
        }
    });

    Ok(())
}
```

**Performance Improvement:**
- 1000 rows: 1ms → 0.1ms (10× faster)
- 10000 rows: 10ms → 0.5ms (20× faster)

---

## Phase 9B: Bulk Refresh API

### Current Approach (Phase 6)

```rust
// Refresh one row at a time
for key in sorted_keys {
    refresh_and_get_parents(&key)?;  // 1 SQL query per row
}
```

**Problem**: 1000 rows = 1000 SQL queries = Slow

### Bulk Refresh API (Phase 9B)

**SECURITY: Uses parameterized queries to prevent SQL injection**

```rust
use pg_sys::quote_identifier;

/// Refresh multiple rows of the same entity in a single operation
pub fn refresh_bulk(entity: &str, pks: Vec<i64>) -> TViewResult<()> {
    if pks.is_empty() {
        return Ok(());
    }

    // Load metadata once
    let meta = TviewMeta::load_by_entity(entity)?
        .ok_or_else(|| TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;

    // Recompute ALL rows in a single query using parameterized ANY($1)
    let view_name = lookup_view_for_source(meta.view_oid)?;
    let pk_col = format!("pk_{}", entity);

    // SAFE: Use ANY($1) with array parameter (prevents SQL injection)
    let query = format!(
        "SELECT * FROM {} WHERE {} = ANY($1)",
        quote_identifier(&view_name),
        quote_identifier(&pk_col)
    );

    Spi::connect(|client| {
        // Create PostgreSQL BIGINT[] array from Vec<i64>
        let rows = client.select(
            &query,
            None,
            Some(vec![(
                PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID),
                pks.clone().into_datum()
            )]),
        )?;

        // Batch update using UPDATE ... FROM unnest()
        let tv_name = relname_from_oid(meta.tview_oid)?;

        // Collect data for update
        let mut update_pks: Vec<i64> = Vec::new();
        let mut update_data: Vec<JsonB> = Vec::new();

        for row in rows {
            let pk: i64 = row[&pk_col].value().unwrap().unwrap();
            let data: JsonB = row["data"].value().unwrap().unwrap();
            update_pks.push(pk);
            update_data.push(data);
        }

        if update_pks.is_empty() {
            return Ok(()); // No rows to update
        }

        // SAFE: Single UPDATE with unnest() (parameterized)
        let update_query = format!(
            "UPDATE {}
             SET data = v.data, updated_at = now()
             FROM (
                 SELECT unnest($1::bigint[]) as pk,
                        unnest($2::jsonb[]) as data
             ) AS v
             WHERE {}.{} = v.pk",
            quote_identifier(&tv_name),
            quote_identifier(&tv_name),
            quote_identifier(&pk_col)
        );

        // Execute batch update with parameters
        client.update(
            &update_query,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID), update_pks.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::JSONBARRAYOID), update_data.into_datum()),
            ]),
        )?;

        Ok(())
    })
}

/// Helper: Quote identifier safely
fn quote_identifier(name: &str) -> String {
    // Use PostgreSQL's quote_ident() for safety
    Spi::get_one_with_args::<String>(
        "SELECT quote_ident($1)",
        Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), name.into_datum())]),
    )
    .unwrap()
    .unwrap_or_else(|| format!("\"{}\"", name.replace("\"", "\"\"")))
}
```

**Performance Improvement:**
- Query count: 1000 queries → 2 queries (**500× fewer queries**)
- Network round-trips: 1000 → 2
- **Realistic speedup: 10-50× faster** (depends on network latency and query complexity)
  - Network-bound workloads: 20-50× faster
  - CPU-bound workloads: 5-10× faster
  - Actual query execution time: Similar (still processing same rows)

---

## Phase 9C: Query Plan Caching

### Problem

```rust
// Every refresh executes the same query pattern
let query = format!("SELECT * FROM v_{} WHERE pk_{} = $1", entity, entity);
Spi::connect(|client| {
    client.select(&query, None, Some(/* pk */))  // Re-parses query every time
})
```

**Overhead**: Query parsing + planning = ~0.5ms per query

### Solution: Prepared Statements with Cache Invalidation

**CRITICAL:** Must invalidate cache on schema changes (ALTER TABLE, DROP/CREATE VIEW, etc.)

```rust
use std::collections::HashMap;
use once_cell::sync::Lazy;
use pg_sys::{CacheRegisterSyscacheCallback, CacheRegisterRelcacheCallback, RELOID};

// Cache prepared statement names per entity
static PREPARED_STATEMENTS: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| {
    Mutex::new(HashMap::new())
});

#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    // Register cache invalidation callbacks
    unsafe {
        // Invalidate on relation (table/view) changes
        CacheRegisterRelcacheCallback(
            Some(tview_relcache_invalidation_callback),
            std::ptr::null_mut(),
        );

        // Invalidate on syscache changes (for good measure)
        CacheRegisterSyscacheCallback(
            RELOID,
            Some(tview_syscache_invalidation_callback),
            std::ptr::null_mut(),
        );
    }
}

/// Called when relation cache is invalidated (ALTER TABLE, DROP, etc.)
#[pg_guard]
unsafe extern "C" fn tview_relcache_invalidation_callback(
    _datum: pg_sys::Datum,
    _oid: pg_sys::Oid,
) {
    // Clear entire prepared statement cache
    if let Ok(mut cache) = PREPARED_STATEMENTS.lock() {
        if !cache.is_empty() {
            info!("TVIEW: Clearing prepared statement cache ({} entries) due to schema change",
                  cache.len());
            cache.clear();
        }
    }
}

/// Called when syscache is invalidated
#[pg_guard]
unsafe extern "C" fn tview_syscache_invalidation_callback(
    _datum: pg_sys::Datum,
    _cache_id: i32,
    _hash_value: u32,
) {
    // Clear entire prepared statement cache
    if let Ok(mut cache) = PREPARED_STATEMENTS.lock() {
        if !cache.is_empty() {
            cache.clear();
        }
    }
}

pub fn refresh_pk_with_cached_plan(entity: &str, pk: i64) -> TViewResult<()> {
    let stmt_name = get_or_prepare_statement(entity)?;

    // Execute with cached plan (no re-parsing)
    Spi::connect(|client| {
        let result = client.select_with_prepared(
            &stmt_name,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk.into_datum())])
        )?;

        // Process result...
        Ok(())
    })
}

fn get_or_prepare_statement(entity: &str) -> TViewResult<String> {
    let mut cache = PREPARED_STATEMENTS.lock().unwrap();

    if let Some(stmt_name) = cache.get(entity) {
        // Verify statement still exists (might have been deallocated)
        let exists = Spi::get_one_with_args::<bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_prepared_statements WHERE name = $1)",
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), stmt_name.clone().into_datum())]),
        )?.unwrap_or(false);

        if exists {
            return Ok(stmt_name.clone());
        } else {
            // Statement was deallocated, remove from cache
            cache.remove(entity);
        }
    }

    // Prepare statement
    let stmt_name = format!("tview_refresh_{}", entity);
    let query = format!(
        "SELECT * FROM v_{} WHERE pk_{} = $1",
        quote_identifier(entity),
        quote_identifier(&format!("pk_{}", entity))
    );

    Spi::run(&format!(
        "PREPARE {} (BIGINT) AS {}",
        quote_identifier(&stmt_name),
        query
    ))?;

    cache.insert(entity.to_string(), stmt_name.clone());
    Ok(stmt_name)
}
```

**Performance Improvement:**
- Query overhead: 0.5ms → 0.05ms (10× faster)
- Planning cost eliminated

---

## Phase 9D: Connection Pooling Support

### Problem with Transaction-Level State

```rust
// Phase 6: Thread-local state
thread_local! {
    static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = ...;
}
```

**Issue with Connection Poolers:**
- PgBouncer (transaction pooling): Connections shared between transactions
- Thread-local state persists across transactions in same connection
- Queue from Transaction A could leak into Transaction B ❌

### Example Failure

```sql
-- Connection 1, Transaction A:
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Queue: {("user", 1)}
COMMIT;
-- Queue processed, but thread-local NOT cleared if callback fails

-- PgBouncer returns connection to pool

-- Connection 1 (reused), Transaction B:
BEGIN;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
-- Queue: {("user", 1), ("post", 1)}  ← WRONG! "user" from Transaction A
COMMIT;
-- Processes both, even though Transaction B only touched "post"
```

### Solution: Explicit Cleanup (Transaction + Session Pooling)

**Handles both transaction pooling AND session pooling:**

```rust
/// Ensure thread-local state is cleared at transaction boundaries
unsafe extern "C" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    match event {
        XACT_EVENT_COMMIT => {
            // Always clear state, even if callback succeeded
            clear_queue();
            reset_scheduled_flag();
        }
        XACT_EVENT_ABORT => {
            // Always clear state on abort
            clear_queue();
            reset_scheduled_flag();
        }
        _ => {}
    }
}

/// Additional safety: Clear on new transaction start
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    unsafe {
        // Register start-of-transaction callback (for transaction pooling)
        pg_sys::RegisterXactCallback(Some(tview_xact_start_callback), std::ptr::null_mut());

        // Register ProcessUtility hook to catch DISCARD ALL (for session pooling)
        PREV_PROCESS_UTILITY_DISCARD = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(tview_process_utility_discard_hook);
    }
}

unsafe extern "C" fn tview_xact_start_callback(event: u32, _arg: *mut c_void) {
    if event == XACT_EVENT_START {
        // Defensive: Clear any leftover state from previous transaction
        clear_queue();
        reset_scheduled_flag();
    }
}

/// Hook ProcessUtility to catch DISCARD ALL (session pooling)
static mut PREV_PROCESS_UTILITY_DISCARD: Option<pg_sys::ProcessUtility_hook_type> = None;

#[pg_guard]
unsafe extern "C" fn tview_process_utility_discard_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    // Check if this is DISCARD ALL
    let query_str = std::ffi::CStr::from_ptr(query_string).to_str().unwrap_or("");

    if query_str.trim().to_uppercase() == "DISCARD ALL" {
        info!("TVIEW: DISCARD ALL detected, clearing all caches");

        // Clear thread-local state
        clear_queue();
        reset_scheduled_flag();

        // Clear global caches
        clear_entity_graph_cache();
        clear_prepared_statement_cache();
        clear_entity_for_table_cache();
    }

    // Call previous hook or standard ProcessUtility
    if let Some(prev_hook) = PREV_PROCESS_UTILITY_DISCARD {
        prev_hook(pstmt, query_string, read_only_tree, context, params,
                  query_env, dest, completion_tag);
    } else {
        pg_sys::standard_ProcessUtility(pstmt, query_string, read_only_tree,
                                       context, params, query_env, dest, completion_tag);
    }
}

/// Clear all global caches (called on DISCARD ALL)
fn clear_entity_graph_cache() {
    if let Ok(mut cache) = ENTITY_GRAPH_CACHE.lock() {
        *cache = None;
    }
}

fn clear_prepared_statement_cache() {
    if let Ok(mut cache) = PREPARED_STATEMENTS.lock() {
        cache.clear();
    }
}

fn clear_entity_for_table_cache() {
    if let Ok(mut cache) = ENTITY_FOR_TABLE_CACHE.lock() {
        cache.clear();
    }
}
```

### Testing with PgBouncer

**Test 1: Transaction Pooling** (most common)

```bash
# Configure PgBouncer for transaction pooling
cat > pgbouncer-transaction.ini <<EOF
[databases]
testdb = host=localhost port=5432 dbname=testdb

[pgbouncer]
pool_mode = transaction
max_client_conn = 100
default_pool_size = 10
EOF

# Start PgBouncer
pgbouncer pgbouncer-transaction.ini

# Test with multiple connections through PgBouncer
for i in {1..100}; do
    psql -h localhost -p 6432 -d testdb -c "
        BEGIN;
        UPDATE tb_user SET name = 'User$i' WHERE pk_user = $i;
        COMMIT;
    "
done

# Verify: No queue leakage between transactions
psql -h localhost -p 6432 -d testdb -c "
    SELECT * FROM pg_tviews_queue_stats();
    -- queue_size should be 0 (no leakage)
"
```

**Test 2: Session Pooling** (with DISCARD ALL)

```bash
# Configure PgBouncer for session pooling
cat > pgbouncer-session.ini <<EOF
[databases]
testdb = host=localhost port=5432 dbname=testdb

[pgbouncer]
pool_mode = session
max_client_conn = 100
default_pool_size = 10
server_reset_query = DISCARD ALL
EOF

# Start PgBouncer
pgbouncer pgbouncer-session.ini

# Test: Verify caches cleared on DISCARD ALL
psql -h localhost -p 6432 -d testdb -c "
    -- Transaction 1
    BEGIN;
    UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
    COMMIT;

    -- Simulate connection return to pool
    DISCARD ALL;

    -- Check: All caches should be cleared
    SELECT * FROM pg_tviews_cache_stats();
    -- graph_cache_size should be 0
    -- prepared_stmt_count should be 0
"
```

---

## Phase 9E: Production Monitoring

### Enhanced Metrics

```sql
-- Real-time queue view
CREATE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    txid_current() as transaction_id,
    COUNT(*) as queue_size,
    array_agg(DISTINCT entity) as entities,
    MAX(enqueued_at) as last_enqueued
FROM pg_tviews_debug_queue()
GROUP BY current_setting('application_name'), txid_current();

-- Historical performance metrics
CREATE TABLE pg_tviews_metrics (
    metric_id BIGSERIAL PRIMARY KEY,
    recorded_at TIMESTAMPTZ DEFAULT now(),
    transaction_id BIGINT,
    queue_size INT,
    refresh_count INT,
    iteration_count INT,
    timing_ms FLOAT,
    graph_cache_hit BOOLEAN,
    table_cache_hits INT
);

-- pg_stat_statements integration
CREATE VIEW pg_tviews_statement_stats AS
SELECT
    query,
    calls,
    total_time,
    mean_time,
    stddev_time
FROM pg_stat_statements
WHERE query LIKE '%pg_tview%' OR query LIKE '%tv_%';
```

### Logging Improvements

```rust
// Structured logging with context
fn handle_pre_commit() -> TViewResult<()> {
    let start = std::time::Instant::now();
    let txid = get_transaction_id();

    info!(
        "TVIEW[txid={}]: Starting commit processing, queue_size={}",
        txid,
        pending.len()
    );

    // ... processing ...

    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    info!(
        "TVIEW[txid={}]: Completed {} refreshes in {} iterations, took {:.2}ms",
        txid,
        processed.len(),
        iteration - 1,
        duration_ms
    );

    // Record metrics if enabled
    if get_enable_metrics() {
        record_metrics(QueueMetrics {
            transaction_id: txid,
            queue_size: pending_initial_size,
            refresh_count: processed.len(),
            iteration_count: iteration - 1,
            timing_ms: duration_ms,
            graph_cache_hit: was_cache_hit,
            table_cache_hits: table_cache_hit_count,
        })?;
    }

    Ok(())
}
```

---

## Files to Create/Modify

### Phase 9A: Statement-Level Triggers
- `src/trigger.rs` - Modify: Add stmt trigger handler
- `sql/tview_stmt_triggers.sql` - New: Statement-level trigger DDL

### Phase 9B: Bulk Refresh API
- `src/refresh/bulk.rs` - New: Bulk refresh implementation
- `src/queue/xact.rs` - Modify: Use bulk refresh where applicable

### Phase 9C: Query Plan Caching
- `src/refresh/cache.rs` - New: Prepared statement cache
- `src/refresh/main.rs` - Modify: Use cached plans

### Phase 9D: Connection Pooling
- `src/queue/xact.rs` - Modify: Add start-of-transaction callback
- `src/queue/state.rs` - Modify: Add defensive clearing

### Phase 9E: Monitoring
- `src/metrics.rs` - Enhance: Add detailed metrics
- `sql/pg_tviews_monitoring.sql` - New: Monitoring views

---

## Testing Strategy

### Unit Tests
- Bulk enqueue logic
- Prepared statement caching
- Cleanup on transaction boundaries

### Integration Tests
```sql
-- Test 1: Statement-level trigger
UPDATE tb_user SET name = name || '_bulk' WHERE pk_user <= 1000;
-- Should fire 1 trigger (not 1000)

-- Test 2: Bulk refresh
SELECT pg_tviews_refresh_bulk('user', ARRAY[1,2,3,4,5]);
-- Should execute 2 queries (not 5)

-- Test 3: Query plan caching
SELECT pg_tviews_check_plan_cache('user');
-- Should return true after first refresh

-- Test 4: Connection pooling
-- Run through PgBouncer, verify no queue leakage
```

### Performance Tests
```sql
-- Benchmark: Row-level vs statement-level triggers
-- 10000-row UPDATE
-- Expected: 10× faster with statement-level

-- Benchmark: Bulk refresh vs individual refresh
-- 1000-row refresh
-- Expected: 100× faster with bulk API

-- Benchmark: Query plan caching
-- 10000 refreshes of same entity
-- Expected: 10× faster with caching
```

---

## Acceptance Criteria

### Phase 9A: Statement-Level Triggers
- ✅ Statement-level triggers installed correctly
- ✅ Transition tables accessed successfully
- ✅ Bulk enqueue works for large statements
- ✅ Performance: 10× faster for bulk operations

### Phase 9B: Bulk Refresh API
- ✅ pg_tviews_refresh_bulk() function works
- ✅ Single-query refresh for multiple rows
- ✅ Performance: 100× faster than row-by-row
- ✅ Integration with commit handler

### Phase 9C: Query Plan Caching
- ✅ Prepared statements cached per entity
- ✅ Cache invalidation on schema changes
- ✅ Performance: 10× faster query execution
- ✅ Memory usage acceptable (< 1MB per entity)

### Phase 9D: Connection Pooling
- ✅ Works correctly with PgBouncer (transaction mode)
- ✅ No queue leakage between transactions
- ✅ Defensive cleanup on transaction start
- ✅ Integration tests pass with pooler

### Phase 9E: Monitoring
- ✅ Enhanced metrics tracked
- ✅ pg_stat_statements integration
- ✅ Real-time queue view works
- ✅ Historical metrics table populated

---

## Performance Targets

| Metric | Phase 6 | Phase 9 | Improvement |
|--------|---------|---------|-------------|
| 1000-row UPDATE trigger overhead | 1ms | 0.1ms | **10× faster trigger** |
| 1000-row bulk refresh (queries) | 1000 queries | 2 queries | **500× fewer queries** |
| 1000-row bulk refresh (actual time) | Variable | Variable | **10-50× faster** (workload-dependent) |
| Query parsing overhead | 0.5ms/query | 0.05ms/query | **10× faster parsing** |
| Connection pool safety | Risky | Safe | **Production-ready** |

**Note:** "500× fewer queries" does not mean "500× faster execution". Actual speedup depends on:
- Network latency (higher latency = bigger speedup from fewer round-trips)
- Query complexity (simple queries = parsing overhead dominates)
- Lock contention (bulk operations may reduce lock overhead)
- I/O patterns (fewer queries = better cache utilization)

---

## Known Limitations After Phase 9

1. **Cross-Database TVIEWs**: Still not supported (requires FDW integration)
2. **Logical Replication**: Not yet integrated
3. **Partitioned Tables**: Limited support

---

## Success Metrics

- **Performance**: 10-50× faster for bulk operations (workload-dependent)
- **Safety**: Works correctly with connection poolers (transaction + session modes)
- **Observability**: Production metrics visible
- **Efficiency**: Reduced query count by 100-500× (fewer round-trips)

---

## Read Next

- `.phases/phase-9a-statement-triggers.md` - Detailed implementation
- `.phases/phase-9b-bulk-refresh.md` - Detailed implementation
- `.phases/phase-9c-query-caching.md` - Detailed implementation
- `.phases/phase-9d-connection-pooling.md` - Detailed implementation
- `.phases/phase-9e-monitoring.md` - Detailed implementation
