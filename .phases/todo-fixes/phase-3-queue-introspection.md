# Phase 3: Implement Queue Introspection

## Objective

Implement real-time queue introspection to replace the placeholder values in `pg_tviews_queue_realtime` view. This allows DBAs to monitor pending refresh operations during long transactions.

## Context

Currently, `src/metadata.rs:123` has a placeholder view:

```sql
CREATE OR REPLACE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    0 as queue_size,  -- TODO: Implement queue introspection
    ARRAY[]::TEXT[] as entities,
    NOW() as last_enqueued;
```

This returns dummy values. We need to expose the actual thread-local queue contents via a SQL-callable function.

## Challenge

The refresh queue is stored in thread-local storage (`TX_REFRESH_QUEUE`), which is only accessible from the same backend process. This is actually perfect for introspection - each backend sees its own queue.

## Files to Modify

| File | Changes |
|------|---------|
| `src/queue/ops.rs` | Add `get_queue_stats()` function |
| `src/lib.rs` | Add `pg_tviews_queue_info()` SQL function |
| `src/metadata.rs` | Update view to use the new function |

## Implementation Steps

### Step 1: Add queue statistics function in ops.rs

```rust
/// Queue statistics for introspection
#[derive(Debug, Clone)]
pub struct QueueStats {
    pub size: usize,
    pub entities: Vec<String>,
}

/// Get current queue statistics (for introspection)
///
/// Returns the number of pending refresh requests and list of unique entities.
/// Safe to call from SQL context.
pub fn get_queue_stats() -> QueueStats {
    TX_REFRESH_QUEUE.with(|q| {
        let queue = q.borrow();

        // Count unique entities
        let mut entity_set = std::collections::HashSet::new();
        for key in queue.iter() {
            entity_set.insert(key.entity.clone());
        }

        let entities: Vec<String> = entity_set.into_iter().collect();

        QueueStats {
            size: queue.len(),
            entities,
        }
    })
}
```

### Step 2: Add SQL-callable function in lib.rs

```rust
/// Get current transaction's refresh queue information
///
/// Returns a single row with queue statistics for the current backend.
/// This is useful for monitoring long-running transactions.
///
/// # Example
///
/// ```sql
/// SELECT * FROM pg_tviews_queue_info();
/// -- Returns: (queue_size, entities[])
/// ```
#[pg_extern]
fn pg_tviews_queue_info() -> TableIterator<'static, (
    name!(queue_size, i32),
    name!(entities, Vec<String>),
)> {
    let stats = crate::queue::ops::get_queue_stats();

    TableIterator::new(vec![(
        stats.size as i32,
        stats.entities,
    )])
}
```

### Step 3: Update the monitoring view in metadata.rs

Replace the placeholder SQL with:

```sql
CREATE OR REPLACE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    q.queue_size,
    q.entities,
    NOW() as snapshot_time
FROM pg_tviews_queue_info() q;
```

### Step 4: Export the ops module function

Ensure `get_queue_stats` is accessible from `src/lib.rs`. Check that the queue module exports it properly.

In `src/queue/mod.rs`:
```rust
pub mod ops;
// ... other modules ...

// Re-export for convenience
pub use ops::get_queue_stats;
```

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Test with pgrx
cargo pgrx test pg18
```

## SQL Verification

After implementation:

```sql
-- Start a transaction with some changes
BEGIN;

-- Make some changes that trigger queue entries
INSERT INTO tb_user VALUES (1, 'Alice');
INSERT INTO tb_user VALUES (2, 'Bob');
INSERT INTO tb_post VALUES (1, 1, 'Hello');

-- Check the queue (should show pending refreshes)
SELECT * FROM pg_tviews_queue_info();
-- Expected: (3, {user, post}) or similar

-- Check the view
SELECT * FROM pg_tviews_queue_realtime;
-- Should show session info plus queue stats

-- Commit (queue gets processed and cleared)
COMMIT;

-- Check again (should be empty)
SELECT * FROM pg_tviews_queue_info();
-- Expected: (0, {})
```

## Acceptance Criteria

- [ ] `pg_tviews_queue_info()` function exists and returns correct data
- [ ] `pg_tviews_queue_realtime` view shows real queue size
- [ ] Queue size updates as items are enqueued
- [ ] Queue shows empty after commit/rollback
- [ ] Code compiles without warnings
- [ ] Clippy passes

## DO NOT

- Do not expose internal RefreshKey structure directly to SQL
- Do not allow modification of the queue via SQL (read-only introspection)
- Do not break existing queue functionality
- Do not add significant overhead to enqueue operations
