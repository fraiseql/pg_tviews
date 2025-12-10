# Phase 6A: Foundation

**Status:** READY TO START
**Prerequisites:** Phase 5 Task 7 Complete ✅
**Estimated Time:** 2-3 days
**TDD Phase:** RED → GREEN → REFACTOR

---

## Objective

Implement the foundational data structures and infrastructure for the transaction-level queue architecture:

1. `RefreshKey` type for identifying unique `(entity, pk)` pairs
2. Thread-local transaction queue using `RefCell<HashSet<RefreshKey>>`
3. Transaction callback registration (xact hooks)
4. Basic enqueue/dequeue functions

---

## Context

The current implementation (Phases 1-5) has no transaction-local state. Triggers immediately call `pg_tviews_cascade()`, which refreshes TVIEWs synchronously.

Phase 6A establishes the infrastructure for **deferred, coalesced refresh** by introducing:
- A per-transaction queue (thread-local state)
- Transaction lifecycle hooks (commit/abort callbacks)
- Type-safe queue operations

---

## Files to Create

### 1. `src/queue/mod.rs` (NEW)

Main queue module with sub-modules:

```rust
//! Transaction-level refresh queue for coalesced TVIEW updates
//!
//! This module implements the transaction queue architecture from PRD_multiupdate.md:
//! - RefreshKey: Identifies unique (entity, pk) pairs
//! - TX_REFRESH_QUEUE: Thread-local HashSet for deduplication
//! - Enqueue/dequeue operations
//! - Transaction callback registration

mod key;
mod state;
mod ops;

pub use key::RefreshKey;
pub use state::{TX_REFRESH_QUEUE, TX_REFRESH_SCHEDULED};
pub use ops::{enqueue_refresh, take_queue_snapshot, clear_queue, register_commit_callback_once};
```

### 2. `src/queue/key.rs` (NEW)

The `RefreshKey` type:

```rust
use std::hash::{Hash, Hasher};

/// Identifies a unique TVIEW row to refresh: (entity, pk)
///
/// Example: RefreshKey { entity: "user".to_string(), pk: 42 }
/// represents the row in tv_user with pk_user = 42
#[derive(Debug, Clone, Eq)]
pub struct RefreshKey {
    /// Entity name (e.g., "user", "post", "company")
    pub entity: String,

    /// Primary key value (pk_<entity>)
    pub pk: i64,
}

impl PartialEq for RefreshKey {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity && self.pk == other.pk
    }
}

impl Hash for RefreshKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity.hash(state);
        self.pk.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_refresh_key_equality() {
        let key1 = RefreshKey { entity: "user".to_string(), pk: 42 };
        let key2 = RefreshKey { entity: "user".to_string(), pk: 42 };
        let key3 = RefreshKey { entity: "user".to_string(), pk: 43 };

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_refresh_key_hashset_dedup() {
        let mut set = HashSet::new();

        set.insert(RefreshKey { entity: "user".to_string(), pk: 42 });
        set.insert(RefreshKey { entity: "user".to_string(), pk: 42 }); // duplicate
        set.insert(RefreshKey { entity: "post".to_string(), pk: 42 });

        assert_eq!(set.len(), 2); // Only 2 unique keys
    }
}
```

### 3. `src/queue/state.rs` (NEW)

Thread-local state for the transaction queue:

```rust
use std::cell::RefCell;
use std::collections::HashSet;
use super::key::RefreshKey;

thread_local! {
    /// Transaction-local queue of refresh requests
    ///
    /// - Populated by triggers on tb_* tables
    /// - Deduplicated automatically (HashSet)
    /// - Flushed at commit time by tx_commit_handler()
    /// - Cleared on transaction abort
    pub static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = RefCell::new(HashSet::new());

    /// Flag: has commit callback been registered for this transaction?
    ///
    /// - Set to true when first refresh is enqueued
    /// - Prevents multiple callback registrations
    /// - Reset to false after commit/abort
    pub static TX_REFRESH_SCHEDULED: RefCell<bool> = RefCell::new(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_thread_local() {
        // Each thread gets its own queue
        TX_REFRESH_QUEUE.with(|q| {
            let mut queue = q.borrow_mut();
            queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });
            assert_eq!(queue.len(), 1);
        });

        // Clear for next test
        TX_REFRESH_QUEUE.with(|q| q.borrow_mut().clear());
    }
}
```

### 4. `src/queue/ops.rs` (NEW)

Queue operations:

```rust
use std::collections::HashSet;
use super::key::RefreshKey;
use super::state::{TX_REFRESH_QUEUE, TX_REFRESH_SCHEDULED};
use crate::TViewResult;

/// Enqueue a refresh request for the given entity and pk
///
/// This is the main entry point from triggers.
/// Deduplica tion is automatic (HashSet).
pub fn enqueue_refresh(entity: &str, pk: i64) -> TViewResult<()> {
    let key = RefreshKey {
        entity: entity.to_string(),
        pk,
    };

    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        queue.insert(key);
    });

    Ok(())
}

/// Take a snapshot of the current queue and clear it
///
/// Called by commit handler to get all pending refreshes.
/// Thread-local state is cleared after snapshot.
pub fn take_queue_snapshot() -> HashSet<RefreshKey> {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        std::mem::take(&mut *queue)
    })
}

/// Clear the queue (used on transaction abort)
pub fn clear_queue() {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().clear();
    });
}

/// Register transaction commit callback (once per transaction)
///
/// This will be implemented in Phase 6C when we have the actual callback handler.
/// For now, this is a placeholder that sets the scheduled flag.
pub fn register_commit_callback_once() -> TViewResult<()> {
    TX_REFRESH_SCHEDULED.with(|flag| {
        let mut scheduled = flag.borrow_mut();
        if *scheduled {
            // Already registered, skip
            return Ok(());
        }

        // TODO Phase 6C: Register actual xact callback here
        // For now, just set the flag
        *scheduled = true;
        Ok(())
    })
}

/// Reset the scheduled flag (called after commit/abort)
pub fn reset_scheduled_flag() {
    TX_REFRESH_SCHEDULED.with(|flag| {
        *flag.borrow_mut() = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_snapshot() {
        clear_queue();

        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 2).unwrap();
        enqueue_refresh("user", 1).unwrap(); // duplicate

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2); // Deduplicated

        // Queue should be empty after snapshot
        let empty_snapshot = take_queue_snapshot();
        assert_eq!(empty_snapshot.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        clear_queue();

        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 2).unwrap();

        clear_queue();

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 0);
    }
}
```

---

## Files to Modify

### 1. `src/lib.rs`

Add queue module:

```rust
mod catalog;
mod refresh;
mod propagate;
mod utils;
mod hooks;
mod trigger;
mod queue;  // NEW

pub use error::{TViewError, TViewResult};
pub use queue::RefreshKey;  // Export for use in other modules
```

### 2. `Cargo.toml`

No changes needed (std::collections::HashSet is in std).

---

## Implementation Steps

### Step 1: Create Module Structure (RED)

1. Create `src/queue/` directory
2. Create `src/queue/mod.rs` with module declarations
3. Create empty `src/queue/key.rs`, `src/queue/state.rs`, `src/queue/ops.rs`
4. Add `mod queue;` to `src/lib.rs`
5. Verify compilation fails (modules are empty)

### Step 2: Implement RefreshKey (GREEN)

1. Write tests in `src/queue/key.rs` (RED)
   - `test_refresh_key_equality()`
   - `test_refresh_key_hashset_dedup()`
2. Implement `RefreshKey` struct with `PartialEq`, `Eq`, `Hash`
3. Run tests: `cargo test --lib` (GREEN)

### Step 3: Implement Thread-Local State (GREEN)

1. Write tests in `src/queue/state.rs` (RED)
   - `test_queue_thread_local()`
2. Implement `TX_REFRESH_QUEUE` and `TX_REFRESH_SCHEDULED`
3. Run tests: `cargo test --lib` (GREEN)

### Step 4: Implement Queue Operations (GREEN)

1. Write tests in `src/queue/ops.rs` (RED)
   - `test_enqueue_and_snapshot()`
   - `test_clear_queue()`
2. Implement functions:
   - `enqueue_refresh()`
   - `take_queue_snapshot()`
   - `clear_queue()`
   - `register_commit_callback_once()` (placeholder)
   - `reset_scheduled_flag()`
3. Run tests: `cargo test --lib` (GREEN)

### Step 5: Integration Test (GREEN)

Create `src/queue/integration_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{enqueue_refresh, take_queue_snapshot, clear_queue};

    #[test]
    fn test_multi_entity_queue() {
        clear_queue();

        // Simulate multiple trigger firings
        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 10).unwrap();
        enqueue_refresh("user", 1).unwrap(); // duplicate
        enqueue_refresh("post", 20).unwrap();
        enqueue_refresh("user", 2).unwrap();

        let snapshot = take_queue_snapshot();

        // Should have 4 unique keys: (user,1), (post,10), (post,20), (user,2)
        assert_eq!(snapshot.len(), 4);

        // Verify specific keys exist
        assert!(snapshot.contains(&RefreshKey { entity: "user".to_string(), pk: 1 }));
        assert!(snapshot.contains(&RefreshKey { entity: "post".to_string(), pk: 10 }));
    }
}
```

### Step 6: Export Public API

Update `src/queue/mod.rs` to export:

```rust
pub use key::RefreshKey;
pub use ops::{enqueue_refresh, take_queue_snapshot, clear_queue};
// Internal use only (not exported):
// - TX_REFRESH_QUEUE
// - TX_REFRESH_SCHEDULED
// - register_commit_callback_once
// - reset_scheduled_flag
```

Update `src/lib.rs` to re-export:

```rust
pub use queue::RefreshKey;
```

---

## Verification Commands

### Compilation Check
```bash
cargo clippy --release -- -D warnings
```

### Unit Tests
```bash
cargo test --lib queue::
```

### Integration Tests
```bash
cargo test --lib
```

### Expected Output
```
test queue::key::tests::test_refresh_key_equality ... ok
test queue::key::tests::test_refresh_key_hashset_dedup ... ok
test queue::state::tests::test_queue_thread_local ... ok
test queue::ops::tests::test_enqueue_and_snapshot ... ok
test queue::ops::tests::test_clear_queue ... ok
test queue::integration_tests::test_multi_entity_queue ... ok

test result: ok. 6 passed; 0 failed
```

---

## Acceptance Criteria

- ✅ `RefreshKey` type compiles with `Hash`, `PartialEq`, `Eq`
- ✅ Thread-local queue infrastructure works
- ✅ `enqueue_refresh()` deduplicates correctly
- ✅ `take_queue_snapshot()` clears queue after snapshot
- ✅ All unit tests pass
- ✅ Clippy strict compliance (0 warnings)
- ✅ Code documented with clear comments

---

## DO NOT

- ❌ Implement transaction callbacks (that's Phase 6C)
- ❌ Integrate with triggers yet (that's Phase 6B)
- ❌ Implement commit handler logic (that's Phase 6C)
- ❌ Touch existing refresh functions (wait for Phase 6C integration)

---

## Notes

- **Thread-local state**: Each PostgreSQL backend process has its own thread-local storage. This is safe because PostgreSQL uses a process-per-connection model.
- **HashSet deduplication**: Automatic - no need for manual checks.
- **Phase 6C preview**: The actual transaction callback registration will use pgrx hooks (e.g., `RegisterXactCallback` FFI bindings).

---

## Next Phase

After Phase 6A is complete and tested:
**Read**: `.phases/phase-6b-trigger-refactor.md`
