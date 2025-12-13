# jsonb_ivm Enhancement Implementation Plan

**Project**: Integrate new jsonb_ivm functions into pg_tviews
**Goal**: Add 4 high-value jsonb_ivm functions to improve performance and capabilities
**Estimated Total**: 5 phases
**Target Performance Gain**: 2-5× for array operations, 10× for existence checks

---

## Overview

This plan integrates new functions from the jsonb_ivm extension into pg_tviews to enhance:
1. Array element existence checking (prevent duplicates)
2. ID extraction from JSONB (cleaner code)
3. Nested path updates within array elements (complex cascades)
4. Batch array updates (bulk operations)
5. Flexible path-based updates (fallback for unknown structures)

---

## Architecture Context

**Current State**:
- pg_tviews uses 5 jsonb_ivm functions:
  - `jsonb_smart_patch_nested()` - nested object updates
  - `jsonb_smart_patch_array()` - array element updates
  - `jsonb_smart_patch_scalar()` - shallow merges
  - `jsonb_array_insert_where()` - array insertions
  - `jsonb_array_delete_where()` - array deletions

**Target State**:
- Add 4 new functions:
  - `jsonb_array_contains_id()` - existence checks
  - `jsonb_extract_id()` - ID extraction
  - `jsonb_ivm_array_update_where_path()` - nested array updates
  - `jsonb_array_update_where_batch()` - batch updates
  - `jsonb_ivm_set_path()` - flexible path updates (fallback)

**Key Files**:
- `src/refresh/array_ops.rs` - Array operation functions
- `src/refresh/main.rs` - Refresh engine with smart patching
- `src/refresh/bulk.rs` - Bulk operations
- `src/utils.rs` - Utility functions
- `test/sql/91-jsonb-ivm-performance.sql` - Performance tests

---

## Phase Breakdown

### Phase 1: Helper Functions (LOW EFFORT, HIGH VALUE)
**Duration**: 1-2 hours
**Files**: `src/utils.rs`, `src/refresh/array_ops.rs`
**Functions**: `jsonb_array_contains_id()`, `jsonb_extract_id()`
**Risk**: LOW
**Benefit**: 10× faster existence checks, cleaner ID extraction

**Deliverables**:
- Add `extract_jsonb_id()` wrapper in utils.rs
- Add `check_array_element_exists()` wrapper in array_ops.rs
- Add `insert_array_element_safe()` using existence check
- Unit tests for both functions
- Documentation updates

---

### Phase 2: Nested Path Array Updates (MEDIUM EFFORT, HIGH VALUE)
**Duration**: 2-3 hours
**Files**: `src/refresh/array_ops.rs`, `src/catalog.rs`
**Functions**: `jsonb_ivm_array_update_where_path()`
**Risk**: MEDIUM
**Benefit**: 2-3× faster for nested array element updates

**Deliverables**:
- Add `update_array_element_path()` function
- Extend dependency metadata to track nested paths in arrays
- Integration with cascade refresh logic
- Unit tests for nested path updates
- Documentation with examples

---

### Phase 3: Batch Array Updates (MEDIUM EFFORT, HIGH VALUE)
**Duration**: 3-4 hours
**Files**: `src/refresh/bulk.rs`, `src/refresh/batch.rs`
**Functions**: `jsonb_array_update_where_batch()`
**Risk**: MEDIUM
**Benefit**: 3-5× faster for bulk array updates

**Deliverables**:
- Add `update_array_elements_batch()` function
- Integration with bulk refresh engine
- Batch size optimization logic
- Unit tests for batch operations
- Performance benchmarks

---

### Phase 4: Fallback Path Operations (LOW EFFORT, MEDIUM VALUE)
**Duration**: 1-2 hours
**Files**: `src/refresh/main.rs`
**Functions**: `jsonb_ivm_set_path()`
**Risk**: LOW
**Benefit**: 2× faster for unknown/complex paths

**Deliverables**:
- Add `jsonb_ivm_set_path()` as fallback in `apply_patch()`
- Error handling for invalid paths
- Unit tests for path-based updates
- Documentation on when fallback is used

---

### Phase 5: Integration Testing & Benchmarking (CRITICAL)
**Duration**: 2-3 hours
**Files**: `test/sql/`, `docs/benchmarks/`
**Risk**: LOW
**Benefit**: Validates all improvements

**Deliverables**:
- Comprehensive integration tests for all new functions
- Performance benchmarks comparing old vs new approaches
- Update benchmark documentation
- Regression tests to prevent breakage
- Migration guide for users

---

## Success Criteria

**Functional**:
- ✅ All 4 new functions integrated and working
- ✅ Backward compatibility maintained (graceful fallback)
- ✅ All existing tests pass
- ✅ New tests cover edge cases

**Performance**:
- ✅ Array existence checks: 10× faster
- ✅ Nested array updates: 2-3× faster
- ✅ Batch operations: 3-5× faster
- ✅ Path-based fallback: 2× faster
- ✅ No regression in existing operations

**Quality**:
- ✅ Code passes clippy strict
- ✅ Documentation complete
- ✅ Error messages helpful
- ✅ Monitoring/logging added

---

## Risk Management

**Low Risk**:
- Phase 1 (helpers) - simple wrappers
- Phase 4 (fallback) - only used when needed

**Medium Risk**:
- Phase 2 (nested paths) - requires metadata changes
- Phase 3 (batch) - complex integration with bulk engine

**Mitigation**:
- Feature flags for new functions
- Graceful fallback if jsonb_ivm missing
- Comprehensive error handling
- Incremental rollout per phase

---

## Dependencies

**Required**:
- jsonb_ivm extension >= 0.2.0 (for path support)
- PostgreSQL 13+ (existing requirement)
- pgrx 0.12+ (existing requirement)

**Optional**:
- None - all features gracefully degrade

---

## Phase Execution Order

```
Phase 1 (Helpers)
    ↓
Phase 2 (Nested Paths) ← depends on Phase 1 metadata
    ↓
Phase 3 (Batch Ops) ← can run parallel with Phase 2
    ↓
Phase 4 (Fallback) ← depends on Phase 2 patterns
    ↓
Phase 5 (Testing) ← depends on all phases complete
```

**Note**: Phases 2 and 3 can be parallelized if working with multiple developers.

---

## Testing Strategy

**Per Phase**:
- Unit tests for new functions
- Integration tests with existing code
- Error handling tests
- Performance regression tests

**Final Integration**:
- End-to-end cascade tests with all functions
- Performance benchmarks vs baseline
- Stress tests with large datasets
- Concurrent operation tests

---

## Documentation Updates

**Code Documentation**:
- Rustdoc comments for all new functions
- Inline comments for complex logic
- Examples in function headers

**User Documentation**:
- Update API reference (docs/reference/api.md)
- Update performance guide (docs/operations/performance.md)
- Add migration guide for new features
- Update benchmark results (docs/benchmarks/results.md)

---

## Rollback Plan

**Per Phase**:
- Git commits per phase for easy rollback
- Feature flags to disable new functions
- Fallback to existing implementations

**Emergency**:
- All new code is additive (no removals)
- Existing functions unchanged
- Can disable jsonb_ivm checks entirely

---

## Next Steps

1. Review this plan with stakeholders
2. Set up development environment
3. Execute Phase 1 (helpers)
4. After Phase 1 verification, proceed to Phase 2
5. Continue sequential execution through Phase 5
6. Final review and merge

---

## Notes for Junior Developers

- Each phase file has detailed implementation steps
- Code examples provided for all changes
- Verification commands specified for testing
- DO NOT guardrails prevent common mistakes
- Ask questions if any step is unclear
- Run tests after EVERY change
- Commit after EVERY phase passes verification

---

**Ready to Start**: Begin with Phase 1 - see `phase-1-helper-functions.md`
