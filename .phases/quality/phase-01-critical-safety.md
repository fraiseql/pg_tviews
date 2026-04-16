# Phase 1: Critical Safety

## Objective

Fix three bugs that silently corrupt state or produce wrong results:
HOOK_IN_PROGRESS is permanently leaked on longjmp; sort_keys destroys dedup-key identity;
SAVEPOINT_DEPTH can underflow to usize::MAX in release mode.

## Success Criteria

- [x] `CREATE TABLE tv_;` no longer permanently disables hook processing for the session
- [x] DISTINCT ON TVIEWs refresh correctly after a flush (dedup keys survive sort_keys)
- [x] `SAVEPOINT_DEPTH` decrement is saturating; warning emitted when pre-decrement is 0
- [x] `cargo clippy --no-default-features --features pg18 -- -D warnings` passes clean
- [x] `cargo pgrx install && psql` smoke tests pass for all three fixed paths

## TDD Cycles

### Cycle 1: F-01 — HOOK_IN_PROGRESS leaked on longjmp (hooks.rs:88,205)

**ROOT CAUSE:**
`tview_process_utility_hook` sets `HOOK_IN_PROGRESS = true` at entry (line 88).
`error!()` inside the `catch_unwind` closure triggers `ereport(ERROR)` → longjmp.
pgrx converts this longjmp into a Rust panic, so `catch_unwind` *does* catch it —
but the `Err(panic_info)` arm (line 180) re-fires `error!()`, which longjmps out
*again*, bypassing the `HOOK_IN_PROGRESS = false` reset at line 205.
Result: every subsequent utility statement is silently swallowed for that session.

**Affected paths:**
- `handle_create_table_as` — multiple `error!()` calls (empty entity name, null query,
  pattern not found, store failure, parse failure)
- `handle_drop_table` — non-IF-EXISTS drop failure `error!()`

**Note:** The COMMIT flush path (lines 95–108) is safe — it resets `HOOK_IN_PROGRESS = false`
before calling `error!()`.

**A Rust RAII guard is NOT sufficient** — the re-raised longjmp bypasses destructors.

- **RED**: Write a pgtest that triggers `CREATE TABLE tv_` (empty entity name), then
  attempts `CREATE TABLE tv_post AS SELECT ...` and verifies the second statement is
  intercepted (not silently passed through).
- **GREEN**:
  1. Change `handle_create_table_as` and `handle_drop_table` return types to
     `Result<bool, TViewError>`.
  2. Replace every `error!(...)` inside those handlers with `return Err(TViewError::...)`.
  3. In `tview_process_utility_hook`, handle the `Err` case in the match arm that is
     *outside* the `catch_unwind` block: reset `HOOK_IN_PROGRESS = false` first, then
     call `error!()`.
- **REFACTOR**: Ensure the Err-handling arm follows exactly the same pattern as the
  existing `Err(panic_info)` arm.
- **CLEANUP**: Remove any leftover `error!()` calls inside the closure; confirm no new
  clippy warnings.

### Cycle 2: P-09 — sort_keys destroys dedup-key identity (queue/graph.rs:98-118)

**ROOT CAUSE:**
`sort_keys` extracts only `key.pk` from each `RefreshKey` when building the groups map.
`RefreshKey::dedup` sets `pk = 0`, so after sorting, the output contains
`RefreshKey::pk(entity, 0)` instead of the original `RefreshKey::dedup(entity, dedup_val)`.
The `dedup_key` field is lost; `refresh_pk(view_oid, 0)` then fails or silently wrong.

- **RED**: Write a unit test that creates a mix of pk and dedup RefreshKeys, calls
  `sort_keys`, and asserts that every dedup key in the input is present (by field equality)
  in the output.
- **GREEN**: Replace the current groups map from `HashMap<String, Vec<i64>>` to
  `HashMap<String, Vec<RefreshKey>>`. Reconstruct output by extending from full
  `RefreshKey` values, not reconstructed `RefreshKey::pk` calls:
  ```rust
  pub fn sort_keys(&self, keys: Vec<RefreshKey>) -> Vec<RefreshKey> {
      let mut groups: HashMap<String, Vec<RefreshKey>> = HashMap::new();
      for key in keys {
          groups.entry(key.entity.clone()).or_default().push(key);
      }
      let mut sorted = Vec::new();
      for entity in &self.topo_order {
          if let Some(ks) = groups.remove(entity) {
              sorted.extend(ks);
          }
      }
      sorted
  }
  ```
- **REFACTOR**: No structural change needed; confirm test coverage for pk + dedup mix.
- **CLEANUP**: Run clippy; remove unused `use` if any.

### Cycle 3: F-07 — SAVEPOINT_DEPTH usize underflow (queue/xact.rs:144,157)

**ROOT CAUSE:**
Both `SUBXACT_EVENT_ABORT_SUB` and `SUBXACT_EVENT_COMMIT_SUB` unconditionally do
`*depth -= 1`. `SAVEPOINT_DEPTH` is a `usize`. If an ABORT_SUB or COMMIT_SUB arrives
without a matching START_SUB (extension loaded mid-transaction, or edge-case event
ordering), this underflows. Debug builds: panic → caught by catch_unwind → warning.
Release builds: silent wraparound to `usize::MAX`, permanently corrupting the counter.

- **RED**: Write a unit test (or pgtest) that decrement-tests a depth of 0 and asserts
  saturating behaviour (stays at 0, not wrapping).
- **GREEN**:
  ```rust
  // Before decrement in both event arms:
  if *depth == 0 {
      warning!("pg_tviews: subxact depth underflow — event ordering unexpected");
  }
  *depth = depth.saturating_sub(1);
  ```
- **REFACTOR**: Extract a helper `fn decrement_savepoint_depth(depth: &mut usize)` shared
  by both arms to avoid duplication.
- **CLEANUP**: Clippy clean; confirm no regression in savepoint-heavy smoke tests.

## Dependencies

- Requires: nothing (first phase)
- Blocks: Phase 2 (security fixes build on a correct hook)

## Status

[x] Complete
