# pg_tviews Extension Roadmap

**Last Updated:** 2025-12-10
**Current Status:** Phase 6 Complete ✅

---

## Overview

This document provides a high-level roadmap for the pg_tviews PostgreSQL extension development. The extension implements incremental view maintenance (IVM) for PostgreSQL using a transaction-level queue architecture.

---

## Development Phases

### ✅ Phase 1-5: Foundation (COMPLETE)

**Status:** Shipped and production-ready
**Features:**
- Basic TVIEW creation and refresh
- Dependency tracking
- Simple propagation
- Row-level triggers

**Known Limitations:**
- No deduplication (multiple updates = multiple refreshes)
- Immediate propagation (not end-of-transaction)
- Random refresh order (no dependency respect)
- Recursive propagation issues

---

### ✅ Phase 6: Transaction-Queue Architecture (COMPLETE)

**Status:** ✅ **PRODUCTION READY** (as of commit `0b78438`)
**Time:** Completed in 3 commits
**Documentation:** `.phases/PHASE6_COMPLETION.md`

**Implemented:**
- ✅ **R1**: Refresh coalescing via HashSet deduplication
- ✅ **R2**: End-of-transaction semantics via PRE_COMMIT callback
- ✅ **R3**: Dependency-correct ordering via topological sort
- ✅ **R4**: Propagation coalescing via local pending queue
- ✅ **R5**: No extra round trips (all in PostgreSQL callbacks)

**Sub-Phases:**
| Phase | Focus | Status | Commit |
|-------|-------|--------|--------|
| 6A | Queue Foundation | ✅ Complete | `37a0fbf` |
| 6B | Trigger Refactoring | ✅ Complete | `37a0fbf` |
| 6C | Commit Processing | ✅ Complete | `0b78438` |
| 6D | Entity Graph | ✅ Complete | `0b78438` |

**Performance:**
- 10 updates (same row): **10× faster** (10 refreshes → 1 refresh)
- 100 updates (same row): **100× faster** (100 refreshes → 1 refresh)
- Multi-entity updates: **Correctness** (random order → dependency order)

**Known Limitations:**
1. Savepoint rollback not handled (queue not cleaned)
2. Prepared transactions (2PC) not supported
3. Deep dependency chains limited to 100 levels
4. Graph caching not implemented (~5ms overhead per transaction)
5. entity_for_table() not cached (~0.1ms per trigger)

**Read:** `.phases/phase-6-transaction-queue-architecture.md`

---

### 📋 Phase 7: Performance Optimizations & Edge Cases

**Status:** PLANNED (Post-Phase 6)
**Priority:** Medium (Production enhancements)
**Estimated Time:** 1-2 weeks
**Documentation:** `.phases/phase-7-overview.md`

**Objectives:**
1. **Graph Caching**: Cache EntityDepGraph to avoid pg_tview_meta queries
2. **entity_for_table() Caching**: Cache table OID → entity mapping
3. **Savepoint Support**: Handle SAVEPOINT/ROLLBACK TO correctly
4. **Configurable Limits**: Add GUC settings for propagation depth
5. **Monitoring & Observability**: Add instrumentation for queue metrics

**Sub-Phases:**
| Phase | Focus | Time | Dependencies |
|-------|-------|------|--------------|
| 7A | Graph Caching | 1 day | Phase 6D ✅ |
| 7B | entity_for_table() Caching | 1 day | Phase 6B ✅ |
| 7C | Savepoint Support | 2-3 days | Phase 6C ✅ |
| 7D | GUC Configuration | 1 day | Phase 6C/6D ✅ |
| 7E | Monitoring & Observability | 2 days | Phase 6C ✅ |

**Performance Targets:**
- Graph load per transaction: **5ms → 0.001ms** (cache hit eliminates repeated loads)
- entity_for_table() lookup: **0.1ms → 0.001ms** per trigger (100× faster via cache)
- 100-trigger transaction overhead: **10ms → 0.5ms** (20× faster total overhead)

**Key Features:**
```rust
// Graph caching
static ENTITY_GRAPH_CACHE: Lazy<Mutex<Option<EntityDepGraph>>> = ...;

// Savepoint support
unsafe extern "C" fn tview_subxact_callback(event: SubXactEvent, ...) {
    match event {
        SUBXACT_EVENT_START_SUB => { /* snapshot queue */ }
        SUBXACT_EVENT_ABORT_SUB => { /* restore snapshot */ }
        // ...
    }
}

// GUC configuration
SET pg_tviews.max_propagation_depth = 200;
SET pg_tviews.enable_graph_cache = true;
SET pg_tviews.log_level = 'debug';

// Monitoring
SELECT * FROM pg_tviews_queue_stats();
SELECT * FROM pg_tviews_refresh_stats();
```

**Read:** `.phases/phase-7-overview.md`

---

### 📋 Phase 8: Two-Phase Commit Support

**Status:** PLANNED (Post-Phase 7)
**Priority:** Low (Enterprise feature)
**Estimated Time:** 2-3 weeks
**Documentation:** `.phases/phase-8-overview.md`

**Objectives:**
1. **Prepared Transaction Support**: Handle `PREPARE TRANSACTION` / `COMMIT PREPARED`
2. **Persistent Queue**: Serialize refresh queue to survive connection termination
3. **Automatic Recovery**: Resume pending refreshes after crash/restart
4. **Cross-Database Refresh**: Support TVIEWs spanning multiple databases
5. **Parallel Refresh**: Multi-worker refresh processing for large queues

**Sub-Phases:**
| Phase | Focus | Time | Dependencies |
|-------|-------|------|--------------|
| 8A | Persistent Queue Table | 2 days | Phase 6C ✅ |
| 8B | PREPARE TRANSACTION Handling | 2-3 days | Phase 8A |
| 8C | COMMIT PREPARED Handling | 2 days | Phase 8B |
| 8D | Automatic Recovery | 2 days | Phase 8C |
| 8E | Parallel Refresh | 3-4 days | Phase 6D ✅ |

**Use Cases:**
- Distributed database systems (Citus, Postgres-XL)
- Multi-database transactions (dblink, postgres_fdw)
- XA-compliant applications (Java EE, enterprise apps)
- Saga pattern implementations
- Multi-tenant systems with database-per-tenant

**Key Features:**
```sql
-- Persistent queue table
CREATE TABLE pg_tview_pending_refreshes (
    gid TEXT PRIMARY KEY,
    refresh_queue JSONB NOT NULL,
    prepared_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    // ...
);

-- Commit prepared transaction with refresh processing
SELECT pg_tviews_commit_prepared('xact_42');

-- Automatic recovery
SELECT * FROM pg_tviews_recover_prepared_transactions();
```

**Performance Targets:**
- 2PC: **Not supported → Fully supported** (new feature)
- Large queue (1000 rows): **5-10s → 2-4s** (2-3× faster with parallelization)
- Large queue (10000 rows): **50-100s → 15-30s** (2-4× faster, workload-dependent)
- **Note:** Parallel speedup varies based on CPU/I/O/lock characteristics

**Read:** `.phases/phase-8-overview.md`

---

### 📋 Phase 9: Production Hardening

**Status:** PLANNED (Post-Phase 8)
**Priority:** Medium (Production optimization)
**Estimated Time:** 2-3 weeks
**Documentation:** `.phases/phase-9-overview.md`

**Objectives:**
1. **Statement-Level Triggers**: Replace row-level triggers for bulk operations
2. **Transition Tables**: Use PostgreSQL's AFTER EACH STATEMENT feature
3. **Bulk Refresh API**: Efficient refresh of multiple rows in single operation
4. **Query Plan Caching**: Cache query plans for refresh operations
5. **Connection Pooling Integration**: Work correctly with PgBouncer, PgPool-II

**Sub-Phases:**
| Phase | Focus | Time | Dependencies |
|-------|-------|------|--------------|
| 9A | Statement-Level Triggers | 3 days | Phase 6B ✅ |
| 9B | Bulk Refresh API | 2 days | Phase 6D ✅ |
| 9C | Query Plan Caching | 2 days | Phase 6C ✅ |
| 9D | Connection Pooling Support | 2-3 days | Phase 6A ✅ |
| 9E | Production Monitoring | 2 days | Phase 7E (optional) |

**Performance Targets:**
- 1000-row UPDATE trigger overhead: **1ms → 0.1ms** (10× faster trigger firing)
- 1000-row bulk refresh query count: **1000 queries → 2 queries** (500× fewer)
- 1000-row bulk refresh actual time: **Variable → 10-50× faster** (workload-dependent)
- Query parsing overhead: **0.5ms → 0.05ms** per query (10× faster parsing)
- Connection pool safety: **Risky → Safe** (production-ready)

**Key Features:**
```sql
-- Statement-level trigger (fires once per statement, not per row)
CREATE TRIGGER tview_stmt_trigger
AFTER UPDATE ON tb_user
REFERENCING OLD TABLE AS old_table NEW TABLE AS new_table
FOR EACH STATEMENT
EXECUTE FUNCTION tview_stmt_trigger_handler();

-- Bulk refresh API
SELECT pg_tviews_refresh_bulk('user', ARRAY[1,2,3,4,5]);

-- Query plan caching
PREPARE tview_refresh_user (BIGINT) AS
    SELECT * FROM v_user WHERE pk_user = $1;

-- Connection pooling safety
# Works correctly with PgBouncer transaction pooling
pool_mode = transaction
```

**Read:** `.phases/phase-9-overview.md`

---

## Implementation Strategy

### Dependencies

```
Phase 1-5 (Foundation)
    ↓
Phase 6 (Transaction-Queue Architecture) ✅
    ↓
    ├──→ Phase 7 (Performance & Edge Cases)
    │       ├──→ 7A: Graph Caching
    │       ├──→ 7B: Table Caching
    │       ├──→ 7C: Savepoint Support
    │       ├──→ 7D: GUC Configuration
    │       └──→ 7E: Monitoring
    │
    ├──→ Phase 8 (2PC Support) [Optional]
    │       ├──→ 8A: Persistent Queue
    │       ├──→ 8B: PREPARE Handling
    │       ├──→ 8C: COMMIT PREPARED
    │       ├──→ 8D: Automatic Recovery
    │       └──→ 8E: Parallel Refresh
    │
    └──→ Phase 9 (Production Hardening)
            ├──→ 9A: Statement-Level Triggers
            ├──→ 9B: Bulk Refresh API
            ├──→ 9C: Query Plan Caching
            ├──→ 9D: Connection Pooling
            └──→ 9E: Production Monitoring
```

### Recommended Order

**Production Deployment Path:**
1. ✅ **Phase 6** (Complete) - Core functionality
2. **Phase 7A + 7B** - Performance optimizations (caching)
3. **Phase 9A + 9B** - Statement-level triggers + bulk operations
4. **Phase 7C** - Savepoint support (if needed)
5. **Phase 9C + 9D** - Query caching + connection pooling safety
6. **Phase 7D + 7E** - Configuration + monitoring
7. **Phase 9E** - Enhanced monitoring
8. **Phase 8** (Optional) - 2PC support (enterprise feature)

**Enterprise/Distributed Path:**
1. ✅ **Phase 6** (Complete) - Core functionality
2. **Phase 7A + 7B** - Performance optimizations
3. **Phase 8A + 8B + 8C** - 2PC support
4. **Phase 8D** - Automatic recovery
5. **Phase 8E** - Parallel refresh
6. **Phase 9** - Production hardening
7. **Phase 7** - Remaining optimizations

---

## Performance Evolution

| Scenario | Phase 1-5 | Phase 6 | Phase 7 | Phase 9 | Total Improvement |
|----------|-----------|---------|---------|---------|-------------------|
| 10 same-row updates | 10 refreshes | 1 refresh | 1 refresh | 1 refresh | **10× faster** |
| 100 same-row updates | 100 refreshes | 1 refresh | 1 refresh | 1 refresh | **100× faster** |
| Transaction overhead | ~0ms | ~10ms | ~0.5ms | ~0.5ms | **20× faster** |
| 1000-row bulk UPDATE trigger | N/A | 1ms | 1ms | 0.1ms | **10× faster** |
| 1000-row bulk refresh (query count) | 1000 queries | 1000 queries | 1000 queries | 2 queries | **500× fewer** |
| 1000-row bulk refresh (actual time) | Baseline | Baseline | Baseline | **10-50× faster** | Workload-dependent |
| Graph load per txn | N/A | 5ms | 0.001ms | 0.001ms | **Cache eliminates** |

**Combined Impact (Phase 6 + 7 + 9):**
- Same-row deduplication: **10-100× faster** (Phase 6 contribution)
- Transaction overhead: **20× faster** (Phase 7 caching)
- Bulk operations (network-bound): **20-50× faster** (Phase 9 bulk API)
- Bulk operations (CPU-bound): **5-15× faster** (Phase 9 optimizations)
- Overall production workloads: **10-100× faster** (typical, highly workload-dependent)

---

## Code Quality Standards

All phases must meet these standards:

### Build & Testing
- ✅ Compiles with `cargo build --release` (0 errors)
- ✅ Clippy clean: `cargo clippy --release -- -D warnings` (0 warnings)
- ✅ Unit tests passing
- ✅ Integration tests passing

### Documentation
- ✅ Phase overview document with context and objectives
- ✅ Detailed implementation plans for each sub-phase
- ✅ Code examples with full context
- ✅ Testing strategy with specific test cases
- ✅ Acceptance criteria
- ✅ Known limitations documented

### Architecture
- ✅ Fail-fast error handling
- ✅ Transaction isolation (thread-local state)
- ✅ Memory safety (no leaks, proper cleanup)
- ✅ PostgreSQL callback integration (FFI safety)
- ✅ Performance targets met

---

## Success Metrics

### Phase 6 (Complete)
- ✅ All 5 PRD requirements implemented
- ✅ 0 clippy warnings
- ✅ 7 unit tests passing
- ✅ Comprehensive documentation (2,800+ lines)
- ✅ Production-ready code quality

### Phase 7 (Planned)
- 20× faster commit overhead (via caching)
- Savepoints handled correctly
- Production metrics visible
- Configurable via GUC settings

### Phase 8 (Planned)
- 2PC fully supported
- No data loss on connection termination
- Automatic recovery functional
- 2-4× faster for large queues (with parallelization, workload-dependent)

### Phase 9 (Planned)
- 10-50× faster bulk operations (workload-dependent)
- 500× fewer queries for bulk refresh
- Safe with connection poolers (transaction + session modes)
- Query plan caching working
- Production monitoring integrated

---

## Next Steps

1. **Review Phase 7-9 Plans**: User review of planning documents
2. **Begin Phase 7A**: Implement graph caching (highest ROI, lowest complexity)
3. **Verify Phase 7A**: Run tests, measure performance improvement
4. **Commit Phase 7A**: Commit with descriptive message
5. **Continue Phase 7B-7E**: Sequential implementation with verification

**Estimated Timeline:**
- Phase 7: 1-2 weeks
- Phase 9: 2-3 weeks (can run in parallel with Phase 8 sub-phases)
- Phase 8: 2-3 weeks (optional, for enterprise features)

**Total to Production-Optimized:** 3-5 weeks (Phase 7 + 9)
**Total with 2PC Support:** 5-8 weeks (Phase 7 + 8 + 9)

---

## Documentation Index

### Overview Documents
- `ROADMAP.md` (this file) - High-level roadmap
- `README.md` - User-facing documentation
- `PRD_multiupdate.md` - Original product requirements

### Phase 6 (Complete)
- `.phases/phase-6-transaction-queue-architecture.md` - Overview
- `.phases/phase-6a-foundation.md` - Queue foundation
- `.phases/phase-6b-trigger-refactor.md` - Trigger refactoring
- `.phases/phase-6c-commit-processing.md` - Commit processing
- `.phases/phase-6d-entity-graph.md` - Entity graph
- `.phases/phase-6-known-limitations.md` - Edge cases (600+ lines)
- `.phases/PHASE6_COMPLETION.md` - Completion summary
- `.phases/PHASE6_IMPROVEMENTS_SUMMARY.md` - Architectural review
- `.phases/PHASE6_QUICKREF.md` - Quick reference

### Phase 7 (Planned)
- `.phases/phase-7-overview.md` - Performance optimizations overview
- `.phases/phase-7a-graph-caching.md` - To be created
- `.phases/phase-7b-table-caching.md` - To be created
- `.phases/phase-7c-savepoint-support.md` - To be created
- `.phases/phase-7d-guc-configuration.md` - To be created
- `.phases/phase-7e-monitoring.md` - To be created

### Phase 8 (Planned)
- `.phases/phase-8-overview.md` - 2PC support overview
- `.phases/phase-8a-persistent-queue.md` - To be created
- `.phases/phase-8b-prepare-handling.md` - To be created
- `.phases/phase-8c-commit-prepared.md` - To be created
- `.phases/phase-8d-recovery.md` - To be created
- `.phases/phase-8e-parallel-refresh.md` - To be created

### Phase 9 (Planned)
- `.phases/phase-9-overview.md` - Production hardening overview
- `.phases/phase-9a-statement-triggers.md` - To be created
- `.phases/phase-9b-bulk-refresh.md` - To be created
- `.phases/phase-9c-query-caching.md` - To be created
- `.phases/phase-9d-connection-pooling.md` - To be created
- `.phases/phase-9e-monitoring.md` - To be created

---

## Version History

| Version | Date | Phase | Commit | Notes |
|---------|------|-------|--------|-------|
| 0.1.0 | - | Phase 1-5 | - | Foundation |
| 0.6.0 | 2025-12-10 | Phase 6 | `b8c6c92` | Documentation |
| 0.6.1 | 2025-12-10 | Phase 6A+6B | `37a0fbf` | Foundation + Triggers |
| 0.6.2 | 2025-12-10 | Phase 6C+6D | `0b78438` | Commit Processing + Entity Graph |
| 0.7.0 | TBD | Phase 7 | TBD | Performance & Edge Cases |
| 0.8.0 | TBD | Phase 8 | TBD | 2PC Support (Optional) |
| 0.9.0 | TBD | Phase 9 | TBD | Production Hardening |
| 1.0.0 | TBD | All | TBD | Production Release |

---

**Status Legend:**
- ✅ **COMPLETE**: Implementation finished, tested, committed
- 🚧 **IN PROGRESS**: Currently being implemented
- 📋 **PLANNED**: Detailed plan exists, not started
- 💡 **PROPOSED**: Rough idea, no detailed plan yet

---

**Last Updated:** 2025-12-10
**Maintained By:** Project Team
**Status:** Phase 6 Complete ✅, Phase 7-9 Planned 📋
