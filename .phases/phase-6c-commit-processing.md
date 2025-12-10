# Phase 6C: Commit-Time Processing

**Status:** BLOCKED (requires Phase 6A, 6B complete)
**Prerequisites:** Phase 6A Foundation ✅, Phase 6B Trigger Refactor ✅
**Estimated Time:** 3-4 days
**TDD Phase:** RED → GREEN → REFACTOR → QA

---

## Objective

Implement the commit-time refresh logic that flushes the transaction queue:

1. Register PostgreSQL transaction callbacks (xact hooks)
2. Implement `tx_commit_handler()` to flush the queue
3. Integrate with existing `refresh::refresh_pk()` functions
4. Handle transaction abort (clear queue)
5. Process refreshes in dependency-correct order

---

## Context

### Current State (After Phase 6B)

Triggers enqueue refreshes, but nothing flushes the queue:

```rust
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Trigger fires → enqueue(("user", 1)) → TX_REFRESH_QUEUE has 1 item
COMMIT;
-- ❌ Queue is NOT flushed (no handler registered)
-- TVIEWs remain stale
```

### Target State (Phase 6C)

Transaction callbacks flush the queue at commit time:

```rust
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
UPDATE tb_user SET email = 'alice@example.com' WHERE pk_user = 1;
-- Both triggers fire → enqueue(("user", 1)) twice
-- Queue: {("user", 1)} (deduplicated)

COMMIT;
-- ✅ tx_commit_handler() fires BEFORE commit completes
--    1. Snapshot queue: {("user", 1)}
--    2. Refresh tv_user row 1
--    3. Propagate to parents (tv_post, tv_feed, etc.)
--    4. Clear queue
-- TVIEWs are now fresh
```

---

## PostgreSQL Transaction Callbacks

PostgreSQL provides C API hooks for transaction lifecycle events:

```c
// pg_sys FFI (PostgreSQL 17)
typedef enum {
    XACT_EVENT_COMMIT,
    XACT_EVENT_ABORT,
    XACT_EVENT_PREPARE,
    XACT_EVENT_PRE_COMMIT,
    XACT_EVENT_PARALLEL_COMMIT,
    XACT_EVENT_PARALLEL_ABORT,
    XACT_EVENT_PARALLEL_PRE_COMMIT
} XactEvent;

typedef void (*XactCallback)(XactEvent event, void *arg);

void RegisterXactCallback(XactCallback callback, void *arg);
void UnregisterXactCallback(XactCallback callback, void *arg);
```

**pgrx Access**: Currently, pgrx 0.12.8 does NOT have high-level wrappers for `RegisterXactCallback`. We need to use FFI directly.

---

## Files to Create

### 1. `src/queue/xact.rs` (NEW)

Transaction callback registration and handlers:

```rust
use pgrx::prelude::*;
use pgrx::pg_sys;
use std::os::raw::c_void;
use super::ops::{take_queue_snapshot, clear_queue, reset_scheduled_flag};
use crate::TViewResult;

/// Transaction event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XactEvent {
    Commit,
    Abort,
    PreCommit,
}

/// Register the transaction callback (called from enqueue logic)
///
/// This uses PostgreSQL's RegisterXactCallback FFI to install our handler.
/// The callback will be invoked at transaction commit/abort.
pub unsafe fn register_xact_callback() -> TViewResult<()> {
    // Safety: We're calling into PostgreSQL FFI
    // The callback function must be extern "C" and #[no_mangle]

    unsafe {
        pg_sys::RegisterXactCallback(
            Some(tview_xact_callback),
            std::ptr::null_mut(),
        );
    }

    Ok(())
}

/// Transaction callback handler (invoked by PostgreSQL)
///
/// This is called at transaction events (COMMIT, ABORT, etc.)
///
/// # Safety
/// This is an extern "C" callback invoked by PostgreSQL internals.
/// Must not panic or unwind.
#[no_mangle]
unsafe extern "C" fn tview_xact_callback(event: pg_sys::XactEvent, _arg: *mut c_void) {
    // Determine event type
    let xact_event = match event {
        pg_sys::XactEvent_XACT_EVENT_COMMIT => XactEvent::Commit,
        pg_sys::XactEvent_XACT_EVENT_ABORT => XactEvent::Abort,
        pg_sys::XactEvent_XACT_EVENT_PRE_COMMIT => XactEvent::PreCommit,
        _ => return, // Ignore other events
    };

    // Handle event
    match xact_event {
        XactEvent::PreCommit => {
            // PRE_COMMIT: Flush queue before transaction commits
            // This is the main refresh point
            if let Err(e) = handle_pre_commit() {
                warning!("TVIEW refresh failed during PRE_COMMIT: {:?}", e);
                // PostgreSQL will abort the transaction if we return error here
                // For now, log warning and let transaction proceed
            }
        }
        XactEvent::Abort => {
            // ABORT: Clear queue without refreshing
            clear_queue();
            reset_scheduled_flag();
        }
        XactEvent::Commit => {
            // COMMIT: Cleanup (queue already flushed in PRE_COMMIT)
            reset_scheduled_flag();
        }
    }
}

/// Handle PRE_COMMIT event: flush the queue and refresh TVIEWs
fn handle_pre_commit() -> TViewResult<()> {
    // Take snapshot of pending refreshes
    let queue = take_queue_snapshot();

    if queue.is_empty() {
        return Ok(());
    }

    info!("TVIEW: Flushing {} refresh requests at commit", queue.len());

    // Process each refresh
    // TODO Phase 6D: Sort by dependency order
    for key in queue {
        if let Err(e) = refresh_entity_pk(&key) {
            error!("Failed to refresh {}[{}]: {:?}", key.entity, key.pk, e);
            // Continue with other refreshes (don't fail entire transaction)
        }
    }

    Ok(())
}

/// Refresh a single entity+pk (delegates to existing refresh logic)
fn refresh_entity_pk(key: &super::key::RefreshKey) -> TViewResult<()> {
    // Map entity → view OID
    let entity = &key.entity;
    let pk = key.pk;

    // Strategy: Reuse existing refresh::refresh_pk() function
    // But we need the view OID, not just entity name

    // Load metadata
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(entity)?
        .ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: entity.clone(),
        })?;

    // Call existing refresh function
    crate::refresh::refresh_pk(meta.view_oid, pk)?;

    Ok(())
}
```

### 2. `src/queue/ops.rs` (MODIFY)

Update `register_commit_callback_once()` to actually register:

```rust
/// Register transaction commit callback (once per transaction)
pub fn register_commit_callback_once() -> TViewResult<()> {
    TX_REFRESH_SCHEDULED.with(|flag| {
        let mut scheduled = flag.borrow_mut();
        if *scheduled {
            // Already registered, skip
            return Ok(());
        }

        // Register xact callback
        unsafe {
            super::xact::register_xact_callback()?;
        }

        *scheduled = true;
        Ok(())
    })
}
```

---

## Files to Modify

### 1. `src/queue/mod.rs`

Add xact module:

```rust
mod key;
mod state;
mod ops;
mod xact;  // NEW

pub use key::RefreshKey;
pub use ops::{enqueue_refresh, take_queue_snapshot, clear_queue, register_commit_callback_once};
// xact module is internal (not exported)
```

### 2. `src/refresh/main.rs` (or wherever `refresh_pk` lives)

Ensure `refresh_pk()` is public and can be called from queue module:

```rust
/// Refresh a single TVIEW row by view OID and PK
///
/// This is the main entry point for both immediate refresh (legacy)
/// and deferred refresh (Phase 6 transaction queue).
pub fn refresh_pk(view_oid: pg_sys::Oid, pk: i64) -> TViewResult<()> {
    // Existing implementation...
}
```

---

## Implementation Steps

### Step 1: Implement Transaction Callback Registration (RED)

1. Create `src/queue/xact.rs` with stub functions
2. Add FFI declarations for `RegisterXactCallback`
3. Write test stub (will fail - requires database)
4. Verify compilation: `cargo clippy --release -- -D warnings`

### Step 2: Implement Callback Handler (GREEN)

1. Implement `tview_xact_callback()` extern "C" function
2. Implement `handle_pre_commit()` with queue flush logic
3. Implement `refresh_entity_pk()` to delegate to existing refresh functions
4. Test with manual SQL:
   ```sql
   BEGIN;
   UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
   UPDATE tb_user SET email = 'alice@example.com' WHERE pk_user = 1;
   COMMIT;
   -- Check tv_user row 1 is updated
   SELECT * FROM tv_user WHERE pk_user = 1;
   ```

### Step 3: Handle Transaction Abort (GREEN)

1. Implement abort handler in `tview_xact_callback()`
2. Test rollback:
   ```sql
   BEGIN;
   UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;
   ROLLBACK;
   -- Queue should be cleared, tv_user unchanged
   ```

### Step 4: Error Handling (REFACTOR)

1. Add comprehensive error handling
2. Log warnings for failed refreshes (don't abort transaction)
3. Test edge cases:
   - Metadata not found
   - View deleted mid-transaction
   - FK violations during refresh

### Step 5: Integration Testing (QA)

Create integration test suite:

```rust
// In src/queue/integration_tests.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_flushes_queue() {
        // Setup: Create test tables and TVIEW
        // Execute: Multi-update transaction
        // Verify: TVIEW refreshed once (not multiple times)
    }

    #[test]
    fn test_abort_clears_queue() {
        // Setup: Create test tables and TVIEW
        // Execute: Transaction with ROLLBACK
        // Verify: Queue cleared, TVIEW unchanged
    }

    #[test]
    fn test_coalescing() {
        // Setup: Create test tables and TVIEW
        // Execute: 10 updates to same row
        // Verify: Only 1 refresh executed
    }
}
```

---

## Verification Commands

### Compilation Check
```bash
cargo clippy --release -- -D warnings
```

### Unit Tests
```bash
cargo test --lib queue::xact
```

### Manual Integration Test

```sql
-- Setup
CREATE TABLE tb_user (pk_user INT PRIMARY KEY, name TEXT, email TEXT);
INSERT INTO tb_user VALUES (1, 'Alice', 'alice@old.com');

-- Create TVIEW (assuming pg_tviews_create exists)
SELECT pg_tviews_create('user',
  'SELECT pk_user, jsonb_build_object(''name'', name, ''email'', email) AS data FROM tb_user');

-- Test: Multi-update coalescing
BEGIN;
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;
UPDATE tb_user SET email = 'alice@new.com' WHERE pk_user = 1;
UPDATE tb_user SET name = 'Alice Final' WHERE pk_user = 1;
COMMIT;

-- Verify: tv_user should show final state
SELECT * FROM tv_user WHERE pk_user = 1;
-- Expected: {"name": "Alice Final", "email": "alice@new.com"}

-- Check logs for coalescing evidence
-- Expected log: "TVIEW: Flushing 1 refresh requests at commit"
-- (not 3 requests)
```

---

## Acceptance Criteria

- ✅ Transaction callback registered successfully
- ✅ PRE_COMMIT event flushes queue and refreshes TVIEWs
- ✅ ABORT event clears queue without refreshing
- ✅ Multiple updates to same row → single refresh (coalescing works)
- ✅ Refreshes use existing `refresh_pk()` logic (no duplication)
- ✅ Error handling doesn't abort transactions unnecessarily
- ✅ Clippy strict compliance (0 warnings)
- ✅ Manual integration test passes

---

## Known Limitations

### Limitation 1: No Dependency Ordering (Yet)

Phase 6C processes refreshes in **queue insertion order**, not dependency order.

**Example**:
```rust
UPDATE tb_company SET name = 'Acme Corp' WHERE pk_company = 1;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1; -- fk_company = 1
UPDATE tb_post SET title = 'Hello' WHERE pk_post = 1; -- fk_user = 1

// Queue at commit: [("company", 1), ("user", 1), ("post", 1)]
// Refresh order: company, then user, then post ✅ (happens to be correct)
```

**Problem**: If queue order is shuffled (e.g., HashMap iteration), dependency order might be violated.

**Solution**: Phase 6D will implement topological sorting.

### Limitation 2: Propagation Not Coalesced

If refreshing `tv_post[1]` discovers parent `tv_user[1]`, the parent is refreshed immediately (not enqueued).

**Solution**: Phase 6D will integrate propagation with the queue.

---

## DO NOT

- ❌ Implement dependency graph (that's Phase 6D)
- ❌ Optimize queue iteration order (Phase 6D)
- ❌ Remove `pg_tviews_cascade()` function (keep for backward compatibility testing)

---

## Rollback Strategy

If Phase 6C causes transaction failures:

1. Add feature flag `pg_tviews.enable_deferred_refresh`:
   ```sql
   SET pg_tviews.enable_deferred_refresh = off; -- Revert to immediate mode
   ```

2. Modify trigger to check flag:
   ```rust
   if config::is_deferred_refresh_enabled() {
       enqueue_refresh(&entity, pk)?;
   } else {
       pg_tviews_cascade(table_oid, pk); // Legacy immediate mode
   }
   ```

3. Investigate Phase 6C issues on feature branch

---

## Next Phase

After Phase 6C is complete and commit-time refresh works:
**Read**: `.phases/phase-6d-entity-graph.md`
