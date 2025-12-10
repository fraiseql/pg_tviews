# PRD_multiupdate.md Impact Analysis

**Date:** 2025-12-10
**Analysis By:** Claude (Architecture Review)
**Context:** Phase 5 Remediation Complete - Assessing Impact on Multi-Update Transaction Plans

---

## Executive Summary

The Phase 5 remediation work revealed that **the current pg_tviews implementation does NOT have the transaction-level coalescing architecture described in `PRD_multiupdate.md`**.

**Current State:** Immediate refresh on every trigger (Phase 1-4 complete)
**PRD Target:** Transaction-level queue with commit-time refresh (not implemented)
**Impact:** **HIGH** - Core architecture change needed for PRD compliance

---

## 1. Current Implementation Analysis

### 1.1 What IS Implemented (Phases 1-4)

**✅ Phase 1-4: Immediate Refresh Architecture**

```
Trigger fires → pg_tviews_cascade() → refresh_pk() → propagate_from_row()
                      ↓                     ↓                  ↓
              Immediate execution   Recompute now      Cascade now
```

**Key characteristics:**
- **Triggers call functions directly** (`src/trigger.rs:29` → `pg_tviews_cascade`)
- **No transaction queue** - no `TX_REFRESH_QUEUE` or commit callbacks
- **Immediate refresh** - each trigger fires independently
- **No coalescing** - same (entity, pk) can be refreshed multiple times in one transaction

**Code evidence:**
```rust
// src/trigger.rs:6-33
#[pg_trigger]
fn pg_tview_trigger_handler<'a>(trigger: &'a PgTrigger<'a>) -> ... {
    // Extract table and PK
    let table_oid = trigger.relation()?.oid();
    let pk_value = crate::utils::extract_pk(trigger)?;

    // IMMEDIATE CALL - no queuing
    crate::pg_tviews_cascade(table_oid, pk_value);

    Ok(None)
}
```

```rust
// src/lib.rs:147-190
#[pg_extern]
fn pg_tviews_cascade(base_table_oid: pg_sys::Oid, pk_value: i64) {
    let dependent_tviews = find_dependent_tviews(base_table_oid)?;

    // IMMEDIATE REFRESH - happens NOW, not at commit
    for tview_meta in dependent_tviews {
        let affected_rows = find_affected_tview_rows(...)?;
        for affected_pk in affected_rows {
            refresh::refresh_pk(tview_meta.view_oid, affected_pk)?;
        }
    }
}
```

**Propagation (src/propagate.rs:13-55):**
- Works correctly: finds parents, cascades to them
- But: happens immediately, not deferred to commit
- Has batch optimization (≥10 rows) but no transaction-level dedup

### 1.2 What is NOT Implemented (PRD Requirements)

**❌ Missing: Transaction-Level Queue (PRD Section 3.1)**

```rust
// PRD expects this - NOT IN CODEBASE:
thread_local! {
    static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = ...;
    static TX_REFRESH_SCHEDULED: RefCell<bool> = ...;
}
```

**Search results:**
```bash
$ rg "TX_REFRESH|thread_local|RefCell<HashSet" src/
# NO MATCHES
```

**❌ Missing: Commit Callback (PRD Section 3.2)**

```rust
// PRD expects this - NOT IN CODEBASE:
fn register_commit_callback() -> spi::Result<()> {
    crate::xact::register_commit_callback(tx_commit_handler);
}

fn tx_commit_handler() -> spi::Result<()> {
    // Process queue at commit time
}
```

**Search results:**
```bash
$ rg "commit_callback|xact_callback|XactEvent" src/
# NO MATCHES
```

**❌ Missing: Enqueue-Only Triggers (PRD Section 5)**

Current triggers call `pg_tviews_cascade()` directly.
PRD expects: `enqueue_refresh_from_trigger()` → defer to commit.

**❌ Missing: RefreshKey Struct (PRD Section 4.1)**

```rust
// PRD expects:
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefreshKey {
    pub entity: String,
    pub pk: i64,
}
```

Current implementation: No unified key type, uses (view_oid, pk) pairs inconsistently.

---

## 2. Gap Analysis

### 2.1 Requirements Coverage

| PRD Requirement | Status | Notes |
|-----------------|--------|-------|
| **R1: Refresh coalescing** | ❌ **NOT MET** | Same (entity, pk) refreshed multiple times |
| **R2: End-of-transaction semantics** | ❌ **NOT MET** | Refreshes happen during transaction, not at commit |
| **R3: Dependency-correct order** | ✅ **PARTIAL** | Propagation respects dependencies but immediate, not batched |
| **R4: Propagation coalescing** | ❌ **NOT MET** | No transaction-level dedup |
| **R5: No extra read-round trips** | ✅ **MET** | All logic in DB |

**Score: 1.5/5 requirements fully met**

### 2.2 Architecture Divergence

**PRD Architecture:**
```
Trigger → Enqueue → [Transaction Queue] → Commit → Flush Queue
              ↓                                         ↓
        Dedup in HashSet                    Refresh each (entity,pk) once
                                                   ↓
                                            Propagate (also enqueued)
```

**Current Architecture:**
```
Trigger → Immediate Refresh → Propagate → Immediate Refresh (recursive)
              ↓                     ↓              ↓
         No dedup          Cascade now      Cascade now
```

**Key Differences:**

1. **Timing:** Immediate vs. commit-time
2. **Deduplication:** None vs. HashSet coalescing
3. **Transaction Safety:** Runs during writes vs. after all writes
4. **Performance:** N refreshes vs. 1 per (entity,pk)

---

## 3. Phase 5 Impact Assessment

### 3.1 What Phase 5 Was Supposed to Deliver

**From commit a354b47 claims:**
- "Array handling and performance optimization - **COMPLETED**"
- "Full array INSERT/DELETE support with automatic type inference"
- "2.03× performance improvement with smart JSONB patching"

**What Phase 5 Actually Delivered (per remediation):**
- ✅ Documentation (ARRAYS.md, README, CHANGELOG)
- ✅ Test suite (RED phase - tests written)
- ✅ Performance benchmarking infrastructure (docs/PERFORMANCE_RESULTS.md)
- ❌ Array handling implementation (tests fail - missing `pg_tview_trigger_handler_wrapper`)
- ❌ Transaction queue architecture (never started)

**Impact on PRD:**
- **Array handling** (partial Phase 5 goal) is **tangential** to PRD
- **Transaction queue** (PRD core) is **not addressed** by Phase 5 at all
- Phase 5 work does **not move toward PRD architecture**

### 3.2 Alignment with PRD Goals

**PRD Focus Areas:**

1. **Multi-update coalescing** → Phase 5: ❌ Not addressed
2. **Commit-time refresh** → Phase 5: ❌ Not addressed
3. **Transaction-level queue** → Phase 5: ❌ Not addressed
4. **Smart JSONB patching** → Phase 5: ✅ Performance work started (but unrelated to coalescing)

**Verdict:** Phase 5 work is **orthogonal** to PRD goals. Smart patching is an optimization that applies AFTER the queue architecture is in place.

---

## 4. Critical Issues for PRD Implementation

### 4.1 Architectural Mismatch

**Problem:** Current design is fundamentally incompatible with PRD.

**Why:**
- Immediate refresh means multiple updates to same row in transaction
- No way to defer work until commit without rewriting trigger layer
- Propagation happens recursively during trigger execution (can't be batched)

**Solution Required:**
- Complete rewrite of trigger → refresh → propagate flow
- Add transaction-local state management
- Implement commit callbacks

**Estimated Effort:** **Major refactoring** (several days)

### 4.2 Missing PostgreSQL Infrastructure

**Problem:** No transaction callback mechanism in current code.

**pgrx Support for xact callbacks:**
- pgrx 0.12.8 **does support** transaction callbacks
- Available via: `pgrx::hooks::register_xact_callback()`
- **Not currently used** in pg_tviews

**Implementation Path:**
```rust
use pgrx::pg_sys::{XactEvent, XactCallback};

unsafe extern "C" fn commit_callback(event: XactEvent, _arg: void) {
    if event == XactEvent::XACT_EVENT_COMMIT {
        // Flush TX_REFRESH_QUEUE here
        if let Err(e) = flush_refresh_queue() {
            error!("Failed to flush refresh queue: {:?}", e);
        }
    }
}

// In extension initialization:
register_xact_callback(commit_callback, std::ptr::null_mut());
```

**Estimated Effort:** 1-2 days to implement and test

### 4.3 Metadata Schema Limitations

**PRD Needs (Section 4.3):**
```rust
pub struct EntityDepGraph {
    pub parents: HashMap<String, Vec<String>>,
    pub children: HashMap<String, Vec<String>>,
    pub topo_order: Vec<String>,
}
```

**Current Metadata (src/metadata.rs, pg_tview_meta table):**
- ✅ Has `dependencies` (base table OIDs)
- ✅ Has `fk_columns`, `uuid_fk_columns`
- ✅ Has `dependency_types`, `dependency_paths`
- ❌ Does **NOT** have entity-to-entity dependency graph
- ❌ Does **NOT** have topological order precomputed

**Current approach:** Query on-the-fly using `find_parent_entities()` (propagate.rs:61-71)
**PRD approach:** Precomputed DAG with entity names and topo sort

**Gap:** Need to build `EntityDepGraph` from existing `pg_tview_meta` data.

**Estimated Effort:** 1 day to compute and cache entity graph

---

## 5. Implementation Roadmap to PRD Compliance

### Phase A: Foundation (Required Before PRD)

**Goal:** Add infrastructure for transaction-level queue

**Tasks:**
1. **Add RefreshKey type** (PRD 4.1)
   - File: `src/refresh/queue.rs` (new)
   - Define `RefreshKey { entity: String, pk: i64 }`
   - Implement `Hash`, `Eq` for dedup

2. **Add transaction-local state** (PRD 4.2)
   - File: `src/refresh/queue.rs`
   - Thread-local `TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>>`
   - Thread-local `TX_REFRESH_SCHEDULED: RefCell<bool>`

3. **Register xact callback** (PRD 6.2)
   - File: `src/hooks.rs` (new or extend existing)
   - Use pgrx `register_xact_callback()`
   - Implement `tx_commit_handler()`

**Estimated Time:** 2-3 days
**Complexity:** Medium (new concepts but clear path)

### Phase B: Refactor Triggers (Breaking Change)

**Goal:** Change triggers from immediate refresh to enqueue-only

**Tasks:**
1. **Modify trigger handler** (PRD 5.1-5.2)
   - File: `src/trigger.rs`
   - Change from `pg_tviews_cascade()` call to `enqueue_refresh(entity, pk)`
   - Remove all immediate refresh logic

2. **Implement enqueue function** (PRD 6.1)
   - File: `src/refresh/queue.rs`
   - `enqueue_refresh_from_trigger(entity: &str, pk: i64)`
   - Insert into `TX_REFRESH_QUEUE`
   - Register callback if not already scheduled

**Estimated Time:** 1-2 days
**Complexity:** Low (straightforward refactor)

**BREAKING CHANGE:** Behavior changes from immediate to deferred refresh

### Phase C: Commit-Time Processing (Core PRD Logic)

**Goal:** Implement flush logic at commit time

**Tasks:**
1. **Implement commit handler** (PRD 6.2)
   - File: `src/refresh/queue.rs`
   - `tx_commit_handler()` - snapshot queue, clear it
   - Loop: pop from queue, refresh, propagate parents back to queue
   - Process until queue empty (closure)

2. **Update refresh logic** (PRD 7)
   - File: `src/refresh/main.rs`
   - Adapt `refresh_pk()` to work with entity names
   - Ensure it doesn't re-enqueue (already in processed set)

3. **Update propagation logic** (PRD 8)
   - File: `src/propagate.rs`
   - Change from immediate refresh to enqueue
   - `parents_for(key: &RefreshKey) -> Vec<RefreshKey>`
   - Return keys instead of calling refresh directly

**Estimated Time:** 3-4 days
**Complexity:** High (core logic, needs careful testing)

### Phase D: Entity Dependency Graph (Optimization)

**Goal:** Precompute entity-to-entity dependencies for topological ordering

**Tasks:**
1. **Build EntityDepGraph** (PRD 4.3)
   - File: `src/catalog.rs` or `src/dependency/graph.rs`
   - Query `pg_tview_meta` to build parent/child relationships
   - Compute topological order
   - Cache in static or lazy_static

2. **Use topo order in commit handler**
   - Process entities in dependency order (optional optimization)
   - Reduces wasted work from propagation

**Estimated Time:** 1-2 days
**Complexity:** Medium (graph algorithms)

**Note:** This is an optimization. The iterative queue approach (Phase C) works without it.

---

## 6. Testing Strategy for PRD Implementation

### 6.1 Test Scenarios from PRD

**Scenario 1: Multi-update coalescing (R1)**
```sql
BEGIN;
  UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;  -- Affects tv_post[10,20,30]
  UPDATE tb_user SET email = 'alice@example.com' WHERE pk_user = 1;  -- Same rows
COMMIT;

-- Expected: tv_post[10,20,30] refreshed ONCE per row (3 total)
-- Current: tv_post[10,20,30] refreshed TWICE per row (6 total)
```

**Scenario 2: End-of-transaction semantics (R2)**
```sql
BEGIN;
  UPDATE tb_post SET title = 'New Title' WHERE pk_post = 1;
  -- At this point, tv_post[1] should NOT be refreshed yet
  SELECT data FROM tv_post WHERE pk_post = 1;
  -- Should see OLD data (before refresh)
COMMIT;
-- NOW tv_post[1] is refreshed with 'New Title'
```

**Current Behavior:** Refresh happens DURING transaction, so SELECT sees new data immediately.

**Scenario 3: Dependency order (R3)**
```sql
-- tv_feed depends on tv_post depends on tv_user

BEGIN;
  UPDATE tb_user SET name = 'Bob' WHERE pk_user = 1;
  -- Changes cascade: tv_user[1] → tv_post[X,Y,Z] → tv_feed[A,B]
COMMIT;

-- Expected order:
--   1. Refresh tv_user[1]
--   2. Refresh tv_post[X,Y,Z]
--   3. Refresh tv_feed[A,B]
-- Each row refreshed exactly once, even if dependencies overlap
```

### 6.2 Regression Testing

**After implementing PRD architecture:**

1. **Phase 1-4 Tests:** All existing tests should still pass
   - Behavior changes but results should be same
   - Immediate vs. deferred is transparent to queries after COMMIT

2. **Phase 5 Array Tests (50-52):** Will still fail (unrelated to PRD)
   - Array handling implementation still needed (separate work)

3. **Performance Tests:** Should see BETTER performance
   - Fewer redundant refreshes
   - Better cache locality (batch processing)

---

## 7. Recommendations

### 7.1 Immediate Actions (Priority Order)

**1. Acknowledge Architectural Gap** ✅ (This document)
   - Current implementation does NOT match PRD
   - Phase 5 work is tangential
   - Major refactoring needed

**2. Decide on PRD Priority**
   - **Option A:** Implement PRD now (Phases A-D, 7-11 days)
   - **Option B:** Defer PRD, focus on array handling (Phase 5 Task 7)
   - **Option C:** Parallel tracks (PRD + arrays, requires coordination)

**3. Update Project Roadmap**
   - Add "Phase 6: Transaction Queue Architecture (PRD)"
   - Mark as REQUIRED for production multi-update workloads
   - Estimated: 2-3 weeks with testing

### 7.2 Risk Assessment

**If PRD NOT implemented:**

**Risk 1: Performance Degradation in Multi-Update Transactions**
- **Severity:** HIGH
- **Likelihood:** HIGH (common pattern in real applications)
- **Impact:** Same TVIEW row refreshed N times instead of once
- **Example:** Batch import of 100 users → 100 refreshes of each dependent tv_post row

**Risk 2: Race Conditions / Inconsistency**
- **Severity:** MEDIUM
- **Likelihood:** MEDIUM
- **Impact:** Intermediate states visible within transaction
- **Example:** User sees partially updated data before COMMIT completes

**Risk 3: Trigger Overhead**
- **Severity:** MEDIUM
- **Likelihood:** HIGH
- **Impact:** Each trigger fires full cascade (can't be batched)
- **Example:** Bulk insert of 1000 rows triggers 1000 separate cascades

**If PRD IS implemented:**

**Benefit 1: Optimal Refresh Count**
- Each (entity, pk) refreshed exactly once per transaction
- **Estimated Speedup:** 2-10× for multi-update workloads

**Benefit 2: Transactional Consistency**
- All refreshes happen atomically at COMMIT
- No intermediate states visible

**Benefit 3: Better Performance Predictability**
- Queue size scales with unique (entity, pk) not trigger count
- Easier to reason about performance

### 7.3 Recommendation Summary

**Recommended Path:**

1. **Complete Phase 5 Task 7 FIRST** (array handling implementation)
   - Fixes immediate test failures
   - Smaller scope, less risk
   - Can be done independently

2. **Then Implement PRD (Phases A-D)** as Phase 6
   - Coordinate with Phase 5 completion
   - Full testing with both features
   - Becomes "Version 1.0" release candidate

**Rationale:**
- Phase 5 array work is partially done (tests written, docs complete)
- PRD is greenfield (no code conflicts)
- Sequential is safer than parallel for architecture changes
- Both are needed for production readiness

**Timeline:**
- Phase 5 Task 7 (arrays): 3-5 days
- PRD Implementation (Phase 6): 7-11 days
- Integration & Testing: 2-3 days
- **Total: 12-19 days to production-ready**

---

## 8. Conclusion

### Current State vs. PRD

| Aspect | Current | PRD Target | Gap |
|--------|---------|------------|-----|
| Refresh Timing | Immediate | Commit-time | **Major** |
| Coalescing | None | HashSet dedup | **Major** |
| Queue | None | Thread-local | **Major** |
| Callbacks | None | xact_callback | **Major** |
| Propagation | Immediate | Enqueued | **Major** |
| Entity Graph | On-demand | Precomputed | Minor |

**Overall Assessment:** **Major architectural changes required**

### Phase 5 Impact

Phase 5 work (documentation, array tests, performance infrastructure) is **valuable but orthogonal** to PRD goals. Smart JSONB patching can integrate with PRD architecture but doesn't substitute for transaction queue system.

### Next Steps

1. ✅ **Document gap** (this analysis)
2. ⚠️  **Decide priority** (PRD now vs. arrays first)
3. 📋 **Create Phase 6 plan** (PRD implementation)
4. 🔨 **Execute** (Phase 5 Task 7 + Phase 6)
5. ✅ **Verify** (tests, benchmarks, production scenarios)

---

**Document Version:** 1.0
**Last Updated:** 2025-12-10
**Status:** Ready for Review
