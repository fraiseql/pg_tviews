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

### Cycle 3: Cache Regex Patterns (F-10) ✅
- **Objective**: Compile regex patterns once, reuse across calls
- **Target**: `src/parser/mod.rs` and `src/schema/analyzer.rs` (regex usage in parsing & analysis)
- **Strategy**: Use LazyLock<Regex> like other static caches

#### Implementation
- Added LazyLock<Regex> static CREATE_TVIEW_REGEX in parser/mod.rs for CREATE TABLE parsing
- Added LazyLock<Regex> static patterns in analyzer.rs:
  - ARRAY_PATTERN_REGEX: 'array_name', jsonb_agg(v_*.data)
  - INLINE_ARRAY_PATTERN_REGEX: 'array_name', jsonb_agg(build_object)
- Replaced Regex::new() calls with cached static references
- Added test: test_create_tview_regex_cached verifies caching
- Dynamic patterns in detect_dependency_type still compile per FK (unavoidable)
- **Commit**: 4766309 (feat: cache regex patterns with LazyLock)

### Cycle 4: Wire Up log_refresh() (F-12a) ✅
- **Objective**: Use log_refresh() in refresh_pk or remove if dead code
- **Target**: `src/audit.rs:46` and `src/refresh/main.rs`
- **Strategy**: Call log_refresh() after refresh operations or delete function

#### Implementation
- Review log_refresh() function and understand its purpose ✅
- Integrate into refresh_pk() and refresh_by_dedup_key() ✅
- Add tests for refresh logging ✅
- Removed #[allow(dead_code)] attribute (function now actively used)
- **Commit**: 3d1ed27 (feat: wire up log_refresh in refresh operations)

### Cycle 5: Delete Dead 2PC Infrastructure (F-12b + P-12) ✅
- **Objective**: Remove persistence.rs and refresh/cache.rs (2PC dead code)
- **Target**: Remove `src/persistence.rs` and `src/refresh/cache.rs`
- **Strategy**: Delete files and update module references

#### Implementation
- Deleted src/twophase.rs (2PC SQL functions never called)
- Deleted src/queue/persistence.rs (only used by non-existent PREPARE support)
- Deleted src/refresh/cache.rs (planned optimization, never integrated)
- Updated src/lib.rs: removed twophase module, removed 2PC from feature list
- Updated src/queue/mod.rs: removed persistence module declaration
- Updated src/refresh/mod.rs: removed cache module declaration
- Verified no internal references to deleted modules
- Code compiles cleanly with zero clippy warnings
- **Commit**: d11c5e5 (refactor: remove dead 2PC infrastructure)

### Cycle 6: Harden Missing-Row Handling (F-13) ✅
- **Objective**: Improve error handling for missing rows in refresh_pk
- **Target**: `src/refresh/main.rs:267-272`
- **Strategy**: Add validation and better error messages

#### Implementation
- Enhanced missing row error message with:
  * Entity name and view name for context
  * Actual SQL query being executed
  * List of possible causes (cascading delete, UNION ALL condition, view WHERE clause)
- Enhanced NULL data column error with similar improvements
- Added test_missing_row_error_handling to verify deletion handling
- Added test_null_data_column_error_handling to verify NULL column handling
- **Commit**: afd6979 (feat: harden missing-row error handling)

## Dependencies
- Requires: Phase 4 complete ✅
- Blocks: Phase 6 (Finalize)

## Status
[~] In Progress (Cycle 6 complete, Cycle 7 ready)
