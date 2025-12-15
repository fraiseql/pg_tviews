# Phase 1: Implement Savepoint Depth Tracking

## Objective

Replace the stub `get_savepoint_depth()` function with a proper implementation using PostgreSQL's `GetCurrentTransactionNestLevel()` API. This enables correct savepoint/rollback behavior for the transaction queue.

## Context

Currently, `src/queue/persistence.rs:144-148` contains:

```rust
/// Get current savepoint depth
const fn get_savepoint_depth() -> usize {
    // This would need to be implemented to track savepoint depth
    // For now, return 0 as we don't have access to this information
    0
}
```

This is problematic because:
1. The `SerializedQueue` stores `savepoint_depth` for rollback tracking
2. Returning constant 0 means all queues appear to be at top-level transaction
3. Savepoint rollback cannot correctly identify which queue snapshot to restore

## Files to Modify

| File | Changes |
|------|---------|
| `src/queue/persistence.rs` | Implement `get_savepoint_depth()` using `pg_sys::GetCurrentTransactionNestLevel()` |

## Implementation Steps

### Step 1: Update the function signature

Remove `const fn` since we need to call FFI:

```rust
// Before
const fn get_savepoint_depth() -> usize {
    0
}

// After
fn get_savepoint_depth() -> usize {
    // Implementation here
}
```

### Step 2: Implement using PostgreSQL API

```rust
/// Get current savepoint depth using PostgreSQL's transaction nesting level
///
/// PostgreSQL's `GetCurrentTransactionNestLevel()` returns:
/// - 1 for top-level transaction (no savepoints)
/// - 2 for first savepoint level
/// - 3 for nested savepoint, etc.
///
/// We convert this to savepoint depth:
/// - 0 = no savepoints (nest level 1)
/// - 1 = one savepoint active (nest level 2)
/// - etc.
fn get_savepoint_depth() -> usize {
    // Safety: GetCurrentTransactionNestLevel is safe to call from any transaction context
    let nest_level = unsafe { pgrx::pg_sys::GetCurrentTransactionNestLevel() };

    // Convert nest level to savepoint depth
    // nest_level is always >= 1 when in a transaction
    if nest_level > 1 {
        (nest_level - 1) as usize
    } else {
        0
    }
}
```

### Step 3: Add import if needed

Ensure `pgrx::pg_sys` is available in the module. Check existing imports at top of file.

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run unit tests
cargo test --no-default-features --features pg18 -- persistence

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings
```

## Expected Test Behavior

After implementation, the following scenario should work correctly:

```sql
BEGIN;
INSERT INTO tb_user VALUES (1, 'Alice');  -- Queue: [(user, 1)]
SAVEPOINT sp1;
INSERT INTO tb_user VALUES (2, 'Bob');    -- Queue: [(user, 1), (user, 2)]
ROLLBACK TO sp1;                          -- Queue should restore to: [(user, 1)]
COMMIT;                                   -- Only user 1 should be refreshed
```

## Acceptance Criteria

- [ ] `get_savepoint_depth()` returns correct nesting level
- [ ] Function is no longer `const fn`
- [ ] Code compiles without warnings
- [ ] Clippy passes
- [ ] Existing tests still pass

## DO NOT

- Do not change the `SerializedQueue` struct
- Do not modify the savepoint callback logic in `xact.rs`
- Do not add new dependencies
