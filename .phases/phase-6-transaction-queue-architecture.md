# Phase 6: Transaction-Level Queue Architecture

**Status:** READY TO START
**Prerequisites:** Phase 5 Task 7 Complete ✅
**PRD Reference:** `PRD_multiupdate.md`
**Gap Analysis:** `docs/PRD_MULTIUPDATE_IMPACT_ANALYSIS.md`
**Estimated Time:** 7-11 days across 4 sub-phases

---

## Overview

This phase implements the transaction-level queue architecture specified in `PRD_multiupdate.md`. The current implementation uses **immediate refresh** (triggers call `pg_tviews_cascade()` directly), but the PRD requires **commit-time coalesced refresh** with proper dependency ordering.

### Key Requirements (from PRD)

- **R1**: Refresh coalescing - Each `(entity, pk)` refreshed at most once per transaction
- **R2**: End-of-transaction semantics - Refreshes run before commit completes
- **R3**: Dependency-correct order - Topological sort of entity dependencies
- **R4**: Propagation coalescing - Parent refreshes also go through queue
- **R5**: No extra round trips - FraiseQL writes to `tb_*`, reads from `tv_*`

### Current vs Target Architecture

```rust
// CURRENT (Phases 1-5): Immediate refresh
#[pg_trigger]
fn pg_tview_trigger_handler(trigger: ...) {
    let table_oid = trigger.relation()?.oid();
    let pk_value = extract_pk(trigger)?;
    pg_tviews_cascade(table_oid, pk_value); // IMMEDIATE
    Ok(None)
}

// TARGET (Phase 6): Transaction queue with commit-time flush
thread_local! {
    static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = RefCell::new(HashSet::new());
    static TX_REFRESH_SCHEDULED: RefCell<bool> = RefCell::new(false);
}

#[pg_trigger]
fn pg_tview_trigger_handler(trigger: ...) {
    let entity = entity_for_table(table_oid)?;
    let pk = extract_pk(trigger)?;
    enqueue_refresh(entity, pk); // DEFERRED
    register_commit_callback_once()?;
    Ok(None)
}

fn tx_commit_handler() {
    // Flush queue at commit time in dependency order
    let queue = take_queue_snapshot();
    for key in sorted_by_dependency_order(queue) {
        refresh_entity_pk(&key)?;
        let parents = parents_for(&key)?;
        enqueue_all(parents); // Propagation also coalesced
    }
}
```

---

## Phase Breakdown

### Phase 6A: Foundation (2-3 days)
- RefreshKey data structure
- Thread-local queue infrastructure
- Transaction callback hooks
- Basic enqueue/dequeue functions

### Phase 6B: Trigger Refactoring (1-2 days)
- Convert triggers to enqueue-only
- Remove immediate cascade calls
- Entity name resolution from table OID

### Phase 6C: Commit-Time Processing (3-4 days)
- Commit callback handler
- Queue flush logic
- Dependency-ordered processing
- Integration with existing refresh functions

### Phase 6D: Entity Dependency Graph (1-2 days)
- Precompute entity dependencies
- Topological sorting
- Graph caching and invalidation

---

## Implementation Plan

Each sub-phase has a detailed plan file:

1. `.phases/phase-6a-foundation.md` - Data structures and infrastructure
2. `.phases/phase-6b-trigger-refactor.md` - Convert triggers to enqueue-only
3. `.phases/phase-6c-commit-processing.md` - Implement commit-time flush
4. `.phases/phase-6d-entity-graph.md` - Dependency graph optimization

---

## Breaking Changes

⚠️ **IMPORTANT**: This is a breaking architectural change.

### Behavior Change

**Before Phase 6** (Immediate refresh):
```rust
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Trigger fires → tv_user row updated IMMEDIATELY
-- Propagation happens IMMEDIATELY (tv_post, tv_feed, etc.)
SELECT * FROM tv_user WHERE pk_user = 1; -- Sees updated data
COMMIT;
```

**After Phase 6** (Deferred refresh):
```rust
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Trigger fires → enqueue (1, 'user') in transaction-local queue
SELECT * FROM tv_user WHERE pk_user = 1; -- ⚠️ OLD DATA (not yet refreshed)
COMMIT; -- ← Refresh happens HERE, before commit completes
```

### Implications

1. **Within-transaction reads** will see stale TVIEW data until commit
2. **Multi-update coalescing** now works correctly (main benefit)
3. **Performance**: Slightly better (deduplication) or slightly worse (deferred overhead)
4. **Correctness**: MUCH better for multi-update workloads

### Migration Strategy

1. Implement Phase 6 on a feature branch
2. Test extensively with existing workloads
3. Document the behavior change in CHANGELOG
4. Consider a `pg_tviews.refresh_mode` GUC setting:
   - `immediate` (legacy, Phase 1-5 behavior)
   - `deferred` (Phase 6, default)
5. Users can opt into legacy mode if needed

---

## Testing Strategy

### Unit Tests

- Queue enqueue/dequeue operations
- Commit callback registration
- Refresh coalescing (same entity+pk multiple times)
- Dependency ordering

### Integration Tests

- Multi-update transactions (10 updates to same row)
- Cross-entity propagation (user → post → feed)
- Transaction rollback (queue cleared)
- Nested transactions (savepoints)

### Performance Tests

- Benchmark: 100 updates to 10 different users
  - Before: 100 immediate refreshes (cascades × 100)
  - After: 10 coalesced refreshes (cascades × 1)
- Measure queue overhead vs. coalescing benefit

### Regression Tests

- Existing Phase 1-5 tests must still pass
- Single-update transactions (no change in behavior)
- Cascade propagation correctness

---

## Acceptance Criteria

- ✅ All 5 PRD requirements (R1-R5) met
- ✅ Tests pass with deferred refresh mode
- ✅ Performance ≥ 2× improvement on multi-update workloads
- ✅ No regression on single-update workloads
- ✅ Documentation updated (CHANGELOG, README, ARCHITECTURE)
- ✅ Breaking change clearly communicated

---

## Rollback Plan

If Phase 6 introduces critical issues:

1. Revert to commit before Phase 6
2. Re-enable Phase 1-5 immediate refresh mode
3. Implement Phase 6 fixes on a separate branch
4. Re-test before re-merging

The Phase 1-5 implementation is fully functional and can serve as a fallback.

---

## Next Steps

1. Review this plan with stakeholders
2. Start with Phase 6A (foundation)
3. Implement incrementally with tests at each phase
4. Document behavior changes as you go

**Read next**:
- `.phases/phase-6a-foundation.md` - Start implementation
- `.phases/phase-6-known-limitations.md` - Edge cases and future enhancements

---

## Additional Documentation

- **Known Limitations**: See `.phases/phase-6-known-limitations.md` for detailed analysis of:
  - Savepoint rollback issues
  - Prepared transaction (2PC) limitations
  - Deep dependency chain limits
  - Foreign key cascade edge cases
  - Large queue memory considerations
