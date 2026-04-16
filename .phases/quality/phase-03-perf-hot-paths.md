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

**ROOT CAUSE:**
`refresh_pk(source_oid, pk)` calls `TviewMeta::load_for_source(source_oid)` (SPI #1),
then passes the data to `recompute_view_row`, then to `apply_patch`, which calls
`TviewMeta::load_for_tview(row.tview_oid)` (SPI #2 — same metadata). For N refreshed
rows: 2N metadata SPI calls instead of N.

- **RED**: Write a test that calls `refresh_pk` and asserts `TviewMeta` SPI query is
  executed exactly once (via a call counter in test mode).
- **GREEN**: Thread `meta` from `refresh_pk` through to `apply_patch` — two options:
  - **Option A** (minimal change): Add `meta: &TviewMeta` parameter to `apply_patch`.
    Remove the `load_for_tview` call inside `apply_patch`.
  - **Option B** (structural): Embed `meta: TviewMeta` in `ViewRow`. Access `row.meta`
    inside `apply_patch`.
  Option A is lower risk; prefer it.
- **REFACTOR**: Update all call sites of `apply_patch`; ensure `apply_full_replacement`
  (which calls `apply_patch` internally or is called by it) also receives `meta`.
- **CLEANUP**: Delete `TviewMeta::load_for_tview` call from `apply_patch`; clippy clean.

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
