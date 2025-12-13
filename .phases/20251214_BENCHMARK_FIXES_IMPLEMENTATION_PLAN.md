# Benchmark Fixes Implementation Plan - December 14, 2025

## 🎯 Executive Summary

**Objective**: Fix all remaining benchmark issues to enable reliable automated testing
**Timeline**: 3-4 hours total (Phase 1: 2-3 hours, Phase 2: 1 hour)
**Risk Level**: Medium (well-understood issues, clear fix paths)
**Success Criteria**: All benchmarks run successfully with manual TVIEW conversion workflow

**Key Architectural Decisions**:
- **TVIEW Auto-Conversion**: DISABLED in event triggers due to PostgreSQL SPI limitations
- **Manual Conversion**: Users must call `pg_tviews_convert_existing_table()` after CREATE TABLE AS SELECT
- **Future**: Background worker for automatic conversion (Phase 3)

---

## 📋 Phase Overview

### Phase 1: Critical Fixes (2-3 hours)
**Goal**: Fix blocking issues preventing benchmark execution

| Task | Priority | Time | Risk | Dependencies |
|------|----------|------|------|--------------|
| 1.1 Data Generation Fix | P0 | 45 min | Low | None |
| 1.2 TVIEW Conversion Fix | P0 | 90 min | Medium | None |
| 1.3 Scenarios Variable Fix | P1 | 10 min | Low | None |
| 1.4 End-to-End Verification | P0 | 30 min | Low | 1.1, 1.2, 1.3 |
| 1.5 Commit Changes | P0 | 15 min | Low | 1.4 |

### Phase 2: Documentation & Polish (1 hour)
**Goal**: Update docs and add diagnostics

| Task | Priority | Time | Risk | Dependencies |
|------|----------|------|------|--------------|
| 2.1 Documentation Updates | Medium | 30 min | Low | Phase 1 |
| 2.2 Diagnostic Logging | Low | 30 min | Low | Phase 1 |

---

## 🔴 Phase 1: Critical Fixes Implementation

### Task 1.1: Fix Data Generation Psql Variable Interpolation
**Priority**: P0 (blocks all data loading)
**Time Estimate**: 45 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql`

#### Step 1.1.1: Diagnose the Issue (10 minutes)
```bash
# Navigate to benchmark directory
cd test/sql/comprehensive_benchmarks

# Examine line 151 where error occurs
sed -n '145,155p' data/01_ecommerce_data.sql | cat -n

# Look for unquoted :data_scale usage
grep -n ":data_scale" data/01_ecommerce_data.sql
```

**Expected Finding**: Line 151 has `:data_scale` (unquoted) instead of `:'data_scale'` (quoted)

#### Step 1.1.2: Understand Psql Variable Syntax (5 minutes)
**Key Learning**: Psql variable interpolation rules:
- `:variable` → Unquoted (for numbers, booleans, SQL keywords)
- `:'variable'` → Single-quoted (for strings, identifiers)
- `:"variable"` → Double-quoted (for case-sensitive identifiers)

**The Problem**: Shell passes `data_scale="small"` but SQL expects string interpolation with `:'data_scale'`

#### Step 1.1.3: Fix the Variable Interpolation (5 minutes)
```bash
# Read the problematic section
read -r line_num line_content < <(grep -n ":data_scale" data/01_ecommerce_data.sql | grep -v ":'data_scale" | head -1)

# Display the exact line that needs fixing
echo "Line $line_num needs fixing:"
sed -n "${line_num}p" data/01_ecommerce_data.sql

# The fix: Change :data_scale to :'data_scale'
# This is typically in a WHERE clause like:
# WHERE some_condition = :data_scale  →  WHERE some_condition = :'data_scale'
```

**Manual Fix Required**:
1. Open `data/01_ecommerce_data.sql` in editor
2. Go to line 151 (or wherever the unquoted `:data_scale` is)
3. Change `:data_scale` to `:'data_scale'`

#### Step 1.1.4: Test the Fix (15 minutes)
```bash
# Test 1: Direct psql variable interpolation
psql -d pg_tviews_benchmark -v data_scale="small" -c "
DO \$\$
BEGIN
    RAISE NOTICE 'Testing quoted interpolation: %', :'data_scale';
END \$\$;
"
# Expected: NOTICE: Testing quoted interpolation: small

# Test 2: Run the data generation script
psql -d pg_tviews_benchmark -v data_scale="small" -f data/01_ecommerce_data.sql

# Test 3: Verify data was loaded
psql -d pg_tviews_benchmark -c "
SELECT 'tb_category' as table_name, COUNT(*) as row_count FROM benchmark.tb_category
UNION ALL
SELECT 'tb_product', COUNT(*) FROM benchmark.tb_product;
"
```

#### Step 1.1.5: Verification Checklist
- [ ] Psql variable interpolation works without syntax errors
- [ ] Data generation completes successfully
- [ ] tb_category and tb_product tables have data
- [ ] No "syntax error at or near :" messages

**Success Criteria Met**: Data generation works for all scales (small, medium, large)

---

### Task 1.2: Fix TVIEW Conversion SPI Transaction Issue
**Priority**: P0 (blocks automatic TVIEW creation)
**Time Estimate**: 90 minutes
**Risk**: Medium (architectural change)
**Files Modified**: `src/event_trigger.rs`, `README.md`

#### Step 1.2.1: Understand the Problem (15 minutes)
**Root Cause**: PostgreSQL event triggers cannot use SPI (Server Programming Interface) to query catalogs during DDL events due to transaction isolation.

**The Issue**:
- Event triggers run in the same transaction as DDL commands
- SPI calls create sub-transactions
- PostgreSQL prevents nested transactions during DDL events
- Error: "SPI error: Transaction Query: Unknown"

#### Step 1.2.2: Test Manual Conversion Works (10 minutes)
```bash
# First ensure tv_product table exists
psql -d pg_tviews_benchmark -c "
SET search_path TO benchmark, public;
\dt tv_product
"

# Test manual conversion (outside event trigger context)
psql -d pg_tviews_benchmark -c "
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');
"

# Verify TVIEW was created
psql -d pg_tviews_benchmark -c "
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_product';
"
```

**Expected Result**: Manual conversion succeeds (proves SPI works outside event triggers)

#### Step 1.2.3: Implement Event Trigger Fix (30 minutes)
**Decision**: Disable auto-conversion in event triggers (Option A from TODO)

**File to Modify**: `src/event_trigger.rs`

**Current Code** (find this function):
```rust
pub fn on_create_table_as_select_end() {
    // Current: tries to auto-convert using SPI
    convert_to_tview()?;
}
```

**New Code** (replace with):
```rust
pub fn on_create_table_as_select_end() -> Result<(), Box<dyn Error>> {
    // Only validate TVIEW structure (no SPI needed)
    validate_tview_structure()?;

    // Log manual conversion instruction
    elog!(INFO, "TVIEW table created. To convert to TVIEW, run: SELECT pg_tviews_convert_existing_table('{}');", table_name);

    Ok(())
}
```

**Exact Changes**:
1. Remove the `convert_to_tview()?;` call
2. Keep only structure validation
3. Add informational log message

#### Step 1.2.4: Rebuild Docker Image (20 minutes)
```bash
# Rebuild with Rust changes
docker build -t pg_tviews .

# Restart containers with new image
docker compose down -v
docker compose up -d

# Verify new image is running
docker compose ps
```

#### Step 1.2.5: Test Event Trigger Behavior (15 minutes)
```bash
# Create a test TVIEW table (should trigger event but not auto-convert)
psql -d pg_tviews_benchmark -c "
CREATE TABLE benchmark.tv_test AS
SELECT id, data FROM benchmark.tv_product LIMIT 5;
"

# Check logs for the informational message
# (Check Docker logs or psql logs)

# Verify table exists but is not yet a TVIEW
psql -d pg_tviews_benchmark -c "
SELECT schemaname, tablename FROM pg_tables WHERE tablename = 'tv_test';
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';  -- Should be empty
"

# Manually convert
psql -d pg_tviews_benchmark -c "
SELECT pg_tviews_convert_existing_table('benchmark.tv_test');
"

# Verify conversion worked
psql -d pg_tviews_benchmark -c "
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';  -- Should have entry
"
```

#### Step 1.2.6: Verification Checklist
- [ ] Event trigger validates structure without crashing
- [ ] No SPI transaction errors during CREATE TABLE AS SELECT
- [ ] Manual conversion works: `SELECT pg_tviews_convert_existing_table('table')`
- [ ] TVIEW metadata created after manual conversion
- [ ] Docker rebuild successful with new Rust code

**Success Criteria Met**: Manual TVIEW conversion succeeds

---

### Task 1.3: Fix Scenarios Variable Quoting
**Priority**: P1 (blocks scenario benchmarks)
**Time Estimate**: 10 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`

#### Step 1.3.1: Identify the Issue (2 minutes)
```bash
# Check line 131 in run_benchmarks.sh
sed -n '125,135p' test/sql/comprehensive_benchmarks/run_benchmarks.sh | cat -n
```

**Current Code (line 131)**:
```bash
$PSQL -v data_scale="'$scale'" -f "scenarios/${scenario}_benchmarks.sql"
```

**Problem**: Extra quotes around `$scale` - should match data generation pattern (line 127)

#### Step 1.3.2: Apply the Fix (3 minutes)
**Change line 131 from**:
```bash
$PSQL -v data_scale="'$scale'" -f "scenarios/${scenario}_benchmarks.sql"
```

**To**:
```bash
$PSQL -v data_scale="$scale" -f "scenarios/${scenario}_benchmarks.sql"
```

**Rationale**: Match the pattern used for data generation (line 127) which works correctly.

#### Step 1.3.3: Verification Checklist
- [ ] Scenarios line matches data generation pattern
- [ ] No extra quotes around `$scale` variable
- [ ] File saved with correct syntax

**Success Criteria Met**: Scenarios execute without variable errors

---

### Task 1.4: End-to-End Verification
**Priority**: P0 (validates all fixes)
**Time Estimate**: 30 minutes
**Risk**: Low
**Dependencies**: Tasks 1.1, 1.2, 1.3 completed

#### Step 1.4.1: Clean Environment (5 minutes)
```bash
# Stop and clean containers
docker compose down -v

# Start fresh containers
docker compose up -d

# Wait for database to be ready
sleep 10

# Verify containers are running
docker compose ps
```

#### Step 1.4.2: Run Full Benchmark (15 minutes)
```bash
# Navigate to benchmark directory
cd test/sql/comprehensive_benchmarks

# Run benchmark with logging
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/benchmark_verification.log

# Check for errors
echo "=== ERRORS FOUND ==="
grep -i "error" /tmp/benchmark_verification.log | grep -v "0 errors"

echo "=== SUCCESS MESSAGES ==="
grep -i "success\|complete" /tmp/benchmark_verification.log
```

#### Step 1.4.3: Structured Validation (10 minutes)
```bash
# Schema verification
echo "=== SCHEMA VERIFICATION ==="
psql -d pg_tviews_benchmark -c "
SELECT schemaname, tablename
FROM pg_tables
WHERE schemaname = 'benchmark'
ORDER BY tablename;
"

# Data verification
echo "=== DATA VERIFICATION ==="
psql -d pg_tviews_benchmark -c "
SELECT 'tb_category' as table, COUNT(*) FROM benchmark.tb_category
UNION ALL SELECT 'tb_product', COUNT(*) FROM benchmark.tb_product
UNION ALL SELECT 'tv_product', COUNT(*) FROM benchmark.tv_product;
"

# TVIEW verification
echo "=== TVIEW VERIFICATION ==="
psql -d pg_tviews_benchmark -c "
SELECT table_name, created_at FROM pg_tviews_metadata ORDER BY table_name;
"

# Manual conversion test
echo "=== MANUAL CONVERSION TEST ==="
psql -d pg_tviews_benchmark -c "
-- Create test table
CREATE TABLE benchmark.tv_manual_test AS
SELECT id, data FROM benchmark.tv_product LIMIT 3;

-- Manually convert
SELECT pg_tviews_convert_existing_table('benchmark.tv_manual_test');

-- Verify
SELECT table_name FROM pg_tviews_metadata WHERE table_name = 'tv_manual_test';
"
```

#### Step 1.4.4: Success Criteria Checklist
- [ ] Schema loads in 'benchmark' schema (not 'public')
- [ ] tb_category has >0 rows
- [ ] tb_product has >0 rows
- [ ] tv_product table exists
- [ ] Manual TVIEW conversion works
- [ ] pg_tviews_metadata populated after manual conversion
- [ ] Benchmark scenarios execute without errors
- [ ] Results written to benchmark_results table
- [ ] No "relation does not exist" errors
- [ ] No "syntax error at or near :" errors
- [ ] No SPI transaction errors

**Success Criteria Met**: All benchmarks run successfully with manual TVIEW conversion workflow

---

### Task 1.5: Commit Changes
**Priority**: P0 (preserve working state)
**Time Estimate**: 15 minutes
**Risk**: Low
**Dependencies**: Task 1.4 passes

#### Step 1.5.1: Pre-Commit Verification (5 minutes)
```bash
# Run tests one more time to ensure everything still works
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small > /tmp/final_verification.log 2>&1

# Check no new errors introduced
grep -i "error" /tmp/final_verification.log | grep -v "0 errors"
```

#### Step 1.5.2: Create Separate Commits (10 minutes)
```bash
# Commit 1: Data generation fix
git add test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql
git commit -m "fix(benchmarks): Fix psql variable interpolation in data generation

- Change :data_scale to :'data_scale' for string interpolation
- Fixes 'syntax error at or near :' at line 151
- Verified with manual psql test and full data generation"

# Commit 2: TVIEW conversion architecture fix
git add src/event_trigger.rs
git commit -m "fix(tview): Disable auto-conversion in event trigger [ARCHITECTURE]

- Event triggers can't use SPI for catalog queries (PG limitation)
- Changed to validate-only mode in on_create_table_as_select_end()
- Users must manually call pg_tviews_convert_existing_table()
- Prevents 'SPI error: Transaction' during DDL events
- Documented manual conversion workflow in logs"

# Commit 3: Scenarios variable fix
git add test/sql/comprehensive_benchmarks/run_benchmarks.sh
git commit -m "fix(benchmarks): Fix psql variable quoting in scenarios

- Match data generation pattern (line 127)
- Remove extra quotes: data_scale='$scale' → data_scale=$scale
- Consistent variable passing across script"

# Commit 4: Rebuild verification
git commit --allow-empty -m "test(benchmarks): Verify all fixes work together

- Full benchmark run successful with manual TVIEW conversion
- Schema loads correctly in benchmark schema
- Data generation works for all scales
- Scenarios execute without variable errors
- Manual conversion workflow validated"
```

#### Step 1.5.3: Verification Checklist
- [ ] Each fix in separate commit (clean history)
- [ ] Commit messages explain WHY, not just WHAT
- [ ] Architectural decisions documented in commit message
- [ ] All tests pass after each commit
- [ ] No uncommitted changes remain

**Success Criteria Met**: All 4 commits pushed with clean history

---

## 🟡 Phase 2: Documentation & Polish

### Task 2.1: Documentation Updates
**Priority**: Medium
**Time Estimate**: 30 minutes
**Risk**: Low
**Files Modified**: `README.md`, `docs/benchmark-troubleshooting.md` (new)

#### Step 2.1.1: Update README with Manual Conversion Workflow (15 minutes)
**Add to README.md** (find the TVIEW section):

```markdown
## TVIEW Creation Workflow

Due to PostgreSQL event trigger limitations, TVIEW tables are not automatically converted during `CREATE TABLE AS SELECT`.

### Manual Conversion Process

1. Create your TVIEW table:
```sql
CREATE TABLE tv_my_entity AS
SELECT
    id,           -- UUID (required)
    data,         -- JSONB (required)
    -- Optional optimization columns:
    pk_entity,    -- INTEGER primary key
    fk_parent,    -- INTEGER foreign key
    parent_id,    -- UUID foreign key
    path          -- LTREE for hierarchies
FROM v_my_entity;
```

2. Manually convert to TVIEW:
```sql
SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

3. Verify conversion:
```sql
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_my_entity';
```

### Event Trigger Behavior

Event triggers now only validate TVIEW structure. They will log:
```
INFO: TVIEW table created. To convert to TVIEW, run: SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

### Future: Automatic Conversion

Background worker support for automatic conversion is planned for a future release.
```

#### Step 2.1.2: Create Benchmark Troubleshooting Guide (15 minutes)
**Create new file**: `docs/benchmark-troubleshooting.md`

```markdown
# Benchmark Troubleshooting Guide

## Common Issues and Solutions

### 1. "syntax error at or near :"

**Symptom**: `psql:data/01_ecommerce_data.sql:151: ERROR: syntax error at or near ":"`

**Cause**: Incorrect psql variable interpolation syntax

**Fix**: Change `:data_scale` to `:'data_scale'` in SQL files for string interpolation

**Verification**:
```bash
psql -v data_scale="small" -c "SELECT :'data_scale' AS value;"
# Should return: small
```

### 2. "SPI error: Transaction"

**Symptom**: `ERROR: Failed to convert table to TVIEW: SPI error: Transaction Query: Unknown`

**Cause**: Event triggers cannot use SPI during DDL events

**Fix**: Manually call `pg_tviews_convert_existing_table()` after CREATE TABLE AS SELECT

**Example**:
```sql
CREATE TABLE tv_test AS SELECT id, data FROM v_test;
SELECT pg_tviews_convert_existing_table('tv_test');
```

### 3. "relation does not exist"

**Symptom**: Table not found during benchmark execution

**Cause**: Missing schema qualification or search_path issues

**Fix**: Use schema-qualified names: `benchmark.table_name`

### 4. Variable Quoting Issues

**Symptom**: Scenarios fail with variable errors

**Fix**: Ensure consistent quoting in `run_benchmarks.sh`:
```bash
# Correct
$PSQL -v data_scale="$scale" -f scenarios/file.sql

# Wrong
$PSQL -v data_scale="'$scale'" -f scenarios/file.sql
```

## Diagnostic Commands

### Check Schema State
```bash
psql -d pg_tviews_benchmark -c "
SELECT schemaname, tablename FROM pg_tables
WHERE schemaname IN ('benchmark', 'public')
ORDER BY schemaname, tablename;
"
```

### Check Data Loading
```bash
psql -d pg_tviews_benchmark -c "
SELECT 'tb_category' as table, COUNT(*) FROM benchmark.tb_category
UNION ALL SELECT 'tb_product', COUNT(*) FROM benchmark.tb_product;
"
```

### Check TVIEW Status
```bash
psql -d pg_tviews_benchmark -c "
SELECT * FROM pg_tviews_metadata ORDER BY table_name;
"
```

### Test Manual Conversion
```bash
psql -d pg_tviews_benchmark -c "
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');
"
```
```

#### Step 2.1.3: Verification Checklist
- [ ] README updated with manual conversion workflow
- [ ] Troubleshooting guide created
- [ ] Examples provided for common issues
- [ ] Diagnostic commands documented

**Success Criteria Met**: Documentation updated

---

### Task 2.2: Add Diagnostic Logging
**Priority**: Low
**Time Estimate**: 30 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`

#### Step 2.2.1: Add Logging Functions (10 minutes)
**Add to run_benchmarks.sh** (after the initial variable setup):

```bash
# Add logging functions
log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" >&2
}

log_info() {
    log "INFO: $1"
}

log_error() {
    log "ERROR: $1"
}

log_success() {
    log "SUCCESS: $1"
}
```

#### Step 2.2.2: Add Diagnostic Logging Throughout Script (20 minutes)
**Add logging after major steps**:

```bash
# After schema loading
log_info "Loading schema..."
$PSQL -f schemas/01_ecommerce_schema.sql
if [ $? -eq 0 ]; then
    log_success "Schema loaded successfully"
else
    log_error "Schema loading failed"
    exit 1
fi

# After data generation
log_info "Generating data (scale=$scale)..."
$PSQL -v data_scale="$scale" -f data/01_ecommerce_data.sql
if [ $? -eq 0 ]; then
    log_success "Data generation completed"
else
    log_error "Data generation failed"
    exit 1
fi

# After data verification
log_info "Verifying data loaded..."
DATA_COUNT=$($PSQL -c "SELECT COUNT(*) FROM benchmark.tb_product;" -t)
if [ "$DATA_COUNT" -gt 0 ]; then
    log_success "Data verification passed: $DATA_COUNT rows in tb_product"
else
    log_error "Data verification failed: no rows in tb_product"
    exit 1
fi

# After scenario execution
log_info "Running scenario: $scenario"
$PSQL -v data_scale="$scale" -f "scenarios/${scenario}_benchmarks.sql"
if [ $? -eq 0 ]; then
    log_success "Scenario $scenario completed successfully"
else
    log_error "Scenario $scenario failed"
    exit 1
fi
```

#### Step 2.2.3: Verification Checklist
- [ ] Logging functions added to script
- [ ] Major steps have diagnostic logging
- [ ] Error handling improved with exit codes
- [ ] Success/failure clearly logged

**Success Criteria Met**: Diagnostic logging added

---

## 🧪 Testing Strategy

### Pre-Implementation Testing (RED Phase)
**Goal**: Confirm issues exist before fixing

1. **Run baseline benchmark**:
   ```bash
   cd test/sql/comprehensive_benchmarks
   ./run_benchmarks.sh --scale small 2>&1 | tee /tmp/baseline_errors.log
   grep -i "error" /tmp/baseline_errors.log
   ```
   **Expected**: Data generation error + TVIEW conversion error

2. **Test manual TVIEW conversion**:
   ```bash
   psql -d pg_tviews_benchmark -c "SELECT pg_tviews_convert_existing_table('benchmark.tv_product');"
   ```
   **Expected**: Success (confirms SPI works outside events)

### Post-Fix Testing (GREEN Phase)
**Goal**: Verify each fix independently

1. **Test data generation fix**:
   ```bash
   psql -d pg_tviews_benchmark -v data_scale="small" -f data/01_ecommerce_data.sql
   ```
   **Expected**: No syntax errors

2. **Test TVIEW conversion fix**:
   ```bash
   # Create test table
   psql -d pg_tviews_benchmark -c "CREATE TABLE benchmark.tv_test AS SELECT id, data FROM benchmark.tv_product LIMIT 5;"
   # Should not auto-convert but not crash
   psql -d pg_tviews_benchmark -c "SELECT pg_tviews_convert_existing_table('benchmark.tv_test');"
   ```
   **Expected**: Manual conversion succeeds

3. **Test scenarios fix**:
   ```bash
   # After data generation works, test scenarios
   ./run_benchmarks.sh --scale small --scenario product_queries
   ```
   **Expected**: No variable quoting errors

### Integration Testing (QA Phase)
**Goal**: Full end-to-end verification

```bash
# Clean environment
docker compose down -v
docker compose up -d

# Full test run
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/full_test.log

# Verify all success criteria
grep -E "(error|success|complete)" /tmp/full_test.log
```

### Regression Testing (GREENFIELD Phase)
**Goal**: Ensure fixes don't break existing functionality

1. **Test other scales**: medium, large
2. **Test multiple scenarios**: All scenario files
3. **Test edge cases**: Empty data, malformed tables
4. **Performance check**: Compare benchmark times

---

## 🛡️ Safety & Rollback

### Pre-Flight Checklist
```bash
# BEFORE starting:
- [ ] Create backup branch: git checkout -b benchmark-fixes-20251214
- [ ] Test baseline: ./run_benchmarks.sh --scale small > /tmp/baseline.log 2>&1
- [ ] Document current error state
- [ ] Ensure Docker has sufficient disk space: df -h
- [ ] Backup important data if any
```

### Rollback Strategy

**If Task 1.1 fails**:
```bash
git checkout test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql
# Re-test data generation
```

**If Task 1.2 fails**:
```bash
git checkout src/event_trigger.rs
docker build -t pg_tviews .
docker compose down -v && docker compose up -d
```

**If Task 1.3 fails**:
```bash
git checkout test/sql/comprehensive_benchmarks/run_benchmarks.sh
```

**Nuclear rollback**:
```bash
git checkout dev  # or main branch
git reset --hard origin/dev
docker compose down -v
docker system prune -af --volumes  # WARNING: Deletes all data
```

### Safety Guardrails
- ✅ **Test each fix independently** before proceeding
- ✅ **Commit after each successful fix** (atomic changes)
- ✅ **Run verification after each change**
- ✅ **Document architectural decisions**
- ✅ **Keep fixes minimal** (no scope creep)
- ❌ **Don't combine multiple fixes** in one commit
- ❌ **Don't skip verification steps**
- ❌ **Don't modify Rust code** without rebuilding Docker
- ❌ **Don't push** without full verification

---

## 📊 Success Metrics

### Phase 1 Complete When:
- [ ] Data generation works for all scales (small, medium, large)
- [ ] Manual TVIEW conversion succeeds without SPI errors
- [ ] Benchmark scenarios execute without variable errors
- [ ] Full benchmark run completes successfully
- [ ] All 4 commits pushed with clean git history
- [ ] Docker rebuild successful with Rust changes

### Phase 2 Complete When:
- [ ] README updated with manual conversion workflow
- [ ] Benchmark troubleshooting guide created
- [ ] Diagnostic logging added to scripts
- [ ] All documentation commits pushed

### Overall Success:
- [ ] `./run_benchmarks.sh --scale small` runs without errors
- [ ] Manual TVIEW conversion workflow documented and working
- [ ] Clean git history with separate commits for each fix
- [ ] Junior engineers can follow this plan independently

---

## 🚀 Quick Reference

### For Junior Engineers - Start Here:

1. **Read this entire plan** (15 minutes)
2. **Run baseline test** to confirm issues exist (5 minutes)
3. **Start with Task 1.1** (data generation fix - 45 minutes)
4. **Test each fix** before moving to next
5. **Commit after each** successful fix
6. **Ask for help** if stuck on any step

### Emergency Contacts:
- **If Docker issues**: Check `docker compose ps` and logs
- **If psql issues**: Test basic connection first
- **If Rust issues**: Verify `docker build` completed successfully
- **If git issues**: Check `git status` and ask before force operations

### Time Checkpoints:
- **1 hour**: Task 1.1 completed and tested
- **2 hours**: Tasks 1.1-1.3 completed
- **2.5 hours**: Task 1.4 (E2E verification) passed
- **3 hours**: Phase 1 commits complete
- **4 hours**: Phase 2 documentation complete

**Remember**: This plan is designed to be foolproof. Follow each step exactly, test thoroughly, and commit frequently. If anything is unclear, ask before proceeding.

---

*Last Updated: 2025-12-14*
*Plan Status: Ready for junior engineer implementation*
*Estimated Time: 3-4 hours*
*Risk Level: Medium (but well-mitigated)*</content>
<parameter name="filePath">.phases/20251214_BENCHMARK_FIXES_IMPLEMENTATION_PLAN.md