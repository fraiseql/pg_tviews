# Phase 4 Quick Start Guide

**⏱️ Time to read:** 2 minutes
**🎯 Goal:** Start implementing Phase 4 immediately

---

## 🚀 Start Now (3 Commands)

```bash
# 1. Review the plan
cat PHASE_4_PLAN.md | less

# 2. Check environment ready
cat .phase4-ready

# 3. Start Task 1
# Edit: src/dependency/triggers.rs
# Test: test/sql/40_refresh_trigger_dynamic_pk.sql
```

---

## 📋 Task Order (Do in sequence)

| # | Task | File | Test | Days |
|---|------|------|------|------|
| 1 | Dynamic PK extraction | `src/dependency/triggers.rs` | `40_*.sql` | 1 |
| 2 | Single row refresh | `src/refresh.rs` | `41_*.sql` | 2-3 |
| 3 | FK cascade | `src/propagate.rs` | `42_*.sql` | 3-4 |
| 4 | Depth limiting | `src/refresh.rs` + `src/error/mod.rs` | `43_*.sql` | 1-2 |
| 5 | Isolation check | `src/dependency/triggers.rs` | All tests | 1 |
| 6 | Wire to Rust | `src/lib.rs` | `44_*.sql` | 2 |

**Total:** 10-14 days

---

## 🔄 TDD Workflow (Every Task)

```bash
# RED: Test fails
cargo pgrx test -- test_name

# GREEN: Implement
# Edit code...

# REFACTOR: Improve
cargo build

# VERIFY: Test passes
cargo pgrx test -- test_name

# COMMIT
git add .
git commit -m "feat(refresh): [task description] [Phase 4 Task N]"
```

---

## 📝 Task 1 Details (Start Here)

### Goal
Fix trigger handler to extract PK column name dynamically.

### Current Problem
```sql
-- Hardcoded (WRONG):
pk_value := OLD.pk;  -- Doesn't work for pk_post, pk_user, etc.
```

### Solution
```sql
-- Dynamic (CORRECT):
-- 1. Get PK column name from catalog
SELECT a.attname INTO pk_col_name
FROM pg_index i
JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
WHERE i.indrelid = TG_RELID AND i.indisprimary;

-- 2. Extract value dynamically
EXECUTE format('SELECT ($1).%I', pk_col_name) USING OLD INTO pk_val_old;
EXECUTE format('SELECT ($1).%I', pk_col_name) USING NEW INTO pk_val_new;
```

### File to Edit
```bash
vim src/dependency/triggers.rs
# Find: create_trigger_handler()
# Replace handler SQL with dynamic PK extraction
```

### Test to Run
```bash
cargo pgrx test -- 40_refresh_trigger_dynamic_pk
```

### Expected Result
✅ Test 40 passes - trigger works with pk_post, pk_user, etc.

---

## 🧪 Running Tests

```bash
# Single test
cargo pgrx test -- 40_refresh

# All Phase 4 tests
for i in {40..44}; do cargo pgrx test -- $i; done

# All tests
cargo pgrx test

# With logging
RUST_LOG=debug cargo pgrx test -- test_name
```

---

## 🔍 Debugging

### Enable Verbose Logging
```rust
// In code
use pgrx::prelude::*;
info!("Debug message: {}", value);
warning!("Warning: {}", msg);
```

### Check Trigger Installed
```sql
SELECT tgname, tgrelid::regclass
FROM pg_trigger
WHERE tgname LIKE 'trg_tview_%';
```

### View Metadata
```sql
SELECT * FROM pg_tview_meta;
```

---

## 📚 Key Files Reference

### Implementation
- `src/dependency/triggers.rs` - Trigger handler (Tasks 1, 5)
- `src/refresh.rs` - Refresh logic (Tasks 2, 4)
- `src/propagate.rs` - Cascade logic (Task 3)
- `src/lib.rs` - Export functions (Task 6)
- `src/error/mod.rs` - Error types (Task 4)

### Tests
- `test/sql/40_*.sql` - Dynamic PK (Task 1)
- `test/sql/41_*.sql` - Single refresh (Task 2)
- `test/sql/42_*.sql` - Cascade (Task 3)
- `test/sql/43_*.sql` - Depth limit (Task 4)
- `test/sql/44_*.sql` - Integration (Tasks 1-6)

### Documentation
- `PHASE_4_PLAN.md` - Detailed implementation guide
- `docs/CONCURRENCY.md` - Concurrency model
- `test/sql/README_PHASE4.md` - Test documentation

---

## ⚠️ Critical Requirements

### 1. Transaction Isolation
All operations require **REPEATABLE READ** or **SERIALIZABLE**.

```sql
-- In tests (already done)
BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;
-- ... test code ...
ROLLBACK;
```

### 2. Error Handling
Always use `TViewResult` and `TViewError`:

```rust
// Good
pub fn my_function() -> TViewResult<()> {
    something()?;
    Ok(())
}

// Bad
pub fn my_function() -> Result<(), Error> {
    something()?;
    Ok(())
}
```

### 3. Advisory Locks
Lock before refresh (already stubbed in plan):

```rust
lock_tview_row(entity, pk_value, timeout)?;
refresh_tview_row(entity, pk_value)?;
```

---

## ✅ Acceptance Criteria (Phase 4 Complete)

- [ ] All 5 SQL tests pass (40-44)
- [ ] `cargo test` passes
- [ ] Single row refresh < 5ms
- [ ] 100-row cascade < 500ms
- [ ] No memory leaks
- [ ] Clear error messages
- [ ] Code documented

---

## 🎯 Success Metrics

| Metric | Target | How to Measure |
|--------|--------|----------------|
| Test Pass Rate | 100% | `cargo pgrx test` |
| Single Row Refresh | < 5ms | `\timing` in SQL tests |
| 100-Row Cascade | < 500ms | Test 44 timing |
| Memory Usage | < 50MB | `pg_stat_activity` |
| Code Coverage | > 80% | `cargo tarpaulin` |

---

## 🆘 Help & Resources

### Stuck?
1. Read `PHASE_4_PLAN.md` for detailed steps
2. Check `docs/CONCURRENCY.md` for concurrency issues
3. Review existing code in `src/refresh.rs` (has stubs)
4. Look at test expectations in `test/sql/4*.sql`

### Common Issues
- **Test fails:** Check extension installed (`cargo pgrx install`)
- **Compilation error:** Check Rust version (`rustc --version`)
- **Isolation warning:** Set database default (see CONCURRENCY.md)

### External Docs
- pgrx: https://github.com/pgcentralfoundation/pgrx
- PostgreSQL triggers: https://www.postgresql.org/docs/current/triggers.html
- Advisory locks: https://www.postgresql.org/docs/current/explicit-locking.html

---

## 📊 Progress Tracking

```bash
# After each task, update todo list
# Mark task complete
# Commit with descriptive message

# Example commits:
git commit -m "feat(refresh): implement dynamic PK extraction [Phase 4 Task 1]"
git commit -m "feat(refresh): implement single row refresh [Phase 4 Task 2]"
git commit -m "feat(refresh): implement FK cascade [Phase 4 Task 3]"
```

---

## 🎉 When Phase 4 Complete

```bash
# 1. Run all tests
cargo pgrx test

# 2. Check acceptance criteria
cat PHASE_4_PLAN.md | grep "Acceptance Criteria" -A 20

# 3. Commit final
git add .
git commit -m "feat(refresh): complete Phase 4 - refresh and cascade logic [Phase 4]"

# 4. Move to Phase 5
cat .phases/implementation/phase-5-arrays-and-optimization.md
```

---

**⏱️ Total Time:** 14-21 days
**🎯 Next Task:** Task 1 - Dynamic PK extraction
**📖 Full Details:** `PHASE_4_PLAN.md`

**Let's implement! 🚀**
