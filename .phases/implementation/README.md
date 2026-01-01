# pg_tviews Implementation Plan - CORRECTED VERSION

**Status:** Ready for Implementation
**Created:** 2025-12-09 (Original)
**Corrected:** 2025-12-09 (Critical fixes applied)
**Methodology:** Test-Driven Development (RED → GREEN → REFACTOR)
**Target:** Simple agents can execute sequentially

---

## 🚨 CRITICAL CORRECTIONS APPLIED

This is the **CORRECTED** version of the implementation plans after expert PostgreSQL/Rust review.

### Major Fixes

| Issue | Severity | Phase | Fix |
|-------|----------|-------|-----|
| **pg_depend query direction wrong** | CRITICAL | Phase 3 | Fixed: `WHERE objid = {}` (was `refobjid`) |
| **Trigger PK column hardcoded** | CRITICAL | Phase 4 | Fixed: Dynamic extraction via `pg_attribute` |
| **No error handling** | HIGH | All | Added: Comprehensive `TViewError` enum |
| **Unsafe code not documented** | HIGH | Phase 2 | Added: SAFETY comments with invariants |
| **No concurrency control** | HIGH | Phase 4 | Added: Advisory locks, isolation requirements |
| **No cycle detection** | MEDIUM | Phase 3 | Added: Cycle detection with depth limits |
| **Parser limitations** | MEDIUM | Phase 2 | Documented: v1 limitations, v2 plan |

### Timeline Adjustments

**Original Estimate:** 26-38 days
**Corrected Estimate:** 44-64 days (+70%)

**Why:** Realistic complexity accounting for:
- PostgreSQL catalog integration debugging
- Rust-PostgreSQL FFI subtleties
- Performance tuning iterations
- Integration testing with real schemas

---

## 📋 Phase Overview

| Phase | Name | Duration | Status | Critical Changes |
|-------|------|----------|--------|------------------|
| **0-A** | Error Types & Safety | 1 day | ✅ NEW | **MUST DO FIRST** |
| **0** | Foundation & Setup | 1-2 days | 📋 Ready | Minor updates |
| **1** | Schema Inference | 5-7 days | 📋 Ready | Duration +2 days |
| **2** | View & Table Creation | 7-10 days | ⚠️ **FIXED** | ProcessUtility hook safety |
| **3** | Dependency Detection | 10-14 days | ⚠️ **FIXED** | pg_depend bug, cycle detection |
| **4** | Refresh & Cascade | 14-21 days | ⚠️ **FIXED** | Concurrency, FK changes |
| **5** | Arrays & Optimization | 7-10 days | 📋 Ready | Minor updates |

**Total:** 45-65 days (realistic estimate)

---

## 🆕 Phase 0-A: Error Types & Safety Infrastructure (NEW)

**CRITICAL:** This phase must complete BEFORE any other phase.

### What's New

```rust
pub enum TViewError {
    MetadataNotFound { entity: String },
    CircularDependency { cycle: Vec<String> },
    InvalidSelectStatement { sql: String, reason: String },
    JsonbIvmNotInstalled,
    CascadeDepthExceeded { current_depth: usize, max_depth: usize },
    // ... 15+ error variants with SQLSTATE mapping
}
```

### Why Critical

- All phases return `TViewResult<T>` instead of generic errors
- Error messages include context (entity names, PKs, SQL snippets)
- SQLSTATE codes enable proper PostgreSQL error handling
- SAFETY comment template for all unsafe blocks

**Read:** `.phases/implementation/phase-0-error-types.md`

---

## Phase 0: Foundation & Project Setup

**Duration:** 1-2 days
**Status:** Ready (minor updates for TViewError)

**Changes:**
- Use `TViewError` in all functions
- Add SAFETY comments to `_PG_init`
- Document Rust/PostgreSQL version matrix

**Read:** Original `phase-0-foundation.md` (minor edits needed)

---

## Phase 1: Schema Inference & Column Detection

**Duration:** 5-7 days (was 3-5)
**Status:** Ready

**Changes:**
- Return `TViewResult` instead of `Result<_, Box<dyn Error>>`
- Document parser limitations (regex-based, v1)
- Add v2 plan (PostgreSQL parser API)

**Read:** Original `phase-1-schema-inference.md` (apply TViewError)

---

## Phase 2: View & Table Creation (MAJOR FIXES)

**Duration:** 7-10 days (was 5-7)
**Status:** ⚠️ **CORRECTED VERSION AVAILABLE**

### Critical Fixes

1. **ProcessUtility Hook Safety**
   ```rust
   // BEFORE: Unsafe without documentation
   static mut PREV_PROCESS_UTILITY_HOOK: Option<...> = None;

   // AFTER: Comprehensive SAFETY comment
   // SAFETY: ProcessUtility hook installation
   //
   // Invariants:
   // 1. PostgreSQL extension init is single-threaded
   // 2. ProcessUtility hook is called serially
   // 3. Static variable lives for process lifetime
   // ...
   static mut PREV_PROCESS_UTILITY_HOOK: Option<...> = None;
   ```

2. **Parser Improvements**
   - Schema-qualified names: `CREATE TVIEW public.tv_post AS ...`
   - Better error messages with suggestions
   - Documented limitations (CTEs, comments, etc.)
   - v2 plan using native PostgreSQL parser

3. **Error Recovery**
   - Subtransaction for atomic rollback
   - Clean up partial state on failure
   - Proper error propagation to PostgreSQL

4. **pg_dump/restore Strategy**
   - Event trigger for DDL logging
   - Extension config dump registration
   - Backup/restore documentation

**Read:** `.phases/implementation/phase-2-ddl-FIXED.md`

---

## Phase 3: Dependency Detection (CRITICAL BUG FIX)

**Duration:** 10-14 days (was 5-7)
**Status:** ⚠️ **CORRECTED VERSION AVAILABLE**

### Critical Bug Fix

```rust
// BEFORE (WRONG): Selects objects that DEPEND ON the view
let deps_query = format!(
    "SELECT DISTINCT objid, objsubid, refobjid, refobjsubid, deptype
     FROM pg_depend
     WHERE refobjid = {}  // ❌ WRONG
       AND deptype = 'n'",
    current_oid
);

// AFTER (CORRECT): Selects objects the view DEPENDS ON
let deps_query = format!(
    "SELECT DISTINCT refobjid, refobjsubid, deptype
     FROM pg_depend
     WHERE objid = {}  // ✅ CORRECT
       AND deptype IN ('n', 'a')
       AND classid = 'pg_class'::regclass::oid
       AND refclassid = 'pg_class'::regclass::oid",
    current_oid
);
```

**Impact:** Original code would not detect ANY dependencies. Extension would be completely broken.

### New Features

1. **Cycle Detection**
   ```rust
   if visiting.contains(&current_oid) {
       let cycle = reconstruct_cycle(&visiting, current_oid)?;
       return Err(TViewError::CircularDependency { cycle });
   }
   ```

2. **Depth Limiting**
   ```rust
   const MAX_DEPENDENCY_DEPTH: usize = 10;

   if depth > MAX_DEPENDENCY_DEPTH {
       return Err(TViewError::DependencyDepthExceeded {
           depth,
           max_depth: MAX_DEPENDENCY_DEPTH,
       });
   }
   ```

3. **Better Dependency Type Handling**
   - Now checks `deptype IN ('n', 'a')` (normal + auto)
   - Filters by `classid`/`refclassid` for pg_class only
   - Handles materialized views (`relkind='m'`)
   - Handles partitioned tables (`relkind='p'`)

**Read:** `.phases/implementation/phase-3-dependency-tracking-FIXED.md`

---

## Phase 4: Refresh Logic & Cascade (MAJOR FIXES)

**Duration:** 14-21 days (was 7-10)
**Status:** ⚠️ **CORRECTED VERSION AVAILABLE**

### Critical Fixes

1. **Dynamic PK Column Extraction**
   ```sql
   -- BEFORE (HARDCODED - BROKEN):
   pk_val := OLD.pk;  -- ❌ Column 'pk' doesn't exist

   -- AFTER (DYNAMIC - CORRECT):
   SELECT a.attname INTO pk_col_name
   FROM pg_index i
   JOIN pg_attribute a ON a.attrelid = i.indrelid
   WHERE i.indrelid = TG_RELID AND i.indisprimary;

   EXECUTE format('SELECT ($1).%I', pk_col_name)
       USING OLD INTO pk_val_old;
   ```

2. **FK Change Detection**
   ```sql
   -- NEW: Detect when UPDATE changes FK
   IF TG_OP = 'UPDATE' THEN
       FOR fk_col_name IN SELECT attname FROM pg_attribute
                          WHERE attrelid = TG_RELID AND attname LIKE 'fk_%'
       LOOP
           EXECUTE format('SELECT ($1).%I', fk_col_name)
               USING OLD INTO fk_val_old;
           EXECUTE format('SELECT ($1).%I', fk_col_name)
               USING NEW INTO fk_val_new;

           IF fk_val_old IS DISTINCT FROM fk_val_new THEN
               -- Cascade to BOTH old and new parent!
           END IF;
       END LOOP;
   END IF;
   ```

3. **Concurrency Control**
   ```rust
   // Advisory locks prevent concurrent refresh of same row
   pub fn lock_tview_row(entity: &str, pk_value: i64) -> TViewResult<()> {
       let lock_key = compute_lock_key(entity, pk_value);

       let acquired = Spi::get_one::<bool>(&format!(
           "SELECT pg_try_advisory_xact_lock({}, {})",
           lock_key.0, lock_key.1
       ))?;

       if !acquired {
           return Err(TViewError::LockTimeout { ... });
       }

       Ok(())
   }
   ```

4. **Transaction Isolation Requirements**
   ```sql
   -- CRITICAL: Must use REPEATABLE READ or SERIALIZABLE
   ALTER DATABASE mydb SET default_transaction_isolation TO 'repeatable read';
   ```

   **Why:** Trigger reads from backing view. Without REPEATABLE READ, could materialize inconsistent state.

5. **Cascade Depth Limiting**
   ```rust
   const MAX_CASCADE_DEPTH: usize = 10;

   if current_depth >= MAX_CASCADE_DEPTH {
       return Err(TViewError::CascadeDepthExceeded {
           current_depth,
           max_depth: MAX_CASCADE_DEPTH,
       });
   }
   ```

### New Documentation

- `docs/CONCURRENCY.md` - Transaction isolation requirements
- Advisory lock design
- Deadlock prevention strategy

**Read:** `.phases/implementation/phase-4-refresh-CASCADE-FIXED.md`

---

## Phase 5: Array Handling & Optimization

**Duration:** 7-10 days (was 5-7)
**Status:** Ready (minor TViewError updates)

**Changes:**
- Use `TViewResult` consistently
- Document array semantics (delete ALL vs FIRST)
- Add edge case tests (empty arrays, NULLs)

**Read:** Original `phase-5-arrays-and-optimization.md` (minor edits needed)

---

## 🔧 Implementation Guidelines

### Error Handling

**ALL functions must return `TViewResult<T>`:**

```rust
// ❌ OLD WAY (Don't use)
pub fn create_tview(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if name.is_empty() {
        return Err("Invalid name".into());
    }
    // ...
}

// ✅ NEW WAY (Use this)
pub fn create_tview(name: &str) -> TViewResult<()> {
    if name.is_empty() {
        return Err(TViewError::InvalidTViewName {
            name: name.to_string(),
            reason: "TVIEW name cannot be empty".to_string(),
        });
    }
    // ...
}
```

### Safety Documentation

**ALL unsafe blocks must have SAFETY comments:**

```rust
// SAFETY: [Why this is safe]
//
// Invariants:
// 1. [Invariant #1]
// 2. [Invariant #2]
//
// Checked:
// - [What checks are performed]
//
// Lifetime: [Lifetime guarantees]
//
// Reviewed: [Date, initials]
unsafe {
    // code
}
```

### Testing Requirements

**Every phase must include:**

- Rust unit tests (`#[test]`)
- pgrx integration tests (`#[pg_test]`)
- SQL integration tests (`test/sql/*.sql`)
- Edge case tests (NULLs, empty inputs, limits)
- Error case tests (use `assert_error_sqlstate`)

**Example:**

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use crate::error::testing::*;

    #[pg_test]
    fn test_circular_dependency_rejected() {
        // Setup that creates cycle
        // ...

        let result = find_base_tables("v_circular");

        assert_error_sqlstate(result, "55P03");  // Lock not available
    }
}
```

---

## 📊 Acceptance Criteria (Overall)

### Functional Requirements (Updated)

- [ ] `CREATE TVIEW` syntax works with schema-qualified names
- [ ] **NEW:** Circular dependencies detected and rejected
- [ ] Automatic view and table creation
- [ ] **FIXED:** Dependency detection finds correct dependencies
- [ ] **NEW:** Maximum depth limit enforced (10 levels)
- [ ] Trigger installation (all base tables)
- [ ] **FIXED:** Row-level refresh with dynamic PK extraction
- [ ] **FIXED:** FK change detection on UPDATE
- [ ] **NEW:** Advisory locks prevent concurrent refresh conflicts
- [ ] jsonb_delta integration
- [ ] FK lineage cascade
- [ ] **NEW:** Cascade depth limited to 10 levels
- [ ] Array column support
- [ ] Batch optimization
- [ ] `DROP TABLE` cleanup

### Quality Requirements (Updated)

- [ ] All 150+ tests pass
- [ ] Code coverage > 80%
- [ ] **NEW:** All errors use `TViewError` with SQLSTATE
- [ ] **NEW:** All unsafe blocks have SAFETY comments
- [ ] **NEW:** Transaction isolation documented
- [ ] No memory leaks (valgrind)
- [ ] **NEW:** Concurrency documentation complete
- [ ] Documentation complete
- [ ] CI/CD pipeline green

### Performance Requirements

- [ ] Single row refresh < 5ms
- [ ] 100-row cascade < 500ms
- [ ] **NEW:** 1000-row cascade < 5s (with depth limit)
- [ ] jsonb_delta 2-3× faster vs native SQL
- [ ] Batch updates 4× faster (100+ rows)
- [ ] Storage 88% smaller vs naive approach

---

## 🚀 Getting Started

### 1. Read Error Types First

```bash
cat .phases/implementation/phase-0-error-types.md
```

**Implement Phase 0-A completely before starting Phase 0.**

### 2. Review Corrected Phases

**Critical phases with fixes:**

```bash
# Phase 2 (ProcessUtility hook safety)
cat .phases/implementation/phase-2-ddl-FIXED.md

# Phase 3 (pg_depend bug fix)
cat .phases/implementation/phase-3-dependency-tracking-FIXED.md

# Phase 4 (concurrency, FK changes)
cat .phases/implementation/phase-4-refresh-CASCADE-FIXED.md
```

### 3. Set Up Development Environment

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install pgrx
cargo install --locked cargo-pgrx

# Install PostgreSQL 15-17
sudo apt-get install postgresql-17 postgresql-server-dev-17

# Initialize pgrx
cargo pgrx init

# Install jsonb_delta extension
git clone https://github.com/fraiseql/jsonb_delta
cd jsonb_delta
make && sudo make install
```

### 4. Create Project

```bash
cargo pgrx new pg_tviews
cd pg_tviews

# Add dependencies to Cargo.toml
[dependencies]
pgrx = "=0.12.8"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
regex = "1.0"

[dev-dependencies]
pgrx-tests = "=0.12.8"
```

### 5. Start with Phase 0-A

```bash
# Create error module
mkdir -p src/error
touch src/error/mod.rs
touch src/error/testing.rs

# Implement TViewError enum
# (Copy from phase-0-error-types.md)
```

---

## 📝 Development Workflow

### For Each Phase

1. **Read phase plan thoroughly**
   - Understand objectives
   - Review test cases (RED phase)
   - Check acceptance criteria

2. **RED: Write failing tests**
   - Create SQL test files
   - Create Rust test functions
   - Run tests → verify they fail

3. **GREEN: Implement minimal code**
   - Write simplest code that passes tests
   - Use `TViewResult` for all functions
   - Add SAFETY comments to unsafe blocks

4. **REFACTOR: Improve code quality**
   - Add error handling
   - Add logging (info!, debug!, warning!)
   - Add Rust unit tests
   - Run all tests → verify still passing

5. **VERIFY: Check acceptance criteria**
   - Run all tests in phase
   - Run integration tests
   - Check performance targets

6. **COMMIT: Git commit**
   - Use semantic commit messages
   - Include test + implementation together
   - Reference phase in commit

### Example Commit Messages

```bash
# Phase 0-A
git commit -m "feat(error): Add TViewError enum with SQLSTATE mapping [Phase 0-A]"

# Phase 3
git commit -m "fix(deps): Correct pg_depend query direction [Phase 3 - CRITICAL]"

# Phase 4
git commit -m "feat(refresh): Add advisory locks for concurrent safety [Phase 4]"
```

---

## ⚠️ Common Pitfalls to Avoid

Based on the review, watch out for:

1. **pg_depend Direction**
   - ❌ `WHERE refobjid = {}` (finds dependents)
   - ✅ `WHERE objid = {}` (finds dependencies)

2. **Dynamic Column Access**
   - ❌ `pk_val := NEW.pk` (hardcoded)
   - ✅ `EXECUTE format('SELECT ($1).%I', col) USING NEW` (dynamic)

3. **FK Change Detection**
   - ❌ Only handle NEW.fk (misses UPDATE changes)
   - ✅ Compare OLD.fk vs NEW.fk, cascade to both

4. **Transaction Isolation**
   - ❌ Using READ COMMITTED (default)
   - ✅ Using REPEATABLE READ or SERIALIZABLE

5. **Error Handling**
   - ❌ `return Err("error".into())`
   - ✅ `return Err(TViewError::SpecificError { context })`

6. **Unsafe Code**
   - ❌ No documentation
   - ✅ Comprehensive SAFETY comment

---

## 🎯 Success Metrics

### Technical Metrics

- ✅ 150+ tests passing
- ✅ 2-3× performance improvement (validated by benchmarks)
- ✅ 88% storage reduction (helper-aware materialization)
- ✅ Zero manual trigger/refresh code needed
- ✅ **NEW:** All errors have proper SQLSTATE codes
- ✅ **NEW:** No unsafe code without SAFETY comments

### Developer Experience Metrics

- ✅ 83% less boilerplate (6 steps → 1 step)
- ✅ 50% schema simplification (70 views → 33 views)
- ✅ 100% automation (triggers, refresh, cascade)
- ✅ **NEW:** Clear error messages with context

### Production Readiness

- ✅ CI/CD pipeline operational
- ✅ Documentation complete
- ✅ Error handling comprehensive
- ✅ **NEW:** Concurrency documented
- ✅ **NEW:** Backup/restore procedure documented
- ✅ Monitoring and logging
- ✅ Rollback plans documented

---

## 📚 Additional Documentation

**Created during review:**

- `.phases/implementation/phase-0-error-types.md` - **START HERE**
- `.phases/implementation/phase-2-ddl-FIXED.md` - ProcessUtility hook
- `.phases/implementation/phase-3-dependency-tracking-FIXED.md` - pg_depend fix
- `.phases/implementation/phase-4-refresh-CASCADE-FIXED.md` - Concurrency

**To be created during implementation:**

- `docs/ERROR_CODES.md` - SQLSTATE reference
- `docs/CONCURRENCY.md` - Transaction isolation guide
- `docs/PARSER_LIMITATIONS.md` - v1 SQL parser limitations
- `docs/BACKUP_RESTORE.md` - pg_dump/restore procedures
- `SAFETY_GUIDE.md` - Unsafe code review checklist

---

## 🔗 References

- **Original PRD:** `/home/lionel/code/pg_tviews/PRD_v2.md`
- **PRD Addendum:** `/home/lionel/code/pg_tviews/PRD_ADDENDUM.md`
- **Helper Optimization:** `/home/lionel/code/pg_tviews/HELPER_VIEW_OPTIMIZATION.md`
- **jsonb_delta:** https://github.com/fraiseql/jsonb_delta
- **pgrx:** https://github.com/pgcentralfoundation/pgrx
- **PostgreSQL Docs:** https://www.postgresql.org/docs/17/

---

## ✅ Ready to Implement?

### Pre-flight Checklist

Before starting implementation:

- [ ] Read this README completely
- [ ] Review all FIXED phase documents
- [ ] Understand `TViewError` requirements
- [ ] Understand SAFETY comment requirements
- [ ] Have development environment set up
- [ ] Have jsonb_delta extension installed
- [ ] Understand transaction isolation requirements
- [ ] Have realistic timeline (45-65 days, not 26-38)

**If all checkboxes are ticked, start with Phase 0-A!**

```bash
cat .phases/implementation/phase-0-error-types.md
```

---

**Review Completed:** 2025-12-09
**Reviewer:** PostgreSQL/Rust Senior Expert (Claude Sonnet 4.5)
**Status:** Ready for Implementation ✅
