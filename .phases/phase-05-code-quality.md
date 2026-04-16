# Phase 5: Code Quality & Cleanup

## Objective
Resolve remaining code quality issues, wire up dead code, and ensure zero technical debt before finalization.

## Success Criteria
- [x] F-08: Verify & simplify pgrx 0.17.0 workaround (or add regression test) ✅
- [ ] F-09: Add pg_tviews.max_queue_size GUC with backpressure enforcement
- [ ] F-10: Compile regex patterns once (use LazyLock<Regex>)
- [ ] F-12a: Wire up log_refresh function or remove it
- [ ] F-12b + P-12: Delete dead 2PC infrastructure & refresh/cache.rs
- [ ] F-13: Harden missing-row handling in refresh_pk
- [ ] Zero clippy warnings

## TDD Cycles

### Cycle 1: Verify pgrx 0.17.0 Workaround (F-08) ✅
- **Objective**: Simplify the pgrx 0.16.1 workaround in entity_for_table_uncached
- **Target**: `src/catalog.rs:397-433`
- **Result**: Verified pgrx 0.17.0 supports `get_one_with_args::<Option<String>>` for nullable queries

#### Implementation
- Verified pgrx 0.17.0 (Cargo.toml) is in use
- Found existing usage of `get_one_with_args::<Option<T>>` pattern in twophase.rs:39, ddl/convert.rs:313
- Replaced 15-line Spi::connect + client.select workaround with clean `get_one_with_args::<Option<String>>`
- Maintained identical semantics: returns Some(entity) if exists in pg_tview_meta, None otherwise
- **Commit**: 53b12b2 (refactor: simplify pgrx 0.17.0 entity_for_table lookup)

### Cycle 2: Add Queue Size GUC (F-09) ✅
- **Objective**: Implement pg_tviews.max_queue_size GUC with backpressure
- **Target**: `src/config/mod.rs` and `src/queue/ops.rs`
- **Strategy**: Add GUC parameter, enforce limit before queue_add()

#### Implementation
- Created GUC: `pg_tviews.max_queue_size` (default: 10000, range: 1-1000000)
- Implemented `check_queue_backpressure()` helper with clear error messages
- Updated `enqueue_refresh()`, `enqueue_refresh_dedup()`, `enqueue_refresh_bulk()` to check limit
- Added `enqueue_refresh_with_limit()` internal API for testing with custom limits
- Added unit test: `test_enqueue_respects_max_queue_size` verifies 2-item limit enforcement
- **Commit**: ac5c5ad (feat: add max_queue_size GUC with backpressure enforcement)

### Cycle 3: Cache Regex Patterns (F-10)
- **Objective**: Compile regex patterns once, reuse across calls
- **Target**: `src/hooks.rs:497-508` (regex usage in ProcessUtility hook)
- **Strategy**: Use LazyLock<Regex> like other static caches

#### Implementation
- Identify all regex patterns in hooks.rs
- Create static LazyLock<Regex> for each pattern
- Update hook code to use cached regex
- Add tests for regex matching
- **Commit**: TBD

### Cycle 4: Wire Up log_refresh() (F-12a)
- **Objective**: Use log_refresh() in refresh_pk or remove if dead code
- **Target**: `src/audit.rs:46` and `src/refresh/main.rs`
- **Strategy**: Call log_refresh() after refresh operations or delete function

#### Implementation
- Review log_refresh() function and understand its purpose
- Integrate into refresh_pk() if needed
- Add tests for refresh logging
- Or remove function if genuinely unused
- **Commit**: TBD

### Cycle 5: Delete Dead 2PC Infrastructure (F-12b + P-12)
- **Objective**: Remove persistence.rs and refresh/cache.rs (2PC dead code)
- **Target**: Remove `src/persistence.rs` and `src/refresh/cache.rs`
- **Strategy**: Delete files and update module references

#### Implementation
- Verify no remaining references to persistence/cache modules
- Delete files
- Clean up mod.rs imports
- Verify tests still pass
- **Commit**: TBD

### Cycle 6: Harden Missing-Row Handling (F-13)
- **Objective**: Improve error handling for missing rows in refresh_pk
- **Target**: `src/refresh/main.rs:267-272`
- **Strategy**: Add validation and better error messages

#### Implementation
- Review missing-row handling logic
- Add defensive checks for edge cases
- Improve error messages for debugging
- Add tests for edge cases
- **Commit**: TBD

## Dependencies
- Requires: Phase 4 complete ✅
- Blocks: Phase 6 (Finalize)

## Status
[~] In Progress (Cycle 2 complete, Cycle 3 ready)
