# Phase 6 Quick Reference

**For:** Developers implementing Phase 6 transaction-queue architecture
**Status:** Ready for implementation
**Last Updated:** 2025-12-10

---

## 📋 Implementation Checklist

### Phase 6A: Foundation (2-3 days)
- [ ] Create `src/queue/` module structure
- [ ] Implement `RefreshKey` type (Hash + Eq)
- [ ] Implement thread-local `TX_REFRESH_QUEUE`
- [ ] Add basic enqueue/dequeue functions
- [ ] All unit tests pass (6 tests expected)

### Phase 6B: Trigger Refactoring (1-2 days)
- [ ] Implement `entity_for_table()` mapping
- [ ] Convert triggers to enqueue-only
- [ ] Remove immediate `pg_tviews_cascade()` calls
- [ ] Triggers compile without errors
- [ ] ⚠️ **Known limitation**: TVIEWs remain stale (Phase 6C will fix)

### Phase 6C: Commit Processing (3-4 days)
- [ ] Implement `RegisterXactCallback` FFI
- [ ] Create PRE_COMMIT handler
- [ ] **CRITICAL**: Use fail-fast error handling (abort on failure)
- [ ] Implement ABORT handler (clear queue)
- [ ] ⚠️ **INCOMPLETE**: No dependency ordering yet (Phase 6D required)
- [ ] ⚠️ **DO NOT DEPLOY** Phase 6C alone - wait for Phase 6D

### Phase 6D: Entity Graph (1-2 days)
- [ ] Implement `EntityDepGraph` with topological sort
- [ ] **Rewrite** `handle_pre_commit()` with local pending queue
- [ ] Add `find_parents_for()` to propagate.rs
- [ ] Implement `PropagationDepthExceeded` error
- [ ] Add graph caching with invalidation
- [ ] All integration tests pass
- [ ] ✅ **READY FOR PRODUCTION** after Phase 6D

---

## ⚠️ Critical Issues to Avoid

### 1. Error Handling in PRE_COMMIT

```rust
// ❌ WRONG - Swallows errors
if let Err(e) = handle_pre_commit() {
    warning!("Failed: {:?}", e);  // Transaction commits anyway!
}

// ✅ CORRECT - Aborts transaction
if let Err(e) = handle_pre_commit() {
    error!("Failed: {:?}", e);  // PostgreSQL aborts transaction
}
```

### 2. Propagation Queue Integration

```rust
// ❌ WRONG - Parents go to empty queue
let queue = take_queue_snapshot();  // Clears TX_REFRESH_QUEUE
for key in queue {
    refresh_and_enqueue_parents(&key)?;  // Parents lost!
}

// ✅ CORRECT - Use local pending queue
let mut pending = take_queue_snapshot();
while !pending.is_empty() {
    let sorted = graph.sort_keys(pending.drain().collect());
    for key in sorted {
        let parents = refresh_and_get_parents(&key)?;
        pending.extend(parents);  // Add to LOCAL queue
    }
}
```

### 3. Dependency Ordering

```rust
// ❌ WRONG - HashSet iteration is non-deterministic
for key in queue {  // Random order!
    refresh_entity_pk(&key)?;
}

// ✅ CORRECT - Topological sort
let sorted = graph.sort_keys(queue.into_iter().collect());
for key in sorted {
    refresh_entity_pk(&key)?;
}
```

---

## 📖 Documentation Structure

```
.phases/
├── phase-6-transaction-queue-architecture.md   ← Start here (overview)
├── phase-6a-foundation.md                      ← Implement first
├── phase-6b-trigger-refactor.md                ← Then this
├── phase-6c-commit-processing.md               ← Then this (INCOMPLETE alone)
├── phase-6d-entity-graph.md                    ← Complete with this
├── phase-6-known-limitations.md                ← Read before starting
├── PHASE6_IMPROVEMENTS_SUMMARY.md              ← Review findings
└── PHASE6_QUICKREF.md                          ← This file
```

---

## 🧪 Test Strategy

### Unit Tests (Phase 6A)
- RefreshKey equality and hashing
- Thread-local queue operations
- Enqueue/dequeue correctness

### Integration Tests (Phase 6C)
- Transaction callback registration
- Queue flush at commit
- Queue clear on abort
- Coalescing (10 updates → 1 refresh)

### End-to-End Tests (Phase 6D)
- Multi-entity transactions
- Dependency-ordered refresh
- Propagation coalescing
- Deep cascade (10+ levels)

### Edge Case Tests (Phase 6D)
- Savepoint rollback (expect limitation)
- FK cascade + delete
- Large queue (10K entries)
- Propagation depth exceeded (>100 levels)

---

## 🚀 Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| Single update overhead | < 1ms | Queue enqueue cost |
| 10 updates (same row) | 1× refresh | Coalescing working |
| 100 updates | < 500ms | Batch optimization active |
| 1000 updates | < 5s | Dependency sorting overhead |
| Queue memory (10K) | < 1MB | ~50 bytes per entry |

---

## 🔍 Debugging Tips

### Check Queue State

```sql
-- Add debug function (Phase 6A):
CREATE FUNCTION pg_tviews_debug_queue()
RETURNS TABLE(entity TEXT, pk BIGINT)
LANGUAGE C AS 'MODULE_PATHNAME', 'pg_tviews_debug_queue_wrapper';
```

```rust
// src/queue/ops.rs
#[pg_extern]
fn pg_tviews_debug_queue() -> TableIterator<'static, (name!(entity, String), name!(pk, i64))> {
    TX_REFRESH_QUEUE.with(|q| {
        let queue = q.borrow();
        let items: Vec<_> = queue.iter()
            .map(|k| (k.entity.clone(), k.pk))
            .collect();
        TableIterator::new(items)
    })
}
```

### Monitor Logs

```sql
-- Enable detailed logging
SET log_min_messages = INFO;

-- Look for these messages:
-- Phase 6C: "TVIEW: Flushing N refresh requests at commit"
-- Phase 6D: "TVIEW: Processing iteration N: M refreshes"
-- Phase 6D: "TVIEW: Completed X refresh operations in Y iterations"
```

### Verify Topological Order

```rust
// Add test in Phase 6D:
#[pg_test]
fn test_graph_order() {
    let graph = EntityDepGraph::load()?;
    // company should come before user
    let company_idx = graph.topo_order.iter().position(|e| e == "company").unwrap();
    let user_idx = graph.topo_order.iter().position(|e| e == "user").unwrap();
    assert!(company_idx < user_idx);
}
```

---

## 📚 Key Files to Modify

| File | Phase | Purpose |
|------|-------|---------|
| `src/queue/mod.rs` | 6A | Module structure |
| `src/queue/key.rs` | 6A | RefreshKey type |
| `src/queue/state.rs` | 6A | Thread-local storage |
| `src/queue/ops.rs` | 6A | Enqueue/dequeue |
| `src/queue/xact.rs` | 6C | Transaction callbacks |
| `src/queue/graph.rs` | 6D | Dependency graph |
| `src/trigger.rs` | 6B | Convert to enqueue |
| `src/catalog.rs` | 6B | entity_for_table() |
| `src/propagate.rs` | 6D | find_parents_for() |
| `src/error.rs` | 6D | New error types |
| `src/lib.rs` | 6A | Export queue module |

---

## 🎯 PRD Requirements Coverage

| Requirement | Phase | Status |
|-------------|-------|--------|
| R1: Refresh coalescing | 6A | HashSet dedup |
| R2: End-of-transaction | 6C | PRE_COMMIT callback |
| R3: Dependency order | 6D | Topological sort |
| R4: Propagation coalescing | 6D | Local pending queue |
| R5: No extra round trips | All | All in DB |

---

## ⏱️ Time Estimates

| Phase | Estimated | Actual | Notes |
|-------|-----------|--------|-------|
| 6A | 2-3 days | ___ | Foundation |
| 6B | 1-2 days | ___ | Triggers |
| 6C | 3-4 days | ___ | Callbacks |
| 6D | 1-2 days | ___ | Graph |
| **Total** | **7-11 days** | ___ | |

---

## ✅ Definition of Done

### Phase 6A
- All unit tests pass (6 expected)
- Clippy clean (0 warnings)
- Queue operations work correctly
- Documentation complete

### Phase 6B
- Triggers compile
- enqueue_refresh() called on every write
- entity_for_table() works correctly
- TVIEWs remain stale (expected)

### Phase 6C
- Transaction callbacks registered
- Queue flushed at commit
- Queue cleared on abort
- **Known limitation documented**: No dependency ordering

### Phase 6D
- Topological sort working
- Propagation integrated
- All 5 PRD requirements met
- **PRODUCTION READY**

---

## 🐛 Known Limitations (Post-Phase 6)

See `phase-6-known-limitations.md` for details:

1. **Savepoint rollback** - Queue not cleaned (workaround: use ROLLBACK)
2. **Prepared transactions** - Not supported (workaround: don't use 2PC)
3. **Deep chains (>100)** - Hardcoded limit (workaround: refactor schema)
4. **FK cascade timing** - Edge case (workaround: explicit deletes)
5. **Large queues** - Memory overhead (mitigation: batch refresh)

---

## 📞 When to Ask for Help

- Unexpected pgrx compiler errors (check pgrx version: 0.12.8)
- Transaction callbacks not firing (check _PG_init())
- Propagation not working (check local queue logic)
- Tests failing after Phase 6D (check graph.sort_keys())

---

**Quick Start:** Read `phase-6-transaction-queue-architecture.md` → Implement 6A → 6B → 6C → 6D in order.

**Emergency Rollback:** Phase 1-5 immediate refresh mode is fully functional fallback.
