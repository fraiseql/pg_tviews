# Phase 3: Queue / 2PC Deduplication

## Objective

1. Eliminate the duplicated propagation loop between `twophase.rs::process_refresh_queue`
   and `queue/xact.rs::flush_refresh_queue`.
2. Delete the two dead functions `handle_prepare` and `get_prepared_transaction_id`
   from `queue/xact.rs` that were superseded when 2PC logic moved to `twophase.rs`.

## Background

### The duplication

`flush_refresh_queue` (xact.rs:205) and `process_refresh_queue` (twophase.rs:101)
both implement the same algorithm:

```
while pending not empty:
    sort by dependency order
    for each key:
        skip if already processed
        refresh key → get parents
        enqueue undiscovered parents
    guard against infinite propagation
```

The differences are minor:
- `flush_refresh_queue` calls `take_queue_snapshot()` to get its initial set;
  `process_refresh_queue` accepts the set as a parameter.
- `flush_refresh_queue` groups keys by entity and uses a bulk refresh path for
  multi-key batches; `process_refresh_queue` always uses the single-key path.
- `flush_refresh_queue` records metrics; `process_refresh_queue` does not.

The bulk-refresh path in `flush_refresh_queue` is a performance optimisation
that should apply to 2PC refreshes too (not just trigger-driven refreshes).
The correct fix is to make one authoritative implementation, not to maintain two.

### The dead code

`handle_prepare` (xact.rs:339) and `get_prepared_transaction_id` (xact.rs:381)
are marked `#[allow(dead_code)]` and are never called. The PREPARE TRANSACTION
handling was moved to the ProcessUtility hook and `twophase.rs`. These functions
are unreachable.

## Success Criteria

- [ ] `process_refresh_queue` and its private `refresh_and_get_parents` in `twophase.rs` are deleted
- [ ] `flush_refresh_queue` accepts an optional pre-loaded queue (`Option<HashSet<RefreshKey>>`)
      or a small enum variant that lets the 2PC caller pass in a pre-serialized queue
- [ ] `twophase.rs::pg_tviews_commit_prepared` calls the unified function from `queue::xact`
- [ ] `handle_prepare` and `get_prepared_transaction_id` are deleted from `xact.rs`
- [ ] 2PC path uses bulk-refresh optimisation (same as trigger path)
- [ ] All tests pass; clippy clean

## TDD Cycles

### Cycle 1: Remove dead code (RED → GREEN)

**RED**

Add a compile-time check that documents the intent: verify neither dead
function is reachable from any public path. A `#[cfg(test)]` assertion is
overkill — just confirm `cargo clippy` currently emits no dead-code warning
(the `#[allow]` suppresses it). The "test" here is removing the `#[allow]`
and confirming clippy fires, then deleting the functions.

```bash
# Should produce dead_code warnings for both functions after removing #[allow]:
cargo clippy --no-default-features --features pg18 2>&1 | grep "handle_prepare\|get_prepared_transaction_id"
```

**GREEN**

Delete `handle_prepare` (xact.rs:339–378) and `get_prepared_transaction_id`
(xact.rs:381–382) entirely. These are private functions with no callers.

Verify the build still compiles and all tests pass.

**CLEANUP**

- Run `cargo clippy --no-default-features --features pg18 -- -D warnings`
- Commit: `refactor(queue): delete dead 2PC helpers superseded by twophase.rs`

---

### Cycle 2: Unify the propagation loop (RED → GREEN → REFACTOR)

**RED**

Write an integration test that calls `pg_tviews_commit_prepared` on a fake GID
with a non-empty serialized queue and asserts that the correct TVIEWs are
refreshed. This is the existing behaviour; the test makes it explicit so any
regression is caught.

(If the test environment doesn't support 2PC, a unit test that calls
`process_refresh_queue` directly with a constructed HashSet also suffices.)

**GREEN**

Refactor `flush_refresh_queue` to accept the initial queue as a parameter
instead of always calling `take_queue_snapshot()`:

```rust
/// Flush the refresh queue.
///
/// `initial` — if `Some`, use this pre-loaded queue instead of the
///             transaction-local snapshot (used by 2PC commit path).
///             If `None`, calls `take_queue_snapshot()`.
pub fn flush_refresh_queue_with(
    initial: Option<std::collections::HashSet<super::key::RefreshKey>>,
) -> TViewResult<()> {
    let mut pending = initial.unwrap_or_else(take_queue_snapshot);
    // ... existing body unchanged ...
}

/// Flush the transaction-local refresh queue (trigger-driven path).
pub fn flush_refresh_queue() -> TViewResult<()> {
    flush_refresh_queue_with(None)
}
```

Update `twophase.rs::pg_tviews_commit_prepared` to replace its
`process_refresh_queue(queue)` call:

```rust
// Before:
match process_refresh_queue(queue) { ... }

// After:
match crate::queue::xact::flush_refresh_queue_with(Some(queue)) { ... }
```

Delete `process_refresh_queue` **and** `refresh_and_get_parents` from
`twophase.rs`. The latter (lines 137–151) is a near-duplicate of the
same-named function in `queue/xact.rs` (lines 313–332) and is only
called by `process_refresh_queue`.

**REFACTOR**

The 2PC path now benefits from the entity-grouped bulk-refresh optimisation
automatically. Verify the metrics recording path does not double-count
(metrics should fire regardless of the `initial` source — check
`record_refresh_start` / `record_refresh_complete` are still called).

**CLEANUP**

- Ensure `process_refresh_queue` and `refresh_and_get_parents` are gone from `twophase.rs`
- `flush_refresh_queue_with` should remain `pub` only within `queue` crate
  boundary — check visibility
- Run `cargo clippy` and `cargo test`
- Commit: `refactor(queue): unify 2PC and trigger-driven refresh queue processing`

## Files Touched

- `src/queue/xact.rs` — add `flush_refresh_queue_with`, delete dead helpers
- `src/twophase.rs` — replace `process_refresh_queue` call, delete function

## Risk

Medium. This touches the transaction-critical refresh path. Key guards:

1. The 2PC path runs in an explicit `BEGIN` / `COMMIT` block in
   `pg_tviews_commit_prepared` — errors are already caught and rolled back.
2. `flush_refresh_queue_with(Some(queue))` short-circuits immediately on empty
   queue, same as before.
3. The only behavioural change is that the 2PC path now uses bulk refresh for
   multi-key same-entity batches — strictly an improvement.

Do not combine this phase with any other phase commit.
