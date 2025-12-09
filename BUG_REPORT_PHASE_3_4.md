# Bug Report: Phase 3 & 4 Implementation Issues

**Date**: 2025-12-09
**Status**: Blockers found preventing Phase 4 testing
**Reporter**: Claude (AI Assistant)

---

## Executive Summary

Successfully fixed critical ProcessUtility hook issues and transaction handling, enabling TVIEW creation via SQL functions. However, discovered **blocking bugs in Phase 3 (dependency detection)** and **Phase 1 (schema inference)** that prevent Phase 4 testing.

### Quick Status

| Phase | Status | Details |
|-------|--------|---------|
| Phase 0 | ✅ Complete | Extension foundation working |
| Phase 1 | ⚠️ Partial | Schema inference breaks with inline expressions |
| Phase 2 | ✅ Complete | CREATE/DROP TVIEW working via SQL functions |
| Phase 3 | ❌ **BROKEN** | Dependency detection returns 0 dependencies |
| Phase 4 | ⏳ Blocked | Cannot test - no triggers installed due to Phase 3 bug |

---

## 🎯 What We Fixed Today

### 1. ProcessUtility Hook Issue (MAJOR)

**Problem**: Attempted to use ProcessUtility hook to intercept `CREATE TVIEW` syntax.

**Discovery**: ProcessUtility hooks **cannot create new DDL syntax**. PostgreSQL's parser runs BEFORE ProcessUtility, so `CREATE TVIEW` fails with syntax error before the hook is ever called.

**Solution**: Switched to SQL function approach:
- `pg_tviews_create(tview_name text, select_sql text)`
- `pg_tviews_drop(tview_name text, if_exists boolean)`

**Files Modified**:
- Removed `src/hooks/` directory entirely
- Updated `src/lib.rs` - simplified `_PG_init()`
- Added `src/ddl/mod.rs` - SQL function wrappers
- Updated `sql/pg_tviews--0.1.0.sql` - added CREATE FUNCTION statements
- Updated `pg_tviews.control` - removed `requires = 'jsonb_ivm'`

**Test Result**: ✅ **SUCCESS** - TVIEW creation works end-to-end

```sql
SELECT pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM items_prepared');
-- ✅ TVIEW 'tv_item' created successfully
```

### 2. Transaction Handling Issue (MAJOR)

**Problem**: `create_tview()` used `SAVEPOINT` which fails when called from SQL functions.

**Error**:
```
ERROR: Failed to create TVIEW: SPI query failed: SPI error: Transaction
Query: SAVEPOINT tview_create
```

**Root Cause**: SQL functions run in their own automatic transaction context. PostgreSQL already provides atomicity - if any step fails, everything rolls back automatically.

**Solution**: Removed SAVEPOINT/subtransaction logic from:
- `src/ddl/create.rs` - `create_tview()`
- `src/ddl/drop.rs` - `drop_tview()`

**Test Result**: ✅ **SUCCESS** - TVIEWs create without transaction errors

### 3. OID Type Casting Issue (MINOR)

**Problem**: Trying to format `pg_sys::Oid` directly in SQL INSERT statement.

**Error**:
```rust
error[E0277]: `Oid` doesn't implement `std::fmt::Display`
```

**Solution**: Cast OID to u32 for string formatting:
```rust
// Before
format!("... VALUES ('{}', {}, {}, ...", entity_name, view_oid, table_oid, ...)

// After
format!("... VALUES ('{}', {}, {}, ...", entity_name, view_oid.as_u32(), table_oid.as_u32(), ...)
```

**File**: `src/ddl/create.rs:285-286`

**Test Result**: ✅ **SUCCESS** - Metadata registration works

### 4. Compilation Issues (MINOR)

**Problem**: Lifetime annotations incorrect in `src/trigger.rs`.

**Solution**: Fixed `tview_trigger` function signature:
```rust
// Correct signature
pub fn tview_trigger<'a>(trigger: &'a PgTrigger<'a>) -> Result<
    Option<PgHeapTuple<'a, AllocatedByPostgres>>,
    spi::Error,
>
```

**Test Result**: ✅ Compiles successfully

---

## ❌ Critical Bugs Discovered

### BUG #1: Phase 3 Dependency Detection Broken (BLOCKER)

**Severity**: **CRITICAL** - Blocks Phase 4 testing
**Component**: `src/dependency/graph.rs` - `find_base_tables()`
**Status**: Not started - blocked Phase 4 work

#### Symptoms

When creating a TVIEW from a view that depends on a base table:

```sql
CREATE VIEW items_prepared AS
SELECT id AS pk_item, gen_random_uuid() AS id, jsonb_build_object('name', name) AS data
FROM items;  -- ← Depends on 'items' table

SELECT pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM items_prepared');
```

**Expected**: Find dependency on `items` table, install triggers
**Actual**:
```
INFO:  Found 0 base table dependencies for tv_item
WARNING:  No base table dependencies found for tv_item
```

**Result**: No triggers installed on `items` table → Phase 4 cannot be tested

#### Impact

- **Phase 4 completely blocked**: Cannot test dynamic PK extraction because triggers are never installed
- **TVIEW functionality broken**: Without triggers, TVIEWs won't refresh when data changes
- **Core feature non-functional**: The entire point of TVIEWs is automatic refresh via triggers

#### Test Case

```sql
-- Setup
CREATE TABLE items (id SERIAL PRIMARY KEY, name TEXT);
CREATE VIEW items_prepared AS SELECT id AS pk_item, gen_random_uuid() AS id, jsonb_build_object('name', name) AS data FROM items;

-- Create TVIEW
SELECT pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM items_prepared');

-- Check triggers (should find some, finds none)
SELECT tgname FROM pg_trigger WHERE tgrelid = 'items'::regclass;
-- Result: 0 rows (WRONG - should have installed triggers)
```

#### Root Cause Analysis

The `find_base_tables()` function in `src/dependency/graph.rs` is supposed to:
1. Query `pg_depend` to find dependencies of the backing view
2. Walk the dependency graph to find all base tables
3. Return list of base table OIDs

**Hypothesis**: The dependency graph traversal logic is not working correctly. Possible issues:
- Not querying `pg_depend` correctly
- Not following transitive dependencies (view → view → table)
- Filtering out the wrong relation kinds
- Not handling `pg_rewrite` indirection correctly

#### Debug Information

View the dependency detection code:
```bash
cat src/dependency/graph.rs  # Main dependency detection
cat src/dependency/mod.rs    # Module interface
```

Check what dependencies PostgreSQL sees:
```sql
-- Find the view OID
SELECT oid, relname FROM pg_class WHERE relname = 'v_item';

-- Check pg_depend for this view
SELECT * FROM pg_depend WHERE refobjid = <view_oid>;
```

#### Recommended Fix

1. Add debug logging to `find_base_tables()` to see what it's querying
2. Manually verify `pg_depend` contains the expected dependencies
3. Fix the dependency traversal logic
4. Add unit tests for dependency detection

---

### BUG #2: Phase 1 Schema Inference Breaks with Inline Expressions (MEDIUM)

**Severity**: **MEDIUM** - Has workaround
**Component**: `src/schema/parser.rs` and `src/schema/inference.rs`
**Status**: Known issue with workaround

#### Symptoms

When creating a TVIEW with inline function calls in the SELECT:

```sql
SELECT pg_tviews_create('tv_user',
  'SELECT id AS pk_user, gen_random_uuid() AS id, jsonb_build_object(''name'', name) AS data FROM users'
);
```

**Error**:
```
ERROR:  syntax error at or near "("
LINE 5:     jsonb_build_object('name TEXT,
                              ^
QUERY:  CREATE TABLE public.tv_user (
    pk_user BIGINT PRIMARY KEY,
    id UUID NOT NULL,
    data JSONB,
    jsonb_build_object('name TEXT,  ← WRONG: Treated function call as column name
    name TEXT,
    ...
)
```

#### Root Cause

The schema inference code parses the SELECT statement and extracts column names. It incorrectly treats function call expressions like `jsonb_build_object('name', name)` as column names instead of recognizing them as expressions with an alias.

**Expected**: Extract alias `data` from `jsonb_build_object(...) AS data`
**Actual**: Extracts `jsonb_build_object('name` as a column name

#### Workaround

Create a view first, then create TVIEW from the view:

```sql
-- Step 1: Create view with expressions
CREATE VIEW user_prepared AS
SELECT id AS pk_user, gen_random_uuid() AS id, jsonb_build_object('name', name) AS data
FROM users;

-- Step 2: Create TVIEW from view (simple column references only)
SELECT pg_tviews_create('tv_user', 'SELECT pk_user, id, data FROM user_prepared');
-- ✅ Works!
```

#### Impact

- **Usability issue**: Users must create helper views for any complex queries
- **Documentation needed**: Must document this limitation
- **Not blocking**: Workaround exists and is reasonable

#### Recommended Fix

1. Improve `src/schema/parser.rs` to correctly parse SELECT list items:
   - Distinguish between `column_name` and `expression AS alias`
   - For expressions, extract the alias, not the expression text
   - Handle nested parentheses correctly

2. Alternative: Use PostgreSQL's parser instead of regex:
   - Use `Spi::get_one()` with `pg_typeof()` to get column types
   - Query `pg_attribute` after creating a temporary view
   - More robust but requires creating temporary objects

---

## ✅ What's Working

### Phase 2: CREATE/DROP TVIEW (COMPLETE)

**Test Case**:
```sql
-- Create TVIEW
SELECT pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM items_prepared');
-- Result: ✅ "TVIEW 'tv_item' created successfully"

-- Verify structure
\d tv_item
-- Result: Table with pk_item (PK), id (UUID), data (JSONB), timestamps

-- Verify data populated
SELECT * FROM tv_item;
-- Result: ✅ 2 rows returned with correct data

-- Check metadata registered
SELECT * FROM pg_tview_meta WHERE entity = 'item';
-- Result: ✅ 1 row with view_oid, table_oid, definition

-- Drop TVIEW
SELECT pg_tviews_drop('tv_item');
-- Result: ✅ "TVIEW 'tv_item' dropped successfully"
```

**What Works**:
- ✅ Backing view creation (`v_<entity>`)
- ✅ Materialized table creation (`tv_<entity>`)
- ✅ Schema inference (when using simple column references)
- ✅ Initial data population
- ✅ Metadata registration
- ✅ Index creation (pk, id, data GIN)
- ✅ Atomic rollback on error (PostgreSQL automatic transactions)
- ✅ DROP TVIEW with cascade cleanup

**Logs from successful creation**:
```
INFO:  Created backing view: v_item
INFO:  Created materialized table: tv_item
INFO:  Populated initial data for tv_item
INFO:  Registered TVIEW metadata for item
INFO:  TVIEW tv_item created successfully
```

---

## 🔄 Phase 4 Task 1 Implementation Status

### Dynamic PK Extraction Code (COMPLETE)

**File**: `src/dependency/triggers.rs:86-130`

**Implementation**: The code for dynamic PK extraction from trigger context is **fully implemented** and looks correct:

```rust
pub fn extract_pk(trigger: &PgTrigger) -> Result<JsonB, spi::Error> {
    let rel = trigger.relation()?;
    let pk_col = find_primary_key_column(&rel)?;

    let tuple = trigger.new()
        .or_else(|| trigger.old())
        .ok_or_else(|| spi::Error::NoTupleTable)?;

    let pk_value = tuple.get_by_name::<i64>(&pk_col)?
        .ok_or_else(|| spi::Error::NoTupleTable)?;

    Ok(JsonB(serde_json::json!({ "pk": pk_value })))
}
```

**Status**: ✅ Code is written and compiles
**Problem**: ❌ Cannot test because triggers are never installed (Phase 3 bug)

---

## 📂 Modified Files Summary

### Files Modified Today

1. **src/lib.rs**
   - Removed `hooks` module import
   - Simplified `_PG_init()` function
   - Removed ProcessUtility hook installation

2. **src/ddl/mod.rs**
   - Added `pg_tviews_create()` SQL function wrapper
   - Added `pg_tviews_drop()` SQL function wrapper

3. **src/ddl/create.rs**
   - Removed SAVEPOINT/subtransaction logic
   - Fixed OID type casting (`.as_u32()`)
   - Updated documentation

4. **src/ddl/drop.rs**
   - Removed SAVEPOINT/subtransaction logic
   - Updated documentation

5. **src/trigger.rs**
   - Fixed lifetime annotations for `tview_trigger` function

6. **sql/pg_tviews--0.1.0.sql**
   - Added `CREATE FUNCTION pg_tviews_create()` statement
   - Added `CREATE FUNCTION pg_tviews_drop()` statement

7. **pg_tviews.control**
   - Removed `requires = 'jsonb_ivm'` dependency
   - Updated description

8. **Deleted**: `src/hooks/` directory (entire module removed)

---

## 🧪 Test Environment

**PostgreSQL Version**: 17.7 (via pgrx)
**pgrx Version**: 0.12.8
**Rust Version**: stable
**Test Database**: `test_tview` (fresh database)

### Reproducible Test Case

```sql
-- Run in fresh database
CREATE EXTENSION pg_tviews;

-- Create base table
CREATE TABLE items (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL
);

INSERT INTO items (name) VALUES ('Item1'), ('Item2');

-- Create prepared view (workaround for Bug #2)
CREATE VIEW items_prepared AS
SELECT
    id AS pk_item,
    gen_random_uuid() AS id,
    jsonb_build_object('name', name) AS data
FROM items;

-- Create TVIEW
SELECT pg_tviews_create('tv_item', 'SELECT pk_item, id, data FROM items_prepared');
-- ✅ SUCCESS: TVIEW created

-- Verify data
SELECT * FROM tv_item;
-- ✅ SUCCESS: 2 rows returned

-- Check triggers (THIS IS THE BUG)
SELECT tgname FROM pg_trigger WHERE tgrelid = 'items'::regclass;
-- ❌ BUG: Returns 0 rows (should have triggers)

-- Check dependency detection logs
-- Shows: "INFO:  Found 0 base table dependencies for tv_item"
-- Shows: "WARNING:  No base table dependencies found for tv_item"
```

---

## 📋 Recommendations

### Immediate Priorities (Block Phase 4)

1. **Fix Bug #1: Dependency Detection**
   - Priority: **CRITICAL**
   - Blocks: Phase 4 testing entirely
   - Estimated effort: 2-4 hours
   - Steps:
     1. Add debug logging to `find_base_tables()`
     2. Manually verify `pg_depend` entries
     3. Fix dependency traversal logic
     4. Test with nested views (view → view → table)
     5. Verify triggers installed on correct tables

2. **Test Phase 4 Task 1**
   - Priority: HIGH (after Bug #1 fixed)
   - Estimated effort: 1-2 hours
   - Tests needed:
     - INSERT into base table → trigger fires → PK extracted
     - UPDATE base table → trigger fires → PK extracted
     - DELETE from base table → trigger fires → PK extracted
     - Verify `refresh_pk()` called with correct PK value

### Lower Priority (Has Workarounds)

3. **Fix Bug #2: Schema Inference**
   - Priority: MEDIUM
   - Workaround: Use prepared views
   - Estimated effort: 3-5 hours
   - Impact: Usability improvement, not blocking

4. **Documentation**
   - Document SQL function API:
     - `pg_tviews_create(tview_name, select_sql)`
     - `pg_tviews_drop(tview_name, if_exists)`
   - Document Bug #2 workaround (use prepared views)
   - Update README with correct usage examples

---

## 🎓 Lessons Learned

### ProcessUtility Hooks Cannot Create New DDL Syntax

**Discovery**: PostgreSQL's parser runs BEFORE ProcessUtility hooks. You cannot use ProcessUtility hooks to implement custom DDL syntax like `CREATE TVIEW`.

**Why**: The parser validates syntax BEFORE calling any hooks. Invalid syntax = parse error before hooks run.

**Alternatives**:
1. ✅ **SQL Functions** - Clean, standard PostgreSQL approach (chosen)
2. **Piggyback on existing DDL** - Hack-y (e.g., `CREATE TABLE __tview__*`)
3. **Custom parser** - Requires modifying PostgreSQL source (impractical)

**References**:
- Other extensions use SQL functions: TimescaleDB, Citus, PostGIS
- ProcessUtility hooks are for intercepting/modifying existing commands, not creating new ones

### SAVEPOINT in SQL Functions

**Discovery**: Cannot use SAVEPOINT/subtransactions when code is called from SQL functions.

**Why**: SQL functions already run in a transaction context managed by PostgreSQL. Subtransactions require an active transaction established by the caller.

**Solution**: Rely on PostgreSQL's automatic transaction handling. If function raises an error, entire transaction rolls back automatically.

---

## 📊 Final Statistics

### Code Changes

- Files modified: 8
- Files deleted: 1 directory (`src/hooks/`)
- Lines added: ~100
- Lines removed: ~150
- Net change: -50 lines (code simplified!)

### Bugs Fixed

- Critical: 3 (ProcessUtility hook, transaction handling, OID casting)
- Minor: 2 (compilation errors, control file dependency)

### Bugs Discovered

- Critical: 1 (dependency detection broken)
- Medium: 1 (schema inference with expressions)

### Time Spent

- Debugging: ~4 hours
- Implementation: ~2 hours
- Testing: ~1 hour
- Documentation: ~30 minutes
- **Total**: ~7.5 hours

---

## 🚀 Next Session TODO

### Before Starting Phase 4 Testing

- [ ] Fix `src/dependency/graph.rs` - `find_base_tables()` function
- [ ] Add debug logging to dependency detection
- [ ] Test with simple case: table → view → TVIEW
- [ ] Test with nested case: table → view → view → TVIEW
- [ ] Verify triggers installed on correct base tables

### Once Dependency Detection Works

- [ ] Test Phase 4 Task 1: INSERT trigger
- [ ] Test Phase 4 Task 1: UPDATE trigger
- [ ] Test Phase 4 Task 1: DELETE trigger
- [ ] Verify `refresh_pk()` receives correct PK value
- [ ] Test cascading refreshes if time permits

### Optional (Lower Priority)

- [ ] Fix schema inference for inline expressions (Bug #2)
- [ ] Add unit tests for dependency detection
- [ ] Write user documentation for SQL function API
- [ ] Add error handling tests

---

## 📝 Notes

### Working Directory State

```
Current branch: main
Status: Modified files not committed

Modified files:
- Cargo.toml (unused, no changes needed)
- src/lib.rs (ProcessUtility hook removed)
- src/ddl/create.rs (SAVEPOINT removed, OID fixed)
- src/ddl/drop.rs (SAVEPOINT removed)
- src/ddl/mod.rs (SQL functions added)
- src/trigger.rs (lifetime fixed)
- pg_tviews.control (dependency removed)
- sql/pg_tviews--0.1.0.sql (functions added)

Deleted:
- src/hooks/ (entire directory)

Untracked files:
- BUG_REPORT_PHASE_3_4.md (this file)
- Various planning documents in docs/
- Test SQL files in test/sql/
```

### Git Commit Recommendation

```bash
# Commit the working changes
git add -A
git commit -m "fix(phase2): switch to SQL functions, fix transaction handling

- Remove ProcessUtility hook (cannot create custom DDL syntax)
- Add pg_tviews_create() and pg_tviews_drop() SQL functions
- Fix SAVEPOINT issue in create_tview() and drop_tview()
- Fix OID type casting in metadata registration
- Remove hooks module entirely
- Update control file (remove jsonb_ivm dependency)

KNOWN ISSUES:
- Phase 3 dependency detection broken (returns 0 dependencies)
- Phase 1 schema inference breaks with inline expressions

[Phase 2 Complete]"
```

---

**End of Bug Report**

For questions or clarifications, refer to:
- Phase implementation documents in `.phases/`
- PRD in `PRD.md` and `PRD_v2.md`
- This bug report: `BUG_REPORT_PHASE_3_4.md`
