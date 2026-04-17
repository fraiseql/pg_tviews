# Phase 4: Cache Backend Migration & Dead Code Removal

## Objective

1. Migrate session-scoped caches from `LazyLock<Mutex<...>>` to `thread_local! + RefCell`
   to eliminate unnecessary atomic operations in PostgreSQL's single-threaded backend.
2. Audit and remove or mark the superseded `cascade.rs` code.
3. Clean up the `DependencyType::from_str` naming conflict.

## Why This Matters

**Cache backend**: PostgreSQL backends are single-threaded. Every `Mutex::lock()` call
performs an atomic compare-and-swap that can't contend but still costs ~10-20ns per
call. With 6 global Mutex caches hit multiple times per trigger, this adds up in
high-throughput scenarios. `RefCell::borrow()` is a simple integer check (~1ns).

**Risk note**: `thread_local!` in shared libraries loaded via `dlopen` (as pgrx
extensions are) can have subtle initialization/teardown issues. Before committing
to this migration, verify that pgrx itself or other pgrx extensions use
`thread_local!` successfully. If issues arise, the fallback is to keep `Mutex` —
the perf difference is measurable but not critical.

**Dead code**: `cascade.rs` exposes `pg_tviews_cascade`, `pg_tviews_insert`, and
`pg_tviews_delete` as `#[pg_extern]` SQL functions. These bypass the queue-based
refresh system entirely (they enqueue but don't go through the dependency-ordered
flush). If called directly, they could cause stale or double-refreshed TVIEWs.

## Success Criteria

- [ ] All 6 session-scoped caches use `thread_local! + RefCell` instead of `LazyLock<Mutex>`
- [ ] `PENDING_TVIEW_SELECTS` in hooks.rs also migrated (it's session-scoped)
- [ ] `cascade.rs` functions either removed or clearly documented as legacy/debug
- [ ] `DependencyType::from_str` renamed to avoid shadowing `FromStr` trait
- [ ] `cargo clippy --no-default-features --features pg18` clean
- [ ] `cargo test` passes

## Caches To Migrate

| Cache | Location | Type |
|-------|----------|------|
| `OID_RELNAME_CACHE` | `utils.rs:173` | `HashMap<Oid, String>` |
| `VIEW_COLUMNS_CACHE` | `utils.rs:185` | `HashMap<String, Vec<String>>` |
| `DEDUP_DML_CACHE` | `utils.rs:201` | `HashMap<String, (String, String)>` |
| `TABLE_ENTITY_CACHE` | `queue/cache.rs:18` | `HashMap<Oid, Option<CachedEntityInfo>>` |
| `ENTITY_GRAPH_CACHE` | `queue/cache.rs:14` | `Option<EntityDepGraph>` |
| `PENDING_TVIEW_SELECTS` | `hooks.rs:437` | `HashMap<String, (String, String)>` |

Note: `JSONB_IVM_AVAILABLE` / `JSONB_IVM_CHECKED` in `lifecycle.rs` use `AtomicBool`
which is already zero-overhead for single-threaded access. Leave them as-is.

## Required Import Changes

In each migrated file, replace:
```rust
use std::sync::{LazyLock, Mutex};           // remove
use std::sync::PoisonError;                  // remove (if present)
```
with:
```rust
use std::cell::RefCell;
```

`thread_local!` is in the prelude — no import needed.

## Files To Change

### 1. `src/utils.rs` — Migrate 3 caches

Replace each `static ... LazyLock<Mutex<...>>` with:

```rust
thread_local! {
    static OID_RELNAME_CACHE: RefCell<HashMap<Oid, String>> =
        RefCell::new(HashMap::new());
}
```

Update all access patterns from:
```rust
let cache = OID_RELNAME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
```
to:
```rust
OID_RELNAME_CACHE.with(|c| {
    let cache = c.borrow();
    // read...
});
```

**Important**: The caches are currently `pub static` so `refresh/main.rs` and other
modules access them directly. After migration, make the thread_locals module-private
and expose access through functions. Here is the exhaustive list of external call
sites that need updating:

**`VIEW_COLUMNS_CACHE`** (external accesses in `src/refresh/main.rs`):
- Line 235: `crate::utils::VIEW_COLUMNS_CACHE.lock().unwrap_or_else(...)` (read)
- Line 268: `crate::utils::VIEW_COLUMNS_CACHE.lock().unwrap_or_else(...)` (write/insert)
- After Phase 2, these will be inside the moved `get_view_columns` function in
  `utils.rs`, so they become internal accesses. No external accessor needed.

**`DEDUP_DML_CACHE`** (external accesses in `src/refresh/main.rs`):
- Lines 183-186: `crate::utils::DEDUP_DML_CACHE.lock().unwrap_or_else(|e| e.into_inner())` — reads via `.get(&view_name).cloned()`
- Lines 201-204: `crate::utils::DEDUP_DML_CACHE.lock().unwrap_or_else(|e| e.into_inner())` — writes via `.insert(view_name.clone(), dml.clone())`
- Replace with two new public functions in `utils.rs`:
  ```rust
  pub fn get_dedup_dml(view_name: &str) -> Option<(String, String)> {
      DEDUP_DML_CACHE.with(|c| c.borrow().get(view_name).cloned())
  }

  pub fn set_dedup_dml(view_name: &str, dml: (String, String)) {
      DEDUP_DML_CACHE.with(|c| {
          c.borrow_mut().insert(view_name.to_string(), dml);
      });
  }
  ```

**`OID_RELNAME_CACHE`** — all accesses are internal to `utils.rs` (lines 179, 216,
249, 309, 315, 324). No external accessor needed; just rewrite inline.

**Internal `utils.rs` accesses that need rewriting** (all follow the same pattern):
- Line 179: `OID_RELNAME_CACHE.lock().unwrap_or_else(|e| e.into_inner())` → `.with(|c| c.borrow_mut()...)`
- Line 191: `VIEW_COLUMNS_CACHE.lock().unwrap_or_else(...)` → same
- Line 207: `DEDUP_DML_CACHE.lock().unwrap_or_else(...)` → same
- Line 216: `OID_RELNAME_CACHE.lock().unwrap_or_else(...)` → same (read)
- Line 249: `OID_RELNAME_CACHE.lock().unwrap_or_else(...)` → same
- Tests (lines 309, 315, 324, 336, 349, 360): use `.with(|c| ...)` pattern

### 2. `src/queue/cache.rs` — Migrate 2 caches

Same pattern for `ENTITY_GRAPH_CACHE` and `TABLE_ENTITY_CACHE`.

These are already accessed through module functions (`load_cached`, `entity_info_cached`,
`invalidate`), so the API doesn't change — only the internal implementation.

### 3. `src/hooks.rs` — Migrate `PENDING_TVIEW_SELECTS`

Replace `LazyLock<Mutex<HashMap<...>>>` (line 437-438) with `thread_local! + RefCell`.

Two access points (API signatures don't change, only internals):

**`store_pending_tview_select`** (line 413-427):
```rust
// BEFORE:
fn store_pending_tview_select(...) -> Result<(), String> {
    PENDING_TVIEW_SELECTS
        .lock()
        .map_err(|e| format!("Failed to lock cache: {e}"))?
        .insert(table_name.to_string(), (schema_name.to_string(), select_sql.to_string()));
    Ok(())
}

// AFTER: RefCell has no lock poisoning, so the error path is removed:
fn store_pending_tview_select(...) -> Result<(), String> {
    PENDING_TVIEW_SELECTS.with(|c| {
        c.borrow_mut().insert(
            table_name.to_string(),
            (schema_name.to_string(), select_sql.to_string()),
        );
    });
    Ok(())
}
```

**`take_pending_tview_select`** (line 445-446):
```rust
// BEFORE:
PENDING_TVIEW_SELECTS.lock().ok()?.remove(table_name)

// AFTER:
PENDING_TVIEW_SELECTS.with(|c| c.borrow_mut().remove(table_name))
```

Call sites (lines 344 and 363) call `store_pending_tview_select` — no changes needed
there since the function signature is unchanged.

### 4. `src/cascade.rs` — Dead code assessment

After reviewing the codebase:

- `pg_tviews_cascade`, `pg_tviews_insert`, `pg_tviews_delete` are `#[pg_extern]`
  functions that enqueue refreshes but bypass the dependency-ordered queue flush.
- They duplicate logic already handled by the trigger system (`trigger.rs`).
- `find_dependent_tviews` does a separate `pg_tview_meta` query that duplicates
  what `parent_entities_for_base_table` in `catalog.rs` already does.
- `find_affected_tview_rows` duplicates what `propagate.rs::find_affected_pks` does.

**Decision**: These functions should be removed. They are not referenced by any
trigger installation code — the actual triggers use `pg_tview_trigger_handler` and
`pg_tview_flush_trigger` (from `trigger.rs`).

**Action**:
- Remove the entire `src/cascade.rs` file
- Remove `mod cascade;` from `src/lib.rs`
- Verify no SQL migration script references these function names

If there's any concern about removing `#[pg_extern]` functions that users may call
directly, an alternative is to keep them but have them delegate to the queue:

```rust
#[pg_extern]
#[deprecated(note = "Use trigger-based refresh instead")]
fn pg_tviews_cascade(base_table_oid: pg_sys::Oid, pk_value: i64) {
    // Just enqueue — the flush will handle ordering
    // ...
}
```

The simpler approach (full removal) is preferred since this is pre-1.0.

**Migration note**: Since these are `#[pg_extern]`, pgrx generates SQL function
definitions during `cargo pgrx install`. Existing databases that ran
`CREATE EXTENSION pg_tviews` will have these functions. On upgrade, the functions
become orphaned (won't crash, but will error if called). Acceptable for pre-1.0
but worth noting in a changelog.

### 5. `src/catalog.rs` — Rename `DependencyType::from_str`

```rust
// BEFORE:
impl DependencyType {
    pub fn from_str(s: &str) -> Self { ... }
}

// AFTER:
impl DependencyType {
    pub fn parse(s: &str) -> Self { ... }
}
```

Update the single call site in `TviewMeta::parse_dependency_types` (line 92):
```rust
.map(|s| DependencyType::parse(&s))
```

## TDD Cycles

### Cycle 1: Migrate utils.rs caches

- **RED**: Existing unit tests for cache invalidation pass
- **GREEN**: Replace LazyLock<Mutex> with thread_local! + RefCell for all 3 caches.
  Add accessor functions for `DEDUP_DML_CACHE`. Update call sites.
- **REFACTOR**: Remove `pub static` exports that exposed cache internals.
  Everything goes through functions.
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 2: Migrate queue/cache.rs caches

- **RED**: `test_graph_cache_invalidation` and `test_table_cache_invalidation` pass
- **GREEN**: Replace LazyLock<Mutex> with thread_local! + RefCell for both caches.
- **REFACTOR**: Simplify error handling (no more PoisonError).
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 3: Migrate hooks.rs cache + simplify

- **RED**: No dedicated tests for the pending select cache
- **GREEN**: Replace LazyLock<Mutex> with thread_local! + RefCell.
  Remove the `map_err` lock-poisoning error path.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 4: Remove cascade.rs

- **RED**: Search for references to cascade functions:
  `grep -rn 'pg_tviews_cascade\|pg_tviews_insert\|pg_tviews_delete\|mod cascade' src/`
  Verify only `lib.rs` and `cascade.rs` reference them.
- **GREEN**: Delete `src/cascade.rs`. Remove `mod cascade;` from `lib.rs`.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 5: Rename DependencyType::from_str

- **RED**: `test_dependency_type_from_str` in `catalog.rs` passes
- **GREEN**: Rename method to `parse`. Update call site and test.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

## Verification

```bash
cargo clippy --no-default-features --features pg18 -- -D warnings
cargo test

# Verify no Mutex remains in session caches:
grep -rn 'LazyLock.*Mutex' src/ --include='*.rs'
# Should only show lifecycle.rs AtomicBool (not Mutex) — zero results expected

# Verify cascade.rs is gone:
test ! -f src/cascade.rs && echo "OK: cascade.rs removed"
```

## Dependencies

- Requires: Phase 3 (cache type changed in Phase 3 for negative caching)
- Blocks: Phase 5 (finalize)

## Status
[ ] Not Started
