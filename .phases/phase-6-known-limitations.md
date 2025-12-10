# Phase 6: Known Limitations and Edge Cases

**Status:** Documentation
**Last Updated:** 2025-12-10
**Applies To:** Phase 6A-6D Implementation

---

## Overview

This document catalogs known limitations of the Phase 6 transaction-queue architecture that are **out of scope** for initial implementation but should be considered for future enhancements.

---

## 1. Savepoint Rollback (SAVEPOINT/ROLLBACK TO)

### Issue

PostgreSQL subtransactions (savepoints) are not integrated with the refresh queue. Queue entries added after a savepoint are **not removed** when rolling back to that savepoint.

### Example Failure

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Queue: {("user", 1)}

SAVEPOINT sp1;

UPDATE tb_user SET email = 'alice@example.com' WHERE pk_user = 1;
-- Queue: {("user", 1)} -- deduplicated, no change

UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
-- Queue: {("user", 1), ("post", 1)}

ROLLBACK TO sp1;
-- ⚠️ Queue NOT cleaned - still contains ("post", 1)
-- Expected: Queue = {("user", 1)}
-- Actual: Queue = {("user", 1), ("post", 1)}

COMMIT;
-- Both user AND post refreshed, but post update was rolled back!
-- Result: tv_post contains stale data OR refresh fails (row not found)
```

### Impact

- **Correctness:** TVIEWs may contain data from rolled-back operations
- **Failures:** Refresh may fail if rolled-back row no longer exists
- **Frequency:** Low (savepoints are uncommon in typical workloads)

### Workaround

Use full `ROLLBACK` instead of `ROLLBACK TO savepoint`:

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- If something goes wrong:
ROLLBACK;  -- ✅ Clears entire queue (ABORT event)
```

### Future Fix (Phase 7+)

Register `SubXactCallback` to handle savepoint-specific events:

```rust
// src/queue/xact.rs (Phase 7 enhancement)
use pgrx::pg_sys::{SubXactEvent, SubXactCallback};

/// Track savepoint depth to implement rollback-to-savepoint cleanup
thread_local! {
    static SAVEPOINT_DEPTH: RefCell<usize> = RefCell::new(0);
    static QUEUE_SNAPSHOTS: RefCell<Vec<HashSet<RefreshKey>>> = RefCell::new(Vec::new());
}

#[no_mangle]
unsafe extern "C" fn tview_subxact_callback(
    event: pg_sys::SubXactEvent,
    subxid: pg_sys::SubTransactionId,
    parentSubid: pg_sys::SubTransactionId,
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

// Register in _PG_init():
unsafe {
    pg_sys::RegisterSubXactCallback(Some(tview_subxact_callback), std::ptr::null_mut());
}
```

**Complexity:** Medium (requires stack of queue snapshots)
**Priority:** Low (savepoints are rarely used)

---

## 2. Prepared Transactions (Two-Phase Commit)

### Issue

Transaction-local queue is stored in `thread_local!` storage, which is **lost when the database connection terminates**. Prepared transactions survive connection termination, but the queue does not.

### Example Failure

```sql
-- Connection 1:
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Queue: {("user", 1)} stored in Connection 1's thread-local storage

PREPARE TRANSACTION 'xact_42';
-- Transaction prepared, but queue is still in thread-local storage
-- Connection 1 terminates

-- Queue is LOST (thread-local storage destroyed)

-- Connection 2 (later):
COMMIT PREPARED 'xact_42';
-- ⚠️ Transaction commits WITHOUT refreshing TVIEWs
-- Result: tv_user and dependent views remain stale
```

### Impact

- **Correctness:** TVIEWs become inconsistent with base tables
- **Silent Failure:** No error raised (transaction commits successfully)
- **Frequency:** Very low (2PC is rare, mostly used in distributed systems)

### Workaround

**Do not use PREPARE TRANSACTION with databases containing TVIEWs.**

Document restriction:
```markdown
## Restrictions
- PREPARE TRANSACTION is not supported with TVIEWs
- Use standard single-phase commit (BEGIN/COMMIT)
```

### Future Fix (Phase 7+)

Serialize queue to persistent table during PREPARE:

```sql
-- Phase 7 enhancement: Persistent queue storage
CREATE TABLE pg_tview_pending_refreshes (
    gid TEXT PRIMARY KEY,  -- Global transaction ID from PREPARE
    refresh_queue JSONB NOT NULL,  -- Serialized RefreshKey[]
    prepared_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX ON pg_tview_pending_refreshes(prepared_at);
```

**Implementation:**

```rust
// During PREPARE TRANSACTION:
fn handle_prepare(gid: &str) -> TViewResult<()> {
    let queue = TX_REFRESH_QUEUE.with(|q| q.borrow().clone());

    if !queue.is_empty() {
        // Serialize queue to table
        let queue_json = serde_json::to_value(&queue)?;
        Spi::run(&format!(
            "INSERT INTO pg_tview_pending_refreshes (gid, refresh_queue) VALUES ($1, $2)",
        ))?;
    }

    Ok(())
}

// During COMMIT PREPARED:
fn handle_commit_prepared(gid: &str) -> TViewResult<()> {
    // Load queue from table
    let queue_json = Spi::get_one::<JsonB>(&format!(
        "SELECT refresh_queue FROM pg_tview_pending_refreshes WHERE gid = $1",
    ))?;

    if let Some(queue_data) = queue_json {
        let queue: HashSet<RefreshKey> = serde_json::from_value(queue_data.0)?;

        // Process queue (same as normal commit)
        process_refresh_queue(queue)?;

        // Clean up
        Spi::run(&format!(
            "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1",
        ))?;
    }

    Ok(())
}
```

**Complexity:** High (requires hooking into PREPARE/COMMIT PREPARED)
**Priority:** Very low (2PC is rarely used)

---

## 3. Very Deep Dependency Chains (>100 Levels)

### Issue

Phase 6D has a hardcoded safety limit of 100 propagation iterations to prevent infinite loops:

```rust
// src/queue/xact.rs (Phase 6D)
fn handle_pre_commit() -> TViewResult<()> {
    let mut iteration = 1;
    while !pending.is_empty() {
        // Process batch...

        iteration += 1;
        if iteration > 100 {
            return Err(TViewError::PropagationDepthExceeded {
                max_depth: 100,
                processed: processed.len(),
            });
        }
    }
}
```

### Example Failure

```sql
-- Hypothetical schema with 150 nested dependencies:
-- tv_entity_1 → tv_entity_2 → ... → tv_entity_150

UPDATE tb_entity_1 SET data = 'X' WHERE pk = 1;
COMMIT;

-- Error: "Propagation exceeded maximum depth of 100 iterations"
-- Transaction aborted
```

### Impact

- **Correctness:** Transaction aborts (safe - no stale data)
- **Usability:** Legitimate deep cascades are blocked
- **Frequency:** Extremely low (realistic schemas have 3-10 levels)

### Realistic Limits

| Dependency Depth | Likelihood | Example |
|------------------|------------|---------|
| 1-5 levels | Very common | user → post → comment |
| 6-10 levels | Common | company → department → team → user → post → ... |
| 11-20 levels | Uncommon | Deep organizational hierarchies |
| 21-50 levels | Rare | Pathological schema design |
| 51-100 levels | Extremely rare | Likely indicates schema problem |
| >100 levels | Never seen in practice | Error is justified |

### Workaround

**Option 1:** Refactor schema to reduce depth
```sql
-- Instead of: A → B → C → D → E → F → G
-- Use: A → [B, C, D, E, F, G] (fan-out, not chain)
```

**Option 2:** Split transaction into smaller batches
```sql
-- Instead of: One transaction updating all 150 entities
BEGIN;
UPDATE tb_entity_1 SET data = 'X' WHERE pk = 1;
COMMIT;  -- Processes 50 levels

BEGIN;
UPDATE tb_entity_51 SET data = 'Y' WHERE pk = 1;
COMMIT;  -- Processes remaining levels
```

### Future Fix (Phase 7+)

Make limit configurable via GUC setting:

```rust
// src/config.rs
pgrx::extension_sql!(
    r#"
    -- Default: 100 (conservative)
    -- Increase if legitimate deep cascades exist
    -- Decrease to detect infinite loops faster
    SET pg_tviews.max_propagation_depth = 100;
    "#,
    name = "pg_tviews_guc_max_propagation_depth"
);

pub fn get_max_propagation_depth() -> usize {
    Spi::get_one::<i32>("SHOW pg_tviews.max_propagation_depth")
        .unwrap_or(Some(100))
        .unwrap_or(100) as usize
}
```

```rust
// src/queue/xact.rs (use dynamic limit)
if iteration > get_max_propagation_depth() {
    return Err(TViewError::PropagationDepthExceeded { ... });
}
```

**Complexity:** Low (simple GUC setting)
**Priority:** Low (current limit is sufficient for 99.9% of cases)

---

## 4. Foreign Key Cascade Timing (Rare Race Condition)

### Issue

PostgreSQL executes foreign key cascades (ON DELETE CASCADE) **before** row-level triggers fire. If cascade deletes rows that have pending queue entries, the queue may contain stale entries.

### Example Scenario

```sql
CREATE TABLE tb_post (
    pk_post INT PRIMARY KEY,
    fk_user INT REFERENCES tb_user(pk_user) ON DELETE CASCADE
);

BEGIN;

-- Step 1: Update post (enqueues refresh)
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
-- Queue: {("post", 1)}

-- Step 2: Delete user (cascades to tb_post)
DELETE FROM tb_user WHERE pk_user = 1;
-- FK cascade deletes tb_post row 1 (no trigger because cascade happens first)
-- Queue: {("post", 1)} -- STALE (row deleted)

COMMIT;
-- Tries to refresh ("post", 1) but row doesn't exist in v_post
-- Result: Refresh fails OR returns empty (depends on view definition)
```

### Impact

- **Correctness:** Refresh attempts on non-existent rows
- **Error Handling:** Depends on view definition (may fail or succeed with no-op)
- **Frequency:** Low (requires ON DELETE CASCADE + within-transaction delete)

### Behavior

**If view returns NULL for deleted row:**
```rust
// refresh_pk() tries to recompute from v_post
// v_post WHERE pk_post = 1 → No rows
// apply_patch() skips (no row to patch)
// Result: No error, but no refresh either (acceptable)
```

**If view JOIN fails:**
```rust
// v_post JOINs tb_user WHERE fk_user = 1
// tb_user row deleted → JOIN returns nothing
// refresh_pk() → No row found → Error
// Result: Transaction aborts (conservative, safe)
```

### Workaround

**Design schema to avoid cascades within transactions:**
```sql
-- Instead of ON DELETE CASCADE:
CREATE TABLE tb_post (
    pk_post INT PRIMARY KEY,
    fk_user INT REFERENCES tb_user(pk_user)  -- No CASCADE
);

-- Handle deletions explicitly:
BEGIN;
DELETE FROM tb_post WHERE fk_user = 1;  -- Explicit delete (triggers fire)
DELETE FROM tb_user WHERE pk_user = 1;  -- Then delete user
COMMIT;
```

### Future Fix (Phase 7+)

**Option 1: Ignore refresh failures for deleted rows**
```rust
fn refresh_and_get_parents(key: &RefreshKey) -> TViewResult<Vec<RefreshKey>> {
    // Try to refresh
    match crate::refresh::refresh_pk(meta.view_oid, key.pk) {
        Ok(_) => {},
        Err(TViewError::RowNotFound { .. }) => {
            // Row deleted during transaction - ignore
            warning!("Refresh skipped for {}[{}]: row deleted", key.entity, key.pk);
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    }
    // ...
}
```

**Option 2: Track deleted rows and remove from queue**
```rust
// Install DELETE trigger to remove from queue
#[pg_trigger]
fn tview_delete_trigger(trigger: &PgTrigger) -> Result<...> {
    let entity = entity_for_table(table_oid)?;
    let pk = extract_pk(trigger)?;

    // Remove from queue if present
    remove_from_queue(&entity, pk)?;

    Ok(None)
}
```

**Complexity:** Medium (requires additional trigger logic)
**Priority:** Low (current fail-fast behavior is safe)

---

## 5. High-Frequency Updates Within Single Transaction

### Issue

If a single transaction performs **thousands of updates to different rows**, the queue can grow very large, consuming memory.

### Example

```sql
BEGIN;

-- Update 10,000 users
DO $$
BEGIN
  FOR i IN 1..10000 LOOP
    UPDATE tb_user SET name = 'User ' || i WHERE pk_user = i;
  END LOOP;
END $$;

-- Queue contains 10,000 entries
-- Estimated memory: 10,000 * (sizeof(String) + sizeof(i64)) ≈ 500KB

COMMIT;
-- Processes 10,000 refreshes sequentially
```

### Impact

- **Memory:** ~50 bytes per queue entry (10K entries ≈ 500KB)
- **Performance:** Sequential processing (no parallelization)
- **Practical Limit:** ~100K entries before memory concerns

### Mitigation

Phase 6 already includes batch refresh optimization:

```rust
// src/propagate.rs (already implemented in Phase 5)
if affected_pks.len() >= 10 {
    info!("Using batch refresh for {} rows", affected_pks.len());
    batch::refresh_batch(&parent_entity, &affected_pks)?;
}
```

**Batch refresh processes multiple rows in a single SQL query** (much faster).

### Future Enhancements

**Option 1: Streaming queue processing**
```rust
// Instead of: take_queue_snapshot() → process all
// Use: Iterate queue in chunks of 1000
```

**Option 2: Parallel refresh (requires worker processes)**
```rust
// Use PostgreSQL background workers to process queue in parallel
// Complexity: Very high
// Benefit: 2-4× speedup for large queues
```

**Complexity:** High
**Priority:** Low (current batch optimization is sufficient)

---

## Summary Table

| Limitation | Impact | Frequency | Workaround | Future Fix Priority |
|------------|--------|-----------|------------|-------------------|
| **Savepoint rollback** | Stale data | Low | Use ROLLBACK | Medium (Phase 7) |
| **Prepared transactions** | Silent failure | Very low | Don't use 2PC | Low (Phase 8) |
| **Deep chains (>100)** | Transaction abort | Extremely low | Refactor schema | Low (Phase 7 GUC) |
| **FK cascade timing** | Refresh failure | Low | Explicit deletes | Low (Phase 7) |
| **Large queues** | Memory/perf | Low | Batch refresh works | Very low |

---

## Recommendations

### For Phase 6 Deployment

1. **Document restrictions clearly:**
   - ❌ PREPARE TRANSACTION not supported
   - ⚠️ SAVEPOINT/ROLLBACK TO may cause stale data
   - ⚠️ Dependency chains limited to 100 levels

2. **Add runtime checks:**
   ```rust
   // Detect 2PC and warn user
   if is_prepared_transaction() {
       warning!("PREPARE TRANSACTION with TVIEWs is not supported. Use standard COMMIT.");
   }
   ```

3. **Monitor in production:**
   - Track `PropagationDepthExceeded` errors (indicates schema issues)
   - Monitor queue sizes (log warning if >10K entries)

### For Future Phases

**Phase 7 (Savepoint support):**
- Implement `SubXactCallback`
- Add queue snapshot stack
- Test with complex savepoint scenarios

**Phase 8 (2PC support - optional):**
- Add `pg_tview_pending_refreshes` table
- Hook into PREPARE/COMMIT PREPARED
- Test with distributed transactions

**Phase 9 (Performance - optional):**
- Make propagation depth configurable
- Add parallel refresh using background workers
- Implement streaming queue processing

---

## Testing Edge Cases

Add these test cases to Phase 6D integration tests:

```sql
-- Test 1: Savepoint rollback (expect stale data with warning)
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
SAVEPOINT sp1;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
ROLLBACK TO sp1;
COMMIT;
-- Check: tv_user updated, tv_post NOT updated (current behavior: both updated ❌)

-- Test 2: FK cascade + delete (expect failure or no-op)
BEGIN;
UPDATE tb_post SET title = 'New' WHERE pk_post = 1;
DELETE FROM tb_user WHERE pk_user = 1;  -- Cascades to tb_post
COMMIT;
-- Check: Transaction aborts OR refresh skipped with warning

-- Test 3: Large queue (10K entries)
BEGIN;
UPDATE tb_user SET name = name || '_updated' WHERE pk_user <= 10000;
COMMIT;
-- Check: All 10K rows refreshed, memory usage < 10MB

-- Test 4: Deep propagation (10 levels)
-- Setup: tv_1 → tv_2 → ... → tv_10
UPDATE tb_entity_1 SET data = 'X' WHERE pk = 1;
COMMIT;
-- Check: All 10 levels refreshed in correct order

-- Test 5: Propagation depth exceeded (>100 levels)
-- Setup: Artificially create 101-level chain
UPDATE tb_entity_1 SET data = 'X' WHERE pk = 1;
COMMIT;
-- Check: Transaction aborts with PropagationDepthExceeded error
```

---

**End of Known Limitations Document**
