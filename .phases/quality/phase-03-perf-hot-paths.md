# Phase 3: Performance — Hot Paths

## Objective

Eliminate the four HIGH-severity performance bottlenecks that affect every DML workload:
SPI calls in `quote_identifier`, uncached extension check per refresh, double metadata
load per refresh, and per-row `TviewMeta` load in the trigger handler.

## Success Criteria

- [x] `quote_identifier` makes zero SPI calls (pure-Rust implementation, single definition)
- [x] `check_jsonb_delta_available` makes at most one SPI call per session
- [x] `refresh_pk` makes one metadata SPI call per refresh (not two)
- [ ] Trigger handler makes zero SPI calls for DISTINCT ON check after first row of a
  given entity (P-01 REFACTOR phase pending)
- [x] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
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

⏳ **IN PROGRESS** (infrastructure complete: commit 82bfa5a)

- **RED**: Inspection verified that load_by_entity is called per-row in trigger handler (line 49).
- **GREEN**: ✅ Created `CachedEntityInfo` struct. Extended `TABLE_ENTITY_CACHE` from 
  `HashMap<Oid, String>` to `HashMap<Oid, CachedEntityInfo>`. Added `entity_info_cached()` 
  alongside backward-compatible `entity_for_table_cached()`.
- **REFACTOR**: PENDING - Update trigger.rs to use `entity_info_cached()` to get 
  `distinct_on_key` directly from cache instead of calling `TviewMeta::load_by_entity()`.
- **CLEANUP**: PENDING - Verify `TviewMeta::load_by_entity` call removed from trigger hot path.

**Note**: Distinct_on_key is currently cached as `None` (TODO to query from pg_tview_meta).
Trigger handler will use cache hit for entity name and fallback to load_by_entity for
metadata until full P-01 REFACTOR is complete.

## Dependencies

- Requires: Phase 2 complete (quote_identifier consolidation must not conflict with
  security fixes to admin.rs)
- Blocks: Phase 4

## Status

[~] In Progress (Cycles 1-3 complete, Cycle 4 infrastructure in place, REFACTOR pending)
