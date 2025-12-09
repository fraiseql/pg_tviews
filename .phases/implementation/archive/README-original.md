# pg_tviews Implementation Plan - TDD Approach

**Status:** Ready for Implementation
**Created:** 2025-12-09
**Methodology:** Test-Driven Development (RED → GREEN → REFACTOR)
**Target:** Simple agents can execute sequentially

---

## Overview

This directory contains detailed, TDD-based implementation plans for the pg_tviews PostgreSQL extension. Each phase is designed to be executed independently by simple agents with clear test cases, acceptance criteria, and rollback plans.

---

## Phase Structure

Each phase follows this structure:

```
Phase N: [Name]
├── Objective (what we're building)
├── Success Criteria (checklist)
├── TDD Tests (RED → GREEN → REFACTOR)
│   ├── Test 1: [Feature]
│   │   ├── RED Phase (failing test)
│   │   ├── GREEN Phase (minimal implementation)
│   │   └── REFACTOR Phase (optimize)
│   ├── Test 2: [Feature]
│   └── Test 3: [Edge Cases]
├── Implementation Steps
├── Acceptance Criteria
└── Rollback Plan
```

---

## Implementation Phases

| Phase | Name | Duration | Complexity | Status |
|-------|------|----------|------------|--------|
| **0** | Foundation & Project Setup | 1-2 days | Low | 📋 Ready |
| **1** | Schema Inference & Column Detection | 3-5 days | Medium | 📋 Ready |
| **2** | View & Table Creation | 5-7 days | High | 📋 Ready |
| **3** | Dependency Detection & Trigger Installation | 5-7 days | High | 📋 Ready |
| **4** | Refresh Logic & Cascade Propagation | 7-10 days | Very High | 📋 Ready |
| **5** | Array Handling & Performance Optimization | 5-7 days | High | 📋 Ready |

**Total Estimated Duration:** 26-38 days

---

## Phase 0: Foundation & Project Setup

**File:** `phase-0-foundation.md`

**Objective:** Establish Rust/pgrx project foundation with testing infrastructure.

**Key Deliverables:**
- Extension compiles with pgrx 0.12.8
- Extension loads into PostgreSQL 15+
- Metadata tables (`pg_tview_meta`, `pg_tview_helpers`) created
- Basic test infrastructure works

**Critical Tests:**
1. Extension loads successfully
2. Metadata tables created
3. Version function callable

**Dependencies:** Rust toolchain, pgrx, PostgreSQL 15+

---

## Phase 1: Schema Inference & Column Detection

**File:** `phase-1-schema-inference.md`

**Objective:** Parse SELECT statements to automatically detect columns and types.

**Key Deliverables:**
- `pg_tviews_analyze_select()` function
- Detects `pk_<entity>`, `id`, `identifier`, `data` columns
- Detects `fk_*` (lineage) and `*_id` (filtering) columns
- Type inference from PostgreSQL catalog

**Critical Tests:**
1. Simple column detection (pk, id, data)
2. Complex columns (FKs, arrays, flags)
3. Edge cases (missing columns, validation)
4. Type inference from catalog

**Dependencies:** Phase 0 complete

---

## Phase 2: View & Table Creation

**File:** `phase-2-view-and-table-creation.md`

**Objective:** Implement `CREATE TVIEW` SQL syntax with automatic DDL generation.

**Key Deliverables:**
- `CREATE TVIEW tv_<name> AS SELECT ...` syntax
- Backing view `v_<entity>` created
- Materialized table `tv_<entity>` with correct schema
- Initial data population
- `DROP TVIEW` cleanup

**Critical Tests:**
1. Basic TVIEW creation (minimal example)
2. TVIEW with foreign keys
3. DROP TVIEW cleanup

**Dependencies:** Phase 0 + Phase 1 complete

**Architecture Decision:** PostgreSQL hook integration for DDL interception.

---

## Phase 3: Dependency Detection & Trigger Installation

**File:** `phase-3-dependency-tracking.md`

**Objective:** Automatic dependency detection and trigger lifecycle management.

**Key Deliverables:**
- Walk `pg_depend` graph to find base tables
- Detect helper views used in SELECT
- Install AFTER triggers on all base tables
- Trigger handler function (logs only, no refresh yet)

**Critical Tests:**
1. Single table dependency detection
2. Transitive dependencies (helper views)
3. Trigger fires on base table change

**Dependencies:** Phase 0 + Phase 1 + Phase 2 complete

**Architecture Decision:** Recursive pg_depend walker with cycle detection.

---

## Phase 4: Refresh Logic & Cascade Propagation

**File:** `phase-4-refresh-and-cascade.md`

**Objective:** Core refresh and cascade logic with jsonb_ivm integration.

**Key Deliverables:**
- Row-level refresh (SELECT FROM v_*, UPDATE tv_*)
- jsonb_ivm integration (jsonb_smart_patch_scalar, jsonb_smart_patch_nested)
- FK lineage propagation
- Cascade to dependent TVIEWs

**Critical Tests:**
1. Single row refresh (no cascade)
2. jsonb_ivm integration (surgical updates)
3. FK lineage cascade (multi-row)

**Dependencies:** Phase 0-3 complete + jsonb_ivm extension installed

**Architecture Decision:** Surgical JSONB updates for 2-3× performance improvement.

**⚠️ MOST COMPLEX PHASE** - Requires careful testing and performance validation.

---

## Phase 5: Array Handling & Performance Optimization

**File:** `phase-5-arrays-and-optimization.md`

**Objective:** Production-ready features with array support and optimization.

**Key Deliverables:**
- Array columns (UUID[], TEXT[]) materialized
- JSONB array updates (jsonb_smart_patch_array)
- Array element INSERT/DELETE (jsonb_array_insert_where, jsonb_array_delete_where)
- Batch update optimization (>10 rows)
- Production monitoring and logging

**Critical Tests:**
1. Array column materialization
2. JSONB array element update
3. Array element INSERT/DELETE
4. Batch optimization (100 rows)

**Dependencies:** Phase 0-4 complete

**Performance Target:** 3-4× faster for batch updates, 3× for array operations.

---

## Testing Strategy

### Test Pyramid

```
           ┌─────────────┐
           │   E2E Tests │  (SQL integration tests)
           │  (~20 tests) │
           └─────────────┘
              ▲         ▲
             /           \
            /             \
     ┌──────────┐    ┌──────────┐
     │Integration│    │Performance│
     │  Tests    │    │Benchmarks │
     │(~30 tests)│    │ (~10)     │
     └──────────┘    └──────────┘
          ▲               ▲
         /                 \
        /                   \
 ┌──────────┐          ┌──────────┐
 │Rust Unit │          │SQL Unit  │
 │  Tests   │          │  Tests   │
 │(~50 tests)│          │(~40 tests)│
 └──────────┘          └──────────┘
```

### Test Categories

| Category | Count | Location | Purpose |
|----------|-------|----------|---------|
| **Rust Unit Tests** | ~50 | `src/**/*.rs` `#[cfg(test)]` | Test individual functions |
| **pgrx Tests** | ~30 | `src/**/*.rs` `#[pg_test]` | Test PostgreSQL integration |
| **SQL Integration** | ~40 | `test/sql/*.sql` | Test DDL and DML operations |
| **Performance Benchmarks** | ~10 | `bench/*.sql` | Validate 2-3× improvement |
| **E2E Tests** | ~20 | `test/e2e/*.sql` | Full lifecycle tests |

**Total Tests:** ~150 tests

---

## TDD Workflow

### For Each Feature

1. **RED Phase - Write Failing Test**
   - Write SQL test that demonstrates desired behavior
   - Run test → verify it fails with expected error
   - Document expected output

2. **GREEN Phase - Minimal Implementation**
   - Write minimum Rust code to pass the test
   - Prioritize simplicity over optimization
   - Run test → verify it passes

3. **REFACTOR Phase - Optimize**
   - Improve code quality without changing behavior
   - Add error handling, logging, validation
   - Add Rust unit tests
   - Run all tests → verify still passing

4. **COMMIT**
   - Git commit with test + implementation
   - Move to next feature

---

## Development Environment

### Prerequisites

```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# pgrx
cargo install --locked cargo-pgrx

# PostgreSQL 15-17
sudo apt-get install postgresql-17 postgresql-server-dev-17

# Initialize pgrx
cargo pgrx init
```

### Build & Test Commands

```bash
# Run Rust unit tests
cargo test

# Run pgrx tests (requires PostgreSQL)
cargo pgrx test pg17

# Install extension locally
cargo pgrx install --release

# Run SQL integration tests
psql -d test_db -f test/sql/00_extension_loading.sql
```

### CI/CD Pipeline

```yaml
# .github/workflows/ci.yml
- Rust unit tests (cargo test)
- pgrx tests (PostgreSQL 15, 16, 17)
- SQL integration tests
- Performance benchmarks
- Documentation generation
```

---

## Dependencies

### External Extensions

| Extension | Version | Purpose | Installation |
|-----------|---------|---------|--------------|
| **jsonb_ivm** | v0.3.0+ | Surgical JSONB updates | `CREATE EXTENSION jsonb_ivm` |
| **PostgreSQL** | 15-17 | Target database | Standard installation |

### Rust Crates

| Crate | Version | Purpose |
|-------|---------|---------|
| **pgrx** | 0.12.8 | PostgreSQL extension framework |
| **serde** | 1.0 | JSON serialization |
| **serde_json** | 1.0 | JSON parsing |
| **regex** | 1.0 | SQL parsing (v1 only) |

---

## Implementation Guidelines

### Code Style

- **Rust**: Follow `rustfmt` standard
- **SQL**: PostgreSQL style guide
- **Comments**: Explain "why", not "what"
- **Naming**: `snake_case` for Rust, SQL objects

### Error Handling

```rust
// Always use Result<T, Box<dyn std::error::Error>>
pub fn create_tview(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Validate input
    if name.is_empty() {
        return Err("TVIEW name cannot be empty".into());
    }

    // Use ? for error propagation
    let schema = infer_schema(name)?;

    // Log important operations
    info!("Creating TVIEW {}", name);

    Ok(())
}
```

### Performance Guidelines

- Target: 2-3× faster than native SQL
- Batch threshold: 10 rows
- Cascade depth limit: 10 levels
- Timeout: 30s per refresh operation

---

## Acceptance Criteria (Overall)

### Functional Requirements

- [ ] `CREATE TVIEW` syntax works
- [ ] Automatic view and table creation
- [ ] Dependency detection (transitive)
- [ ] Trigger installation (all base tables)
- [ ] Row-level refresh
- [ ] jsonb_ivm integration
- [ ] FK lineage cascade
- [ ] Array column support
- [ ] Batch optimization
- [ ] `DROP TVIEW` cleanup

### Quality Requirements

- [ ] All 150+ tests pass
- [ ] Code coverage > 80%
- [ ] No memory leaks (valgrind)
- [ ] Documentation complete
- [ ] CI/CD pipeline green

### Performance Requirements

- [ ] Single row refresh < 5ms
- [ ] 100-row cascade < 500ms
- [ ] jsonb_ivm 2-3× faster vs native SQL
- [ ] Batch updates 4× faster (100+ rows)
- [ ] Storage 88% smaller vs naive approach

---

## Risk Mitigation

### High-Risk Areas

1. **Phase 4 Complexity**
   - **Risk:** Most complex phase, many integration points
   - **Mitigation:** Break into smaller tests, extensive logging, rollback plan

2. **Performance Targets**
   - **Risk:** May not achieve 2-3× improvement
   - **Mitigation:** jsonb_ivm validated separately, benchmarks in each phase

3. **PostgreSQL Version Compatibility**
   - **Risk:** pgrx abstractions may differ across PG versions
   - **Mitigation:** CI tests on PG 15, 16, 17

4. **Dependency Detection Edge Cases**
   - **Risk:** Circular dependencies, complex views
   - **Mitigation:** Cycle detection, depth limits, clear error messages

---

## Success Metrics

### Technical Metrics

- ✅ 150+ tests passing
- ✅ 2-3× performance improvement (validated by benchmarks)
- ✅ 88% storage reduction (helper-aware materialization)
- ✅ Zero manual trigger/refresh code needed

### Developer Experience Metrics

- ✅ 83% less boilerplate (6 steps → 1 step)
- ✅ 50% schema simplification (70 views → 33 views)
- ✅ 100% automation (triggers, refresh, cascade)

### Production Readiness

- ✅ CI/CD pipeline operational
- ✅ Documentation complete
- ✅ Error handling comprehensive
- ✅ Monitoring and logging
- ✅ Rollback plans documented

---

## Next Steps After Implementation

1. **Integration Testing**
   - Test with PrintOptim backend schemas
   - Validate performance improvements
   - Stress test with large datasets

2. **Documentation**
   - User guide (how to use TVIEW)
   - Architecture documentation
   - Performance tuning guide
   - Migration guide from manual tv_*

3. **Production Deployment**
   - Staging environment testing
   - Performance profiling
   - Monitoring setup
   - Rollout plan

4. **Future Enhancements**
   - Async mode (background workers)
   - Schema change detection
   - Distributed support (Citus)
   - Advanced monitoring UI

---

## References

- **PRD v2.0:** `/home/lionel/code/pg_tviews/PRD_v2.md`
- **PRD Addendum:** `/home/lionel/code/pg_tviews/PRD_ADDENDUM.md`
- **Helper Optimization:** `/home/lionel/code/pg_tviews/HELPER_VIEW_OPTIMIZATION.md`
- **jsonb_ivm:** `https://github.com/fraiseql/jsonb_ivm`
- **pgrx:** `https://github.com/pgcentralfoundation/pgrx`

---

## Contact & Support

- **Author:** Claude Code (AI-assisted design)
- **Methodology:** Test-Driven Development (TDD)
- **Target Users:** Simple AI agents, human developers
- **License:** PostgreSQL License (same as extension)

---

**Ready to implement? Start with Phase 0!**

```bash
cd /home/lionel/code/pg_tviews/.phases/implementation
cat phase-0-foundation.md
```
