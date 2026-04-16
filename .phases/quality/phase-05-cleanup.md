# Phase 5: Code Quality & Cleanup

## Objective

Resolve the remaining LOW-severity findings: verify the pgrx 0.17.0 workaround; add
a queue-size GUC; compile regex patterns once; resolve all dead code (wire it or
delete it); and optionally harden the missing-row case in refresh_pk.

## Success Criteria

- [ ] `catalog.rs:414` workaround either simplified (if bug fixed in 0.17.0) or annotated
  with a pgrx issue reference and a regression test
- [ ] `pg_tviews.max_queue_size` GUC exists; `enqueue_refresh` enforces it
- [ ] `GID_RE_SINGLE` and `GID_RE_DOUBLE` are `LazyLock<Regex>` statics
- [ ] Zero `#[allow(dead_code)]` attributes in the codebase (either wired or deleted)
- [ ] `log_refresh` is called from at least one refresh path (or deleted)
- [ ] `queue/persistence.rs` is either fully wired into 2PC flow or removed entirely
- [ ] `refresh/cache.rs` is either wired or removed
- [ ] `refresh_pk` treats missing-row as a DELETE signal (returns `Ok(None)`) rather than
  `Err(...)`
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` produces zero warnings
- [ ] `git grep -i "allow(dead_code)"` returns empty
- [ ] All tests pass

## TDD Cycles

### Cycle 1: F-08 — pgrx 0.17.0 workaround verification (catalog.rs:414-430)

**Context:** The workaround comment says pgrx 0.16.1's `get_one_with_args` errors on
zero-row results. The project now uses pgrx 0.17.0. Need to verify whether the bug is
fixed.

- **RED**: Write a unit test that calls `entity_for_table_uncached` for an OID that does
  not exist in `pg_tview_meta`. If the workaround is still needed, this test documents it.
  If it's fixed, the test proves we can simplify.
- **GREEN**:
  - Test `Spi::get_one_with_args` with a zero-row-returning query in pgrx 0.17.0.
  - **If fixed**: Replace the `Spi::connect + client.select` workaround with a single
    `Spi::get_one_with_args` call. Remove the workaround comment.
  - **If not fixed**: Update the comment to say: "pgrx 0.17.0 still has this bug; see
    [pgrx issue #NNNN]. Remove this workaround when the issue is resolved." File the
    pgrx issue if it doesn't exist.
- **REFACTOR**: No structural change.
- **CLEANUP**: Clippy clean.

### Cycle 2: F-09 — unbounded refresh queue (queue/state.rs:12, queue/ops.rs:9-37)

**ROOT CAUSE:**
`TX_REFRESH_QUEUE` is an unsized `HashSet`. Bulk DML fires the row-level trigger once
per row. With N unique PKs, the queue grows to N entries. No backpressure exists. Any
user with INSERT/UPDATE/DELETE on a TVIEW-backed table can exhaust backend memory.

- **RED**: Write a test: call `enqueue_refresh` beyond the GUC limit and assert it raises
  a clear error.
- **GREEN**:
  1. Add a GUC in `src/config/mod.rs`:
     ```rust
     // pg_tviews.max_queue_size (default 100_000, min 1, max 10_000_000)
     ```
  2. In `enqueue_refresh` (queue/ops.rs), after inserting into the queue:
     ```rust
     let max = get_max_queue_size(); // reads GUC
     if queue.len() > max as usize {
         error!(
             "pg_tviews: refresh queue exceeded max_queue_size = {}. \
              Consider batching your DML or raising pg_tviews.max_queue_size.",
             max
         );
     }
     ```
- **REFACTOR**: No structural change needed.
- **CLEANUP**: Document the GUC in README; clippy clean.

### Cycle 3: F-10 — regex compiled on every PREPARE TRANSACTION (hooks.rs:497-508)

**ROOT CAUSE:**
Two `Regex::new` calls execute on every `PREPARE TRANSACTION` statement inside the
ProcessUtility hook. Regex compilation allocates even for simple patterns.

- **RED**: Code review: grep for `Regex::new` in hooks.rs and assert it is not called
  inside any function body (i.e., only at static initialisation).
- **GREEN**:
  ```rust
  static GID_RE_SINGLE: LazyLock<Regex> = LazyLock::new(||
      Regex::new(r"PREPARE\s+TRANSACTION\s+'([^']+)'").unwrap()
  );
  static GID_RE_DOUBLE: LazyLock<Regex> = LazyLock::new(||
      Regex::new(r#"PREPARE\s+TRANSACTION\s+"([^"]+)""#).unwrap()
  );
  ```
  Replace the inline `Regex::new` calls with references to these statics.
- **REFACTOR**: No structural change.
- **CLEANUP**: Remove `regex` dependency from Cargo.toml if no longer needed elsewhere;
  else confirm it is still used.

### Cycle 4: F-12a — wire log_refresh (audit.rs:46)

**Context:** `audit.rs::log_refresh` is never called. Low cost to wire, high value for
auditability.

- **RED**: `cargo clippy` with `#[deny(dead_code)]` added globally — confirm `log_refresh`
  is flagged as unused.
- **GREEN**: Call `log_refresh` from `refresh_pk` and `refresh_by_dedup_key` after each
  successful refresh. Remove `#[allow(dead_code)]` from `log_refresh`.
- **REFACTOR**: No structural change needed.
- **CLEANUP**: Clippy clean.

### Cycle 5: F-12b — delete dead 2PC and cache infrastructure (queue/xact.rs:349,391, queue/persistence.rs, refresh/cache.rs) + P-12

**Context:** Seven items in `queue/persistence.rs` and `handle_prepare` are
`#[allow(dead_code)]`. `refresh/cache.rs` is entirely dead (and its
`get_or_prepare_statement` validates each cache hit with a `pg_prepared_statements` query,
defeating its own fast path — P-12). Each represents abandoned infrastructure.

**Decision rule:**
- 2PC is deferred (F-05 now emits error) → delete 2PC infrastructure.
- `refresh/cache.rs` is unwired and P-12 shows it's broken by design → delete it.
  The performance gains from P-06/P-07 make it unnecessary.

- **RED**: `cargo clippy` with `#[deny(dead_code)]` — list all items that fail after
  Cycle 4 wiring.
- **GREEN**:
  - Delete `handle_prepare`, delete `persistence.rs`, delete the GID-capture in the
    hook if it was only used by handle_prepare. Add `// 2PC support removed; see
    https://github.com/… for roadmap` comment in xact.rs.
  - Delete `refresh/cache.rs` entirely (resolves both F-12 and P-12).
- **REFACTOR**: Re-run `cargo clippy` to confirm zero dead_code warnings.
- **CLEANUP**: Remove all remaining `#[allow(dead_code)]`; add `#[deny(dead_code)]` at
  crate root (or rely on clippy::pedantic which includes it). Clippy must be clean.

### Cycle 6: F-13 — harden refresh_pk missing-row handling (refresh/main.rs:267-272)

**Context:** `recompute_view_row` returns `Err("No row in v_* for given pk: {pk}")` when
the backing view returns no rows. This propagates to the COMMIT hook's `error!()`,
aborting the user's transaction. The savepoint mechanism likely removes such keys before
flush, but a TRUNCATE inside a committed savepoint could leave a stale key.

- **RED**: Write a pgtest: INSERT a row, enqueue it, then TRUNCATE the base table (so
  the view row disappears), then COMMIT. Assert the commit succeeds (TVIEW row deleted)
  rather than raising an error.
- **GREEN**: In `recompute_view_row`, when the backing view returns no rows, return
  `Ok(None)` instead of `Err(...)`. In `refresh_and_get_parents`, treat `Ok(None)` as a
  DELETE signal: issue `DELETE FROM tv_{entity} WHERE pk_{entity} = $1`.
- **REFACTOR**: Ensure the delete path also fires `log_refresh` (from Cycle 4) if wired.
- **CLEANUP**: Remove the `Err("No row in v_* for given pk…")` path; clippy clean.

## Dependencies

- Requires: Phase 4 complete
- Blocks: Phase 6 (Finalize)

## Status

[ ] Not Started
