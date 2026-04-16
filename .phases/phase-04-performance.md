# Phase 4: Performance — Caching & Batching

## Objective
Eliminate redundant SPI queries and optimize hot paths through caching and batching.

## Success Criteria
- [ ] P-05: OID→relname caching (session-level) ✅
- [ ] P-06: EntityDepGraph caching in propagation ✅
- [ ] P-07: Batch affected-PK queries (multi-key propagation) ✅
- [ ] P-08: Cache view column names in DISTINCT ON refresh ✅
- [ ] P-10: Pre-allocate collections for bulk operations
- [ ] P-11: Optimize dedup key refresh DML generation

## TDD Cycles

### Cycle 1: OID→relname Caching (P-05) ✅
- **RED**: Test that repeated `relname_from_oid()` calls hit the cache
- **GREEN**: Implement `OID_RELNAME_CACHE` static with LazyLock<Mutex<HashMap>>
- **REFACTOR**: Extract cache logic into utils module
- **CLEANUP**: Remove debug prints, pass lints
- **Commit**: 7b909f8

### Cycle 2: EntityDepGraph Caching in Propagation (P-06) ✅
- **RED**: Test that parent discovery uses cached graph instead of SPI queries
- **GREEN**: Thread graph reference through find_parents_for() and find_parent_entities()
- **REFACTOR**: Export EntityDepGraph as public API from queue module
- **CLEANUP**: Update both xact.rs and twophase.rs callers
- **Commit**: 9c0ca8d

### Cycle 3: Batch Affected-PK Queries (P-07) 🔄
- **Objective**: Reduce SPI queries when discovering parents for multiple changed PKs

#### Context
Current flow (lines 266-285 in `src/queue/xact.rs`):
```rust
for key in &entity_keys {
    // For N keys × M parent entities = N*M find_affected_pks() calls
    let parents = crate::propagate::find_parents_for(key, &graph)?;
    // Each call queries: SELECT pk_parent FROM tv_parent WHERE fk_child = $1
}
```

**Problem**: With 3 changed user PKs and 2 parent entities (post, comment):
- 6 separate SPI queries instead of 2 batched queries

**Solution**: Create `find_parents_batch()` that:
1. Collects all (parent_entity, child_entity, child_pk) tuples
2. Groups by (parent_entity, child_entity)
3. Issues one parameterized query per group: `SELECT pk_parent, fk_child FROM tv_parent WHERE fk_child = ANY($1)`
4. Returns Map<RefreshKey, Vec<RefreshKey>> for bulk lookup

#### RED
- Test that `find_parents_batch([user:1, user:2, user:3])` returns 6 parent PKs with single SPI round trip
- Verify dedup keys are still handled correctly (returned empty)
- Verify cycle graphs still work (no duplicate parents)

#### GREEN
- Implement `find_parents_batch(keys: &[RefreshKey], graph) -> Result<Map<RefreshKey, Vec<RefreshKey>>>`
- Use PostgreSQL `= ANY($1)` parameterization for IN lists
- Keep `find_affected_pks()` unchanged (used elsewhere)
- Update xact.rs line 276-285 loop to call batched version once

#### REFACTOR
- Consider extracting common batch-building logic
- Review transaction context (still inside SPI_connect)
- Ensure error handling is consistent

#### CLEANUP
- Run lints, fix clippy warnings
- No commented code
- Commit with clear message

### Cycle 4: Cache View Column Names (P-08) ✅
- **Objective**: Avoid repeated `pg_attribute` queries in DISTINCT ON refresh
- **Target**: `get_view_columns()` in `src/refresh/main.rs` (line 207)
- **Strategy**: Session-level cache like OID_RELNAME_CACHE
- **Metric**: DISTINCT ON TVIEWs with many dedup keys (currently query per key)

#### Implementation
- Created `VIEW_COLUMNS_CACHE` static in `src/utils.rs` (HashMap<String, Vec<String>>)
- Created `invalidate_view_columns_cache()` function
- Updated `get_view_columns()` to check cache first (fast path), then query and cache (slow path)
- Integrated cache invalidation into `invalidate_all_caches()` in `src/queue/cache.rs`
- Added comprehensive test for cache invalidation
- **Commit**: b756548

### Cycle 5: Pre-allocate Collections (P-10)
- **Objective**: Reduce allocations in hot refresh paths
- **Target**: `Vec::with_capacity()` for known sizes
- **Examples**: `parent_keys.reserve()`, `affected_pks.reserve()`

### Cycle 6: Optimize Dedup Key DML (P-11)
- **Objective**: Reduce query string construction overhead
- **Target**: `refresh_by_dedup_key()` builds same columns list repeatedly
- **Strategy**: Precompute or cache column list

## Dependencies
- Requires: Phase 3 complete
- Blocks: None

## Status
[~] In Progress (Cycle 4 complete, Cycle 5 ready)
