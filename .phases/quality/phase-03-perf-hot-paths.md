# Phase 3: Performance — Hot Paths

## Objective

Eliminate the four HIGH-severity performance bottlenecks that affect every DML workload:
SPI calls in `quote_identifier`, uncached extension check per refresh, double metadata
load per refresh, and per-row `TviewMeta` load in the trigger handler.

## Success Criteria

- [ ] `quote_identifier` makes zero SPI calls (pure-Rust implementation, single definition)
- [ ] `check_jsonb_delta_available` makes at most one SPI call per session
- [ ] `refresh_pk` makes one metadata SPI call per refresh (not two)
- [ ] Trigger handler makes zero SPI calls for DISTINCT ON check after first row of a
  given entity
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
- [ ] All existing tests pass; benchmark shows measurable throughput improvement on
  bulk-DML workloads (measure before/after with `EXPLAIN ANALYZE` on a 10k-row UPDATE)

## TDD Cycles

### Cycle 1: P-04 — quote_identifier issues SPI on every call (refresh/bulk.rs:134, refresh/cache.rs:115)

✅ **COMPLETE** (commit 1a40d95)

- **RED**: ✅ Added unit tests in `src/utils.rs` covering: normal identifiers, uppercase,
  underscores, and internal quote escaping.
- **GREEN**: ✅ Created canonical `pub fn quote_identifier` in `src/utils.rs`.
  Deleted duplicate definitions from `src/refresh/bulk.rs` and `src/refresh/cache.rs`.
  Updated 8 files to import from `crate::utils`.
- **REFACTOR**: ✅ Verified single definition in utils.rs; all callers import from utils.
- **CLEANUP**: ✅ Removed duplicate tests; `cargo clippy --no-default-features --features pg18 -- -D warnings` passes.

### Cycle 2: P-02 — private check_jsonb_delta_available() duplicate uncached (refresh/main.rs:518)

✅ **COMPLETE** (commit d2db8e0)

- **RED**: Inspection verified that private duplicate issues uncached SPI query per-row.
- **GREEN**: ✅ Deleted private `check_jsonb_delta_available()` from refresh/main.rs.
  Updated apply_patch() to import and use public cached version from lifecycle.rs.
- **REFACTOR**: ✅ Verified AtomicBool caching is correct. Added
  invalidate_jsonb_delta_cache() to reset JSONB_IVM_CHECKED/AVAILABLE on extension
  create/drop. Integrated into invalidate_all_caches().
- **CLEANUP**: ✅ No unused imports; clippy clean.

### Cycle 3: P-03 — double metadata load per refresh (refresh/main.rs:102, 369)

✅ **COMPLETE** (commit 93eb360)

- **RED**: Inspection verified that load_for_tview is called per-row in apply_patch.
- **GREEN**: ✅ Threaded metadata from refresh_pk → apply_patch → apply_full_replacement.
  Used Option A: Added `meta: &TviewMeta` parameter; removed load_for_tview calls.
- **REFACTOR**: ✅ Updated all call sites: refresh_pk, apply_patch (2× calls to
  apply_full_replacement). Verified TviewMeta::load_for_tview is now dead code.
- **CLEANUP**: ✅ Marked load_for_tview with #[allow(dead_code)]; clippy clean.

### Cycle 4: P-01 — TviewMeta::load_by_entity called per row in trigger (trigger.rs:50)

**ROOT CAUSE:**
`pg_tview_trigger_handler` calls `entity_for_table(table_oid)` (cached after first call),
then `TviewMeta::load_by_entity(&entity)` (full SPI roundtrip) on *every* row to check
`meta.is_distinct_on()`. For 50k-row UPDATE: 50k identical SPI roundtrips.

The `is_distinct_on` flag and `distinct_on_keys[0]` are stable metadata — they only
change if the TVIEW is dropped and recreated.

- **RED**: Write a test that fires the trigger 10 times for the same table_oid and
  asserts `TviewMeta::load_by_entity` is called at most once (on cache miss).
- **GREEN**: Extend `TABLE_ENTITY_CACHE` entries from bare `String` (entity name) to a
  small struct:
  ```rust
  struct CachedEntityInfo {
      name: String,
      distinct_on_key: Option<String>, // None → standard PK-based refresh
  }
  ```
  On cache miss, load both fields in a single query:
  ```sql
  SELECT entity, distinct_on_keys[1]
  FROM pg_tview_meta WHERE entity = $1
  ```
  The trigger hot path then reads `cached_info.distinct_on_key` directly — zero extra SPI.
- **REFACTOR**: Update `entity_for_table` (and `entity_for_table_uncached`) to return
  `CachedEntityInfo`; update all callers that previously accessed only `.name`.
- **CLEANUP**: Delete the `TviewMeta::load_by_entity` call from the trigger hot path;
  confirm `invalidate_all_caches()` also clears `TABLE_ENTITY_CACHE`.

## Dependencies

- Requires: Phase 2 complete (quote_identifier consolidation must not conflict with
  security fixes to admin.rs)
- Blocks: Phase 4

## Status

[ ] Not Started
