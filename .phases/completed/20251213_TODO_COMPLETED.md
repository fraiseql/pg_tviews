# Benchmark Fixes TODO - December 13, 2025

## ✅ COMPLETED - 2025-12-13

**Status**: Major fixes completed and committed (96054e4)

### Work Completed:

1. ✅ **Schema Context** - Added SET search_path to SQL files
2. ✅ **Function Calls** - Schema-qualified all record_benchmark() calls
3. ✅ **TVIEW Validation** - Updated Rust code to only require id + data
4. ✅ **Schema Structure** - Added id column to tv_product

**Final Commit**: `96054e4` - "fix(benchmarks): Fix schema context, function calls, validation, and TVIEW structure"
**Files Changed**: 11 files (2 Rust, 9 SQL)
**Next Steps**: See `.phases/20251214_TODO.md` for remaining tasks

---

## Original TODO (Archive)

### Status: Partial Fix Completed ✅

**Initial Commit**: `5dcfa20` - Fixed critical psql variable quoting issue

---

## ✅ COMPLETED

### 1. Psql Variable Quoting in Data Generation
**File**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh:127`
**Status**: FIXED ✅
**Commit**: 5dcfa20

**Issue**: Data generation failed with `ERROR: syntax error at or near ":"` because psql variable interpolation was broken.

**Root Cause**:
```bash
# WRONG (was causing syntax error):
$PSQL -v data_scale="'$scale'" -f "data/${scenario}_data.sql"
# This created: data_scale="'small'"
# Which expanded :data_scale to :'small' (invalid SQL)

# FIXED:
$PSQL -v data_scale="$scale" -f "data/${scenario}_data.sql"
# This creates: data_scale="small"
# Which expands :data_scale to 'small' (valid SQL)
```

**Evidence**: Benchmark log showed:
```
psql:data/01_ecommerce_data.sql:148: ERROR: syntax error at or near ":"
```

---

## 🔴 CRITICAL - Must Fix Before Benchmarks Will Work

### 2. Schema Context Problem - Tables Created in Wrong Schema
**File**: `test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql`
**Status**: NOT FIXED ❌
**Priority**: HIGH

**Issue**: Tables are being created in `public` schema instead of `benchmark` schema, causing:
- `ERROR: relation "tb_category" already exists` on subsequent runs
- Cleanup script `DROP SCHEMA benchmark CASCADE` doesn't remove them
- Schema pollution between test runs

**Error Evidence**:
```
psql:schemas/01_ecommerce_schema.sql:16: ERROR: relation "tb_category" already exists
```

**Root Cause**: The schema file doesn't set search_path before creating tables.

**Fix Required**:
Add to the beginning of `schemas/01_ecommerce_schema.sql`:
```sql
-- Ensure all objects are created in benchmark schema
SET search_path TO benchmark, public;
```

**Verification**:
```bash
# After fix, verify tables are in correct schema:
psql -d pg_tviews_benchmark -c "\dt benchmark.*"
# Should show: benchmark.tb_category, benchmark.tb_product, etc.
# NOT: public.tb_category
```

---

### 3. Benchmark Infrastructure Location Issue
**File**: `test/sql/comprehensive_benchmarks/00_setup.sql`
**Status**: NOT FIXED ❌
**Priority**: HIGH

**Issue**: Benchmark infrastructure (tables, functions) created in one schema but accessed from another.

**Error Evidence**:
```
ERROR: function record_benchmark(unknown, unknown, unknown, unknown, integer, integer, numeric, unknown) does not exist
```

**Root Cause**:
- Setup creates `benchmark_results`, `record_benchmark()`, etc. in `public` schema
- But benchmarks run with `search_path = benchmark, public`
- Functions aren't found because they're not schema-qualified

**Fix Options**:

**Option A** (Recommended): Keep infrastructure in `public`, use qualified names
```sql
-- In scenario benchmarks, change:
SELECT record_benchmark(...)
-- To:
SELECT public.record_benchmark(...)
```

**Option B**: Create infrastructure in both schemas
```sql
-- In 00_setup.sql, add:
CREATE SCHEMA IF NOT EXISTS benchmark;
-- Then create tables/functions in benchmark schema
```

**Option C**: Use search_path consistently
```sql
-- Ensure all SQL files use:
SET search_path TO public, benchmark;
-- Instead of:
SET search_path TO benchmark, public;
```

**Recommendation**: Use Option A - it's clearest and prevents ambiguity.

---

## 🟡 MEDIUM PRIORITY - Will Cause Some Tests to Fail

### 4. TVIEW Syntax Validation Too Strict
**File**: `pg_tviews` Rust code (event trigger handler)
**Status**: NOT FIXED ❌
**Priority**: MEDIUM

**Issue**: Automatic TVIEW detection rejects valid table structures with overly strict validation.

**Error Evidence**:
```
WARNING: Invalid TVIEW syntax for 'tv_product': Missing jsonb_build_object for data column -
TVIEW must have: pk_<entity>, id (UUID), data (JSONB) columns
```

**Actual Schema**:
```sql
CREATE TABLE tv_product AS
SELECT
    pk_product,      -- ✓ Has pk_<entity>
    fk_category,     -- Extra column (valid!)
    data             -- ✓ Has data column
FROM v_product;
```

**Root Cause**: The validation logic in `pg_tviews` assumes:
1. MUST have exactly: `pk_<entity>`, `id`, `data` columns
2. CANNOT have additional columns like `fk_category`

**Why This is Wrong**: TVIEWs can have additional columns for foreign key caching, which is a valid optimization pattern.

**Fix Required**: Modify validation in `src/event_trigger.rs` or `src/ddl/mod.rs`:
```rust
// Current (too strict):
fn validate_tview_syntax(table_name: &str, columns: &[Column]) -> Result<(), String> {
    // Requires EXACTLY: pk_, id, data
    if columns.len() != 3 { return Err("Wrong column count"); }
    // ...
}

// Should be (more permissive):
fn validate_tview_syntax(table_name: &str, columns: &[Column]) -> Result<(), String> {
    // Requires AT LEAST: pk_, id, data
    // Allow additional columns for FK caching, computed columns, etc.
    let has_pk = columns.iter().any(|c| c.name.starts_with("pk_"));
    let has_id = columns.iter().any(|c| c.name == "id");
    let has_data = columns.iter().any(|c| c.name == "data");

    if !has_pk || !has_id || !has_data {
        return Err("Missing required columns");
    }
    Ok(())
}
```

**Verification**:
```sql
-- After fix, this should succeed:
CREATE TABLE tv_product AS
SELECT
    pk_product,
    fk_category,    -- Extra column should be allowed
    data
FROM v_product;

-- Should get: SUCCESS instead of WARNING
```

---

## 🟢 LOW PRIORITY - Nice to Have

### 5. Improve Error Messages in Cleanup Script
**File**: `test/sql/comprehensive_benchmarks/cleanup_schema.sql`
**Status**: WORKS BUT COULD BE BETTER
**Priority**: LOW

**Current Behavior**: Script silently drops public schema tables if they exist.

**Improvement**: Add explicit cleanup for public schema tables with notices:
```sql
-- Drop any leftover tables in public schema (from failed runs)
DO $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN
        SELECT tablename
        FROM pg_tables
        WHERE schemaname = 'public'
        AND tablename IN ('tb_category', 'tb_supplier', 'tb_product', 'tb_review', 'tb_inventory')
    LOOP
        RAISE NOTICE 'Cleaning up leaked table: public.%', r.tablename;
        EXECUTE format('DROP TABLE IF EXISTS public.%I CASCADE', r.tablename);
    END LOOP;
END$$;
```

---

### 6. Add Diagnostic Logging to Benchmark Runner
**File**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`
**Status**: WORKS BUT LIMITED DIAGNOSTICS
**Priority**: LOW

**Improvement**: Add more diagnostic output to help debug issues:
```bash
# After schema load, add:
log "  Verifying schema state..."
$PSQL -c "SELECT schemaname, tablename FROM pg_tables WHERE schemaname IN ('benchmark', 'public') ORDER BY schemaname, tablename;" | tee -a "$LOG_FILE"

# After data generation, add:
log "  Verifying data loaded..."
$PSQL -c "SELECT 'tb_category' as table, COUNT(*) FROM benchmark.tb_category
           UNION ALL SELECT 'tb_product', COUNT(*) FROM benchmark.tb_product
           UNION ALL SELECT 'tb_review', COUNT(*) FROM benchmark.tb_review
           UNION ALL SELECT 'tv_product', COUNT(*) FROM benchmark.tv_product;" | tee -a "$LOG_FILE"
```

---

## 📋 Implementation Plan

### Phase 1: Critical Fixes (Must Do)
1. ✅ Fix psql variable quoting (DONE - commit 5dcfa20)
2. ❌ Fix schema context in `schemas/01_ecommerce_schema.sql`
3. ❌ Fix benchmark infrastructure accessibility in scenarios
4. Test: Run `./scripts/master.sh --scale small` and verify data generation succeeds

### Phase 2: TVIEW Validation (Should Do)
1. Modify TVIEW validation logic in Rust code
2. Rebuild extension
3. Test: Create TVIEW with extra columns, verify no warnings

### Phase 3: Improvements (Nice to Have)
1. Improve cleanup script with notices
2. Add diagnostic logging to runner
3. Document benchmark architecture

---

## 🧪 Testing Strategy

### After Each Fix

**Quick Test** (5 minutes):
```bash
# Test single scale
./scripts/master.sh --scale small

# Check for specific errors:
# ✓ No "syntax error at or near :"
# ✓ No "relation already exists"
# ✓ No "function does not exist"
```

**Full Test** (15 minutes):
```bash
# Test all scales
./scripts/master.sh --scale "small medium large"

# Verify results:
cat /tmp/benchmark_results_*.log | grep -E "ERROR|SUCCESS|completed"
```

### Success Criteria

Benchmark run is successful when:
- [x] Data generation completes without syntax errors
- [ ] Schema loads without "relation exists" errors
- [ ] All benchmark tests execute (even if some fail)
- [ ] Results are written to benchmark_results table
- [ ] CSV export succeeds

**Current Status**: 1/5 criteria met

---

## 📝 Notes

### Why Parameters Were Broken

The shell parameter expansion works like this:

```bash
# Shell variable:
scale="small"

# WRONG: -v data_scale="'$scale'"
# Results in: data_scale='small'    (note: data_scale contains the string 'small' WITH quotes)
# In psql: :data_scale expands to 'small'
# In PL/pgSQL: v_scale := :'small';   ← SYNTAX ERROR (two quote levels)

# RIGHT: -v data_scale="$scale"
# Results in: data_scale=small      (note: data_scale contains the string small WITHOUT quotes)
# In psql: :data_scale expands to small
# In PL/pgSQL: v_scale := :'small';   ← ERROR still! We need...
# Actually in the SQL: v_scale TEXT := :'data_scale';
# With -v data_scale="$scale", this becomes: v_scale TEXT := :data_scale; → v_scale TEXT := small;
# Which needs quoting in the variable value, so actually:
# -v data_scale="'$scale'" was attempting to provide the quotes
# But the issue is the data file line 18: v_scale TEXT := :'data_scale';
# The :' syntax means "expand variable and quote it"
# So with data_scale='small', we get: v_scale TEXT := :''small''; (double quotes - error!)
# With data_scale=small, we get: v_scale TEXT := :'small'; (correct!)
```

Actually, reviewing the data file line 18:
```sql
v_scale TEXT := :'data_scale';  -- Use psql variable: small, medium, large
```

The `:` prefix expands the psql variable, and the `'` after `:` means "quote the result as a string literal".
- So `:data_scale` would expand to raw value (error if not quoted)
- And `:'data_scale'` expands and quotes (correct)
- With `-v data_scale="$scale"` where scale=small:
  - data_scale psql variable = small
  - :'data_scale' expands to 'small' (quoted)
  - Final SQL: v_scale TEXT := 'small'; ✓

- With `-v data_scale="'$scale'"` where scale=small:
  - data_scale psql variable = 'small' (already has quotes)
  - :'data_scale' expands to '''small''' (triple quotes!)
  - Final SQL: v_scale TEXT := '''small'''; ✗ (syntax error)

So the fix was correct!

### Schema Architecture

```
Database: pg_tviews_benchmark
├── public schema
│   ├── Extensions: pg_tviews, jsonb_delta, uuid-ossp
│   ├── Infrastructure:
│   │   ├── benchmark_results (table)
│   │   ├── benchmark_comparison (view)
│   │   ├── record_benchmark() (function)
│   │   └── calculate_improvement() (function)
│   └── Monitoring views from pg_tviews
│
└── benchmark schema (test isolation)
    ├── Base tables:
    │   ├── tb_category
    │   ├── tb_supplier
    │   ├── tb_product
    │   ├── tb_review
    │   └── tb_inventory
    ├── Helper views:
    │   └── v_product
    ├── TVIEWs (4 approaches):
    │   ├── tv_product (pg_tviews+jsonb_delta)
    │   ├── manual_product (manual function refresh)
    │   ├── manual_func_product (manual unlimited cascade)
    │   └── mv_product (traditional matview)
    └── Refresh functions:
        └── refresh_manual_product()
```

**Key Principle**: Infrastructure in `public` (persistent), test data in `benchmark` (ephemeral).

---

## 🔗 Related Files

- Main runner: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`
- Setup: `test/sql/comprehensive_benchmarks/00_setup.sql`
- Cleanup: `test/sql/comprehensive_benchmarks/cleanup_schema.sql`
- Schema: `test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql`
- Data: `test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql`
- Benchmarks: `test/sql/comprehensive_benchmarks/scenarios/01_ecommerce_benchmarks.sql`
- Master script: `scripts/master.sh`
- Benchmark executor: `scripts/05_run_benchmarks.sh`

---

## 📊 Expected Timeline

- **Phase 1** (Critical): 30-60 minutes
  - Schema context fix: 10 minutes
  - Infrastructure fix: 15 minutes
  - Testing: 15-30 minutes

- **Phase 2** (TVIEW validation): 1-2 hours
  - Rust code changes: 30-60 minutes
  - Rebuild + test: 30-60 minutes

- **Phase 3** (Improvements): 30 minutes
  - Optional enhancements

**Total Estimate**: 2-3.5 hours for all phases

---

Last Updated: 2025-12-13
Status: 1 critical fix completed (psql quoting), 2 critical fixes remaining (schema context, infrastructure)
