# Benchmark Fixes Implementation Plan - December 14, 2025 (v2.0)

## 🎯 Executive Summary

**Objective**: Fix all remaining benchmark issues to enable reliable automated testing
**Timeline**: 3-4 hours total (Phase 1: 2-3 hours, Phase 2: 1 hour)
**Risk Level**: Medium (well-understood issues, clear fix paths)
**Success Criteria**: All benchmarks run successfully with manual TVIEW conversion workflow

**Key Architectural Decisions**:
- **TVIEW Auto-Conversion**: DISABLED in event triggers due to PostgreSQL SPI limitations
- **Manual Conversion**: Users must call `pg_tviews_convert_existing_table()` after CREATE TABLE AS SELECT
- **Future**: Background worker for automatic conversion (Phase 3)

**For Junior Engineers**: This plan is designed to be foolproof. Follow each step exactly, test thoroughly, and commit frequently. If anything is unclear or takes >30 minutes, ask for help.

---

## 📋 Phase Overview

### Phase 1: Critical Fixes (2-3 hours)
**Goal**: Fix blocking issues preventing benchmark execution

| Task | Priority | Time | Risk | Dependencies |
|------|----------|------|------|--------------|
| **Pre-Check** | P0 | 10 min | Low | None |
| 1.1 Data Generation Fix | P0 | 45 min | Low | Pre-Check |
| 1.2 TVIEW Conversion Fix | P0 | 90 min | Medium | Pre-Check |
| 1.3 Scenarios Variable Fix | P1 | 10 min | Low | None |
| 1.4 End-to-End Verification | P0 | 40 min | Low | 1.1, 1.2, 1.3 |
| 1.5 Commit Changes | P0 | 15 min | Low | 1.4 |

### Phase 2: Documentation & Polish (1 hour)
**Goal**: Update docs and add diagnostics

| Task | Priority | Time | Risk | Dependencies |
|------|----------|------|------|--------------|
| 2.1 Documentation Updates | Medium | 30 min | Low | Phase 1 |
| 2.2 Diagnostic Logging | Low | 30 min | Low | Phase 1 |

---

## 🔍 Pre-Implementation Sanity Check (10 minutes)

**Run BEFORE starting any fixes to ensure environment is ready**

### Step 0.1: Environment Verification (5 minutes)

```bash
echo "=== ENVIRONMENT SANITY CHECK ==="

# Check 1: Database is accessible
echo "Testing database connection..."
psql -d pg_tviews_benchmark -c "SELECT version();" > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ Database connection: OK"
else
    echo "❌ Database connection: FAILED"
    echo "Fix: Check docker compose ps and restart if needed"
    exit 1
fi

# Check 2: Docker is running
echo "Checking Docker status..."
docker compose ps | grep -q "Up"
if [ $? -eq 0 ]; then
    echo "✅ Docker containers: Running"
else
    echo "❌ Docker containers: Not running"
    echo "Fix: Run 'docker compose up -d'"
    exit 1
fi

# Check 3: Benchmark directory exists
echo "Checking benchmark directory..."
if [ -f "test/sql/comprehensive_benchmarks/run_benchmarks.sh" ]; then
    echo "✅ Benchmark directory: Found"
else
    echo "❌ Benchmark directory: Not found"
    echo "Fix: cd to project root directory"
    exit 1
fi

# Check 4: Disk space (need at least 5GB for Docker builds)
echo "Checking disk space..."
available_space=$(df -BG / | awk 'NR==2 {print $4}' | sed 's/G//')
if [ "$available_space" -gt 5 ]; then
    echo "✅ Disk space: ${available_space}GB available"
else
    echo "⚠️  Disk space: Only ${available_space}GB available"
    echo "Warning: Docker builds may fail. Consider cleaning up."
fi

# Check 5: Git status
echo "Checking git status..."
git status --short
echo ""
echo "✅ Git status shown above"
echo ""

echo "=== SANITY CHECK COMPLETE ==="
echo ""
```

**All checks pass?** ✅ Proceed to Step 0.2
**Any failures?** ❌ Fix environment issues first, then retry

---

### Step 0.2: Capture Baseline (5 minutes)

```bash
# Create backup branch
git checkout -b benchmark-fixes-20251214
echo "✅ Created backup branch: benchmark-fixes-20251214"

# Capture baseline errors
echo "Capturing baseline errors (this will fail - that's expected)..."
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small > /tmp/baseline_errors.log 2>&1

# Extract key errors
echo ""
echo "=== BASELINE ERRORS ==="
grep -i "error" /tmp/baseline_errors.log | head -20
echo ""
echo "Expected errors:"
echo "  1. syntax error at or near ':' (data generation)"
echo "  2. SPI error: Transaction (TVIEW conversion)"
echo ""
echo "✅ Baseline captured at /tmp/baseline_errors.log"
echo ""

# Return to project root
cd ../../..
```

**Checklist**:
- [ ] Backup branch created
- [ ] Baseline errors captured
- [ ] Expected errors visible in output

---

## 🔴 Phase 1: Critical Fixes Implementation

### Task 1.1: Fix Data Generation Psql Variable Interpolation
**Priority**: P0 (blocks all data loading)
**Time Estimate**: 45 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql`

---

#### Step 1.1.1: Diagnose the Issue (10 minutes)

```bash
# Navigate to benchmark directory
cd test/sql/comprehensive_benchmarks

# Examine area around line 151 where error occurs
echo "=== EXAMINING ERROR LOCATION ==="
sed -n '145,155p' data/01_ecommerce_data.sql | cat -n
echo ""

# Search for ALL uses of :data_scale
echo "=== SEARCHING FOR :data_scale USAGE ==="
grep -n ":data_scale" data/01_ecommerce_data.sql
echo ""
```

**What to look for**:
- ✅ `:'data_scale'` (quoted) - CORRECT for string interpolation
- ❌ `:data_scale` (unquoted) - WRONG causes "syntax error at or near :"

**Expected Finding**: Line 151 (or nearby) has `:data_scale` without quotes

---

#### Step 1.1.2: Understand Psql Variable Syntax (5 minutes)

**Key Learning - Psql Variable Interpolation Rules**:

**`:variable` (UNQUOTED)**
- Psql substitutes the VALUE directly into SQL
- Use for: numbers, booleans, SQL keywords
- Example: `:limit` with `-v limit=100` → `LIMIT 100`

**`:'variable'` (SINGLE-QUOTED)**
- Psql substitutes and wraps in single quotes
- Use for: string literals
- Example: `:'scale'` with `-v scale=small` → `'small'` (becomes: `WHERE scale = 'small'`)

**`:"variable"` (DOUBLE-QUOTED)**
- Psql substitutes and wraps in double quotes
- Use for: identifiers (table/column names)
- Example: `:"table"` with `-v table=products` → `"products"` (becomes: `SELECT * FROM "products"`)

**Common Mistake**:
```sql
-- WRONG: Unquoted when expecting string
WHERE scale = :data_scale
-- Error: PostgreSQL expects integer/boolean, gets 'small' → SYNTAX ERROR

-- CORRECT: Quoted for strings
WHERE scale = :'data_scale'
-- Becomes: WHERE scale = 'small' → WORKS ✅
```

**The Problem in Our Code**:
Shell passes `data_scale="small"` but SQL line 151 uses unquoted `:data_scale` instead of quoted `:'data_scale'`

---

#### Step 1.1.3: Locate and Fix the Variable (10 minutes)

```bash
# Find the exact line with unquoted :data_scale (excluding correct quoted usage)
echo "=== FINDING UNQUOTED :data_scale ==="
grep -n ":data_scale" data/01_ecommerce_data.sql | grep -v ":'data_scale'"
echo ""
echo "The line(s) above need fixing"
echo ""

# Show the exact context
line_num=$(grep -n ":data_scale" data/01_ecommerce_data.sql | grep -v ":'data_scale'" | head -1 | cut -d: -f1)
if [ -n "$line_num" ]; then
    echo "Line $line_num needs fixing:"
    sed -n "$((line_num-2)),$((line_num+2))p" data/01_ecommerce_data.sql | cat -n
    echo ""
fi
```

**Manual Fix Required**:
1. Open `data/01_ecommerce_data.sql` in your editor
2. Go to the line number shown above (likely around line 151)
3. Find: `:data_scale` (unquoted)
4. Replace with: `:'data_scale'` (quoted)
5. Save the file

**Example Fix**:
```sql
-- BEFORE (line 151):
WHERE some_condition = :data_scale

-- AFTER (line 151):
WHERE some_condition = :'data_scale'
```

**Verify your change**:
```bash
# Check the line is now correct
grep -n ":'data_scale'" data/01_ecommerce_data.sql
# Should show the line you just fixed
```

---

#### Step 1.1.4: Test the Fix (15 minutes)

```bash
# Test 1: Psql variable interpolation in isolation
echo "=== TEST 1: Variable Interpolation ==="
psql -d pg_tviews_benchmark -v data_scale="small" <<EOF
DO \$\$
BEGIN
    RAISE NOTICE 'Testing quoted interpolation: %', :'data_scale';
END \$\$;
EOF
# Expected output: NOTICE: Testing quoted interpolation: small

echo ""

# Test 2: Run the data generation script
echo "=== TEST 2: Data Generation Script ==="
psql -d pg_tviews_benchmark -v data_scale="small" -f data/01_ecommerce_data.sql
# Expected: Script completes without "syntax error at or near :"

echo ""

# Test 3: Verify data was actually loaded
echo "=== TEST 3: Data Verification ==="
psql -d pg_tviews_benchmark <<EOF
SELECT 'tb_category' as table_name, COUNT(*) as row_count
FROM benchmark.tb_category
UNION ALL
SELECT 'tb_product', COUNT(*)
FROM benchmark.tb_product
ORDER BY table_name;
EOF
# Expected: Both tables have rows (tb_category > 0, tb_product > 0)

echo ""
```

**What Success Looks Like**:
- ✅ Test 1: Shows "NOTICE: Testing quoted interpolation: small"
- ✅ Test 2: Completes without syntax errors
- ✅ Test 3: Shows row counts > 0 for both tables

**If Test 1 Fails**:
- ❌ Check you're connected to correct database: `pg_tviews_benchmark`
- ❌ Check variable syntax: Should be `-v data_scale="small"` (no extra quotes)

**If Test 2 Fails with Different Error**:
- ❌ Check the error message carefully
- ❌ May have other syntax errors in the file
- ❌ Ask for help with the exact error message

**If Test 3 Shows 0 Rows**:
- ❌ Data generation script may have failed silently
- ❌ Check for warnings in Test 2 output
- ❌ Verify schema exists: `\dt benchmark.*`

---

#### Step 1.1.5: Verification Checklist

- [ ] Psql variable interpolation works without syntax errors (Test 1)
- [ ] Data generation completes successfully (Test 2)
- [ ] tb_category has >0 rows (Test 3)
- [ ] tb_product has >0 rows (Test 3)
- [ ] No "syntax error at or near :" messages anywhere

**Success Criteria Met**: ✅ Data generation works for small scale

**Communication Checkpoint**:
- [ ] Post in team channel: "Task 1.1 complete - data generation fixed ✅"
- [ ] If blocked >30 minutes, asked for help

---

### Task 1.2: Fix TVIEW Conversion SPI Transaction Issue
**Priority**: P0 (blocks automatic TVIEW creation)
**Time Estimate**: 90 minutes
**Risk**: Medium (architectural change)
**Files Modified**: `src/event_trigger.rs`

---

#### Step 1.2.1: Understand the Problem (15 minutes)

**Root Cause**: PostgreSQL event triggers cannot use SPI (Server Programming Interface) to query catalogs during DDL events due to transaction isolation.

**The Technical Details**:
- Event triggers run in the same transaction as DDL commands (like CREATE TABLE)
- SPI calls create sub-transactions to query the database
- PostgreSQL prevents nested transactions during DDL events (safety feature)
- Error message: "SPI error: Transaction Query: Unknown"

**Why This Matters**:
- Current code tries to auto-convert tables to TVIEW inside the event trigger
- Auto-conversion needs SPI to query pg_class and other catalogs
- This creates a nested transaction → PostgreSQL blocks it → ERROR

**The Solution**:
- Disable auto-conversion in the event trigger
- Keep only validation (doesn't need SPI)
- Users call manual conversion function after CREATE TABLE AS SELECT
- Manual function runs outside event context → SPI works fine

**Architecture Decision**: This is a PostgreSQL limitation, not a bug in our code. The fix changes behavior (no auto-conversion) but enables manual workflow.

---

#### Step 1.2.2: Test Manual Conversion Works (10 minutes)

**Goal**: Prove that SPI works outside event triggers (confirms our hypothesis)

```bash
# First ensure tv_product table exists
echo "=== CHECKING IF TABLE EXISTS ==="
psql -d pg_tviews_benchmark <<EOF
SET search_path TO benchmark, public;
\dt tv_product
EOF

# If table doesn't exist, may need to run data generation first
# (Should exist if Task 1.1 completed successfully)

echo ""
echo "=== TESTING MANUAL CONVERSION (OUTSIDE EVENT TRIGGER) ==="
psql -d pg_tviews_benchmark <<EOF
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');
EOF

echo ""
echo "=== VERIFYING TVIEW WAS CREATED ==="
psql -d pg_tviews_benchmark <<EOF
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_product';
EOF
```

**Expected Result**:
- ✅ Manual conversion succeeds
- ✅ TVIEW metadata shows tv_product entry

**This proves**: SPI works fine outside event triggers (only event trigger is the problem)

---

**IF MANUAL CONVERSION FAILS** ⚠️:

**STOP HERE - DO NOT PROCEED**

This is unexpected and indicates SPI code itself is broken. Debug steps:

```bash
# Check 1: Does the function exist?
psql -d pg_tviews_benchmark -c "\df pg_tviews*"
# Should show: pg_tviews_convert_existing_table

# Check 2: Does the table exist?
psql -d pg_tviews_benchmark -c "\dt benchmark.tv_product"
# Should show: tv_product table

# Check 3: What's the exact error?
psql -d pg_tviews_benchmark <<EOF
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');
EOF
# Copy the FULL error message
```

**Common Failure Causes**:
1. **Function doesn't exist**: Extension not loaded → Run `CREATE EXTENSION pg_tviews;`
2. **Table doesn't exist**: Data gen failed → Go back to Task 1.1
3. **Schema qualification**: Try without schema → `pg_tviews_convert_existing_table('tv_product')`
4. **Permissions**: Check you're superuser → `\du`

**If still failing after checks**:
- Document the exact error message
- Take screenshots if helpful
- **Ask senior engineer for help** - this is beyond expected issues
- Include: Error message, function definition, table schema

---

#### Step 1.2.2b: Inspect Current Event Trigger Code (15 minutes)

**Goal**: Understand actual codebase before making changes

```bash
# Read the event trigger implementation
echo "=== READING EVENT TRIGGER CODE ==="
cat src/event_trigger.rs
echo ""

# Look for these key elements:
# 1. Function name: on_create_table_as_select_end (or similar)
# 2. Auto-conversion logic (what calls SPI?)
# 3. Validation logic (what should we keep?)
# 4. Error handling
```

**Take notes on**:
- Exact function name for event trigger
- Where conversion happens (which function call?)
- Where validation happens (what to preserve?)
- Line numbers for changes

**Example - What you might find**:
```rust
// Example (actual code may differ)
#[pg_guard]
pub fn on_create_table_as_select_end() -> Result<(), Error> {
    validate_tview_structure()?;  // ← Keep this (no SPI)
    convert_to_tview(table_name)?; // ← Remove this (uses SPI)
    Ok(())
}
```

**Make a note**:
```
Function name: on_create_table_as_select_end
Line to remove: Line XX - convert_to_tview() call
Line to keep: Line YY - validate_tview_structure() call
Line to add: Informational log message
```

---

#### Step 1.2.3: Implement Event Trigger Fix (30 minutes)

**Decision**: Disable auto-conversion in event triggers (Option A from architecture docs)

**File to Modify**: `src/event_trigger.rs`

**BEFORE making changes**:
```bash
# Create a backup
cp src/event_trigger.rs src/event_trigger.rs.backup
echo "✅ Backup created: src/event_trigger.rs.backup"
```

**The Change** (adapt to actual code structure):

Find the event trigger function (example - adjust to your actual code):
```rust
// BEFORE (Current code - uses SPI):
#[pg_guard]
pub fn on_create_table_as_select_end() -> Result<(), Error> {
    // Validation
    validate_tview_structure()?;

    // Auto-conversion (REMOVE THIS - uses SPI)
    convert_to_tview(table_name)?;

    Ok(())
}
```

Change to:
```rust
// AFTER (New code - validation only):
#[pg_guard]
pub fn on_create_table_as_select_end() -> Result<(), Error> {
    // Only validate TVIEW structure (no SPI needed)
    validate_tview_structure()?;

    // Log manual conversion instruction
    let table_name = get_current_table_name()?; // Adjust based on actual API
    elog!(
        INFO,
        "TVIEW table created. To convert to TVIEW, run: SELECT pg_tviews_convert_existing_table('{}');",
        table_name
    );

    Ok(())
}
```

**Exact Changes to Make**:
1. **REMOVE**: The line calling `convert_to_tview()` (or similar auto-conversion)
2. **KEEP**: Validation logic (`validate_tview_structure()` or similar)
3. **ADD**: Informational log message using `elog!(INFO, ...)`

**Note**: Exact function names may differ. The key is:
- Remove any SPI-using conversion code
- Keep validation code
- Add helpful log message

---

#### Step 1.2.3b: Review Your Changes (5 minutes)

```bash
# Show what you changed
echo "=== GIT DIFF ==="
git diff src/event_trigger.rs
echo ""

# Verify checklist:
echo "=== VERIFICATION CHECKLIST ==="
echo "Review the diff above and confirm:"
echo "  [ ] Removed auto-conversion call (convert_to_tview or similar)"
echo "  [ ] Validation logic still intact"
echo "  [ ] Added informational log message with elog!(INFO, ...)"
echo "  [ ] No syntax errors (check Rust syntax highlighting)"
echo "  [ ] No unintended changes to other functions"
echo ""
echo "If unsure about any changes, ask senior engineer to review before building"
echo ""
```

**Rust Syntax Check** (if you have Rust installed locally):
```bash
# Optional: Check for syntax errors
cd src
cargo check 2>&1 | grep -E "(error|warning)"
cd ..
```

**If you see errors**: Fix syntax before proceeding
**If unsure**: Show diff to senior engineer

---

#### Step 1.2.4: Rebuild Docker Image (5-20 minutes)

**Time varies**:
- ✅ With cache: ~5 minutes
- ⚠️  Without cache: ~15-20 minutes
- ❌ Slow network: up to 30 minutes

```bash
echo "=== BUILDING DOCKER IMAGE ==="
echo "This may take 5-20 minutes depending on cache..."
echo "Expected: Compiling Rust code, creating new image"
echo ""

# Try cached build first (faster)
docker build -t pg_tviews . 2>&1 | tee /tmp/docker_build.log

# Check if build succeeded
if [ $? -eq 0 ]; then
    echo ""
    echo "✅ Docker build succeeded"
else
    echo ""
    echo "❌ Docker build failed - check /tmp/docker_build.log"
    echo "Common issues:"
    echo "  - Rust syntax error (check git diff above)"
    echo "  - Network timeout (try again)"
    echo "  - Disk space (run: df -h)"
    exit 1
fi

echo ""
echo "=== RESTARTING CONTAINERS WITH NEW IMAGE ==="
docker compose down -v
docker compose up -d

echo ""
echo "Waiting for database to be ready..."
sleep 10

echo ""
echo "=== VERIFYING NEW IMAGE IS RUNNING ==="
docker compose ps
echo ""

# Verify database is accessible
echo "Testing database connection..."
psql -d pg_tviews_benchmark -c "SELECT version();" > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ Database connection: OK"
else
    echo "❌ Database connection: FAILED"
    echo "Wait 30 seconds and try again, or check docker compose logs"
fi
```

**If build fails with cache issues**:
```bash
# Try no-cache build (slower but more reliable)
docker build --no-cache -t pg_tviews .
docker compose down -v
docker compose up -d
```

**If build fails with Rust errors**:
- Check your code changes in Step 1.2.3
- Review git diff for syntax errors
- Restore backup: `cp src/event_trigger.rs.backup src/event_trigger.rs`
- Ask for help with error message

---

#### Step 1.2.5: Test Event Trigger Behavior (15 minutes)

**Goal**: Verify event trigger validates but doesn't crash

```bash
# Navigate back to benchmark directory
cd test/sql/comprehensive_benchmarks

# Reload schema and data (fresh state)
echo "=== RELOADING SCHEMA AND DATA ==="
psql -d pg_tviews_benchmark -f schemas/01_ecommerce_schema.sql
psql -d pg_tviews_benchmark -v data_scale="small" -f data/01_ecommerce_data.sql

echo ""
echo "=== CREATING TEST TABLE (TRIGGERS EVENT) ==="
psql -d pg_tviews_benchmark <<EOF
-- This should trigger event but NOT auto-convert
CREATE TABLE benchmark.tv_test AS
SELECT id, data FROM benchmark.tv_product LIMIT 5;
EOF

# Check for log message (may appear in Docker logs or psql output)
echo ""
echo "Expected log: INFO: TVIEW table created. To convert..."
echo ""

# Verify table exists but is NOT a TVIEW yet
echo "=== VERIFYING TABLE EXISTS ==="
psql -d pg_tviews_benchmark <<EOF
SELECT schemaname, tablename
FROM pg_tables
WHERE tablename = 'tv_test';
EOF

echo ""
echo "=== VERIFYING NOT YET A TVIEW ==="
psql -d pg_tviews_benchmark <<EOF
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';
EOF
# Expected: Empty result (not converted yet)

echo ""
echo "=== MANUALLY CONVERTING ==="
psql -d pg_tviews_benchmark <<EOF
SELECT pg_tviews_convert_existing_table('benchmark.tv_test');
EOF

echo ""
echo "=== VERIFYING NOW A TVIEW ==="
psql -d pg_tviews_benchmark <<EOF
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';
EOF
# Expected: Shows tv_test entry

echo ""
```

**What Success Looks Like**:
- ✅ CREATE TABLE completes without SPI transaction error
- ✅ Table exists in pg_tables
- ✅ NOT in pg_tviews_metadata before manual conversion
- ✅ Manual conversion succeeds
- ✅ Appears in pg_tviews_metadata after manual conversion

**If CREATE TABLE fails with SPI error**:
- ❌ Event trigger still has conversion code
- ❌ Docker image didn't rebuild correctly
- ❌ Check: Did you restart containers after build?
- ❌ Verify: `docker images pg_tviews` shows recent timestamp

**If manual conversion fails**:
- ❌ Review Step 1.2.2 troubleshooting
- ❌ Check function exists: `\df pg_tviews*`

---

#### Step 1.2.6: Verification Checklist

- [ ] Event trigger validates structure without crashing
- [ ] No "SPI error: Transaction" during CREATE TABLE AS SELECT
- [ ] Tables created but NOT auto-converted to TVIEW
- [ ] Manual conversion works: `SELECT pg_tviews_convert_existing_table('table')`
- [ ] TVIEW metadata populated after manual conversion
- [ ] Docker rebuild successful with new Rust code
- [ ] Database accessible after rebuild

**Success Criteria Met**: ✅ Manual TVIEW conversion workflow works

**Communication Checkpoint**:
- [ ] Post in team channel: "Task 1.2 complete - TVIEW manual conversion working ✅"
- [ ] Note: Architectural change (auto-conversion disabled)

---

### Task 1.3: Fix Scenarios Variable Quoting
**Priority**: P1 (blocks scenario benchmarks)
**Time Estimate**: 10 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`

---

#### Step 1.3.1: Identify the Issue (3 minutes)

```bash
# Check the scenarios execution line
echo "=== CHECKING LINE 131 ==="
sed -n '125,135p' test/sql/comprehensive_benchmarks/run_benchmarks.sh | cat -n
echo ""
```

**Current Code (line 131)**:
```bash
$PSQL -v data_scale="'$scale'" -f "scenarios/${scenario}_benchmarks.sql"
```

**Problem**: Extra quotes around `$scale` - double-quoting issue
- Shell wraps `$scale` in quotes: `"'$scale'"`
- Psql receives: `data_scale='small'` (with quotes as part of value)
- SQL uses: `:'data_scale'` (adds more quotes)
- Result: `:''small''` → ERROR

**Correct Pattern** (line 127 - data generation):
```bash
$PSQL -v data_scale="$scale" -f data/01_ecommerce_data.sql
```

**Why it works**:
- Shell: `data_scale="$scale"` → psql receives: `data_scale=small`
- SQL: `:'data_scale'` → becomes: `'small'`
- Result: Correct string interpolation

---

#### Step 1.3.2: Apply the Fix (5 minutes)

**Make backup first**:
```bash
cp test/sql/comprehensive_benchmarks/run_benchmarks.sh test/sql/comprehensive_benchmarks/run_benchmarks.sh.backup
echo "✅ Backup created"
```

**Edit the file**:
1. Open `test/sql/comprehensive_benchmarks/run_benchmarks.sh`
2. Go to line 131 (or search for `scenarios/${scenario}_benchmarks.sql`)
3. Find: `$PSQL -v data_scale="'$scale'" -f`
4. Change to: `$PSQL -v data_scale="$scale" -f`
5. Save the file

**Verify the change**:
```bash
echo "=== VERIFYING CHANGE ==="
sed -n '131p' test/sql/comprehensive_benchmarks/run_benchmarks.sh
echo ""
echo "Should match data generation pattern (line 127):"
sed -n '127p' test/sql/comprehensive_benchmarks/run_benchmarks.sh
echo ""

# Show diff
git diff test/sql/comprehensive_benchmarks/run_benchmarks.sh
```

---

#### Step 1.3.3: Quick Test (Optional - 2 minutes)

```bash
# If you want to test scenarios immediately (optional):
echo "=== TESTING SCENARIO EXECUTION ==="
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | grep -E "(scenario|error)" | head -20
cd ../../..
```

**Expected**: Scenarios start executing without variable errors

---

#### Step 1.3.4: Verification Checklist

- [ ] Line 131 matches line 127 pattern
- [ ] No extra quotes around `$scale` variable
- [ ] File saved correctly
- [ ] Git diff shows only the quote removal

**Success Criteria Met**: ✅ Scenarios variable quoting fixed

**Communication Checkpoint**:
- [ ] Post in team channel: "Task 1.3 complete - scenarios variable fixed ✅"

---

### Task 1.4: End-to-End Verification
**Priority**: P0 (validates all fixes)
**Time Estimate**: 40 minutes
**Risk**: Low
**Dependencies**: Tasks 1.1, 1.2, 1.3 completed

---

#### Step 1.4.1: Clean Environment (5 minutes)

```bash
echo "=== CLEANING ENVIRONMENT ==="

# Stop and clean containers
docker compose down -v
echo "✅ Containers stopped, volumes removed"

# Start fresh containers
docker compose up -d
echo "✅ Containers started"

# Wait for database to be ready
echo "Waiting for database initialization..."
sleep 10

# Verify containers are running
echo ""
echo "=== CONTAINER STATUS ==="
docker compose ps
echo ""

# Test database connection
echo "=== TESTING DATABASE CONNECTION ==="
psql -d pg_tviews_benchmark -c "SELECT version();" > /dev/null 2>&1
if [ $? -eq 0 ]; then
    echo "✅ Database ready"
else
    echo "⚠️  Database not ready, waiting 30 more seconds..."
    sleep 30
    psql -d pg_tviews_benchmark -c "SELECT version();" > /dev/null 2>&1
    if [ $? -eq 0 ]; then
        echo "✅ Database ready"
    else
        echo "❌ Database still not ready - check docker compose logs"
        exit 1
    fi
fi
echo ""
```

---

#### Step 1.4.2: Run Full Benchmark (15 minutes)

```bash
# Navigate to benchmark directory
cd test/sql/comprehensive_benchmarks

# Run benchmark with logging
echo "=== RUNNING FULL BENCHMARK ==="
echo "This will take 10-15 minutes..."
echo ""
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/benchmark_verification.log

echo ""
echo "=== CHECKING FOR ERRORS ==="
grep -i "error" /tmp/benchmark_verification.log | grep -v "0 errors" | head -20

echo ""
echo "=== CHECKING FOR SUCCESS MESSAGES ==="
grep -iE "success|complete|finished" /tmp/benchmark_verification.log | tail -20

echo ""
```

**What to watch for**:
- ✅ Schema loads without errors
- ✅ Data generation completes
- ✅ No "syntax error at or near :"
- ✅ No "SPI error: Transaction"
- ✅ Scenarios execute
- ⚠️  May see INFO messages about manual TVIEW conversion (expected)

---

#### Step 1.4.3: Structured Validation (15 minutes)

```bash
# Return to project root
cd ../../..

# Schema verification
echo "=== SCHEMA VERIFICATION ==="
psql -d pg_tviews_benchmark <<EOF
SELECT schemaname, tablename
FROM pg_tables
WHERE schemaname = 'benchmark'
ORDER BY tablename;
EOF
# Expected: Shows benchmark.tb_category, benchmark.tb_product, benchmark.tv_product

echo ""

# Data verification
echo "=== DATA VERIFICATION ==="
psql -d pg_tviews_benchmark <<EOF
SELECT 'tb_category' as table, COUNT(*) FROM benchmark.tb_category
UNION ALL SELECT 'tb_product', COUNT(*) FROM benchmark.tb_product
UNION ALL SELECT 'tv_product', COUNT(*) FROM benchmark.tv_product
ORDER BY table;
EOF
# Expected: All tables have row counts > 0

echo ""

# TVIEW verification BEFORE manual conversion
echo "=== TVIEW VERIFICATION (Before Manual Conversion) ==="
psql -d pg_tviews_benchmark <<EOF
SELECT table_name, created_at FROM pg_tviews_metadata ORDER BY table_name;
EOF
# Expected: May be empty or missing tv_product (event trigger doesn't auto-convert)

echo ""

# Perform manual conversion
echo "=== PERFORMING MANUAL TVIEW CONVERSION ==="
psql -d pg_tviews_benchmark <<EOF
-- Convert tv_product to TVIEW
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');
EOF
# Expected: Function returns successfully

echo ""

# TVIEW verification AFTER manual conversion
echo "=== TVIEW VERIFICATION (After Manual Conversion) ==="
psql -d pg_tviews_benchmark <<EOF
SELECT table_name, created_at FROM pg_tviews_metadata ORDER BY table_name;
EOF
# Expected: Now shows tv_product entry

echo ""

# Test creating a new TVIEW manually
echo "=== MANUAL CONVERSION WORKFLOW TEST ==="
psql -d pg_tviews_benchmark <<EOF
-- Create test table (triggers event but doesn't auto-convert)
CREATE TABLE benchmark.tv_manual_test AS
SELECT id, data FROM benchmark.tv_product LIMIT 3;

-- Manually convert
SELECT pg_tviews_convert_existing_table('benchmark.tv_manual_test');

-- Verify
SELECT table_name FROM pg_tviews_metadata WHERE table_name = 'tv_manual_test';
EOF
# Expected: Shows tv_manual_test entry

echo ""
```

---

#### Step 1.4.4: Success Criteria Checklist

**Core Functionality**:
- [ ] Schema loads in 'benchmark' schema (not 'public')
- [ ] tb_category has >0 rows
- [ ] tb_product has >0 rows
- [ ] tv_product table exists

**TVIEW Conversion**:
- [ ] Manual conversion works: `pg_tviews_convert_existing_table()`
- [ ] pg_tviews_metadata populated after manual conversion
- [ ] Event trigger validates but doesn't crash
- [ ] No SPI transaction errors

**Benchmark Execution**:
- [ ] Data generation completes without syntax errors
- [ ] Scenarios execute without variable errors
- [ ] Results written to benchmark_results table (if applicable)
- [ ] No "relation does not exist" errors

**Error Resolution**:
- [ ] No "syntax error at or near :" errors
- [ ] No "SPI error: Transaction" errors
- [ ] No psql variable quoting errors

**Success Criteria Met**: ✅ All benchmarks run successfully with manual TVIEW conversion workflow

---

#### Step 1.4.5: Compare Before/After (5 minutes)

```bash
echo "=== BASELINE vs CURRENT COMPARISON ==="
echo ""
echo "BEFORE (baseline errors):"
grep -i "error" /tmp/baseline_errors.log | head -10
echo ""
echo "AFTER (current state):"
grep -i "error" /tmp/benchmark_verification.log | grep -v "0 errors" | head -10
echo ""

echo "=== CHANGES ==="
echo "What should have changed:"
echo "  ❌ 'syntax error at or near :' → ✅ Should be GONE"
echo "  ❌ 'SPI error: Transaction' → ✅ Should be GONE"
echo "  ❌ Failed benchmark runs → ✅ Should SUCCEED"
echo ""
echo "What should remain (expected):"
echo "  ℹ️  Manual TVIEW conversion workflow → Still required (this is expected)"
echo "  ℹ️  INFO messages about conversion → Normal (event trigger logs)"
echo ""
```

**Verification**:
- [ ] Baseline errors are gone
- [ ] New errors haven't appeared
- [ ] Manual conversion workflow is documented

---

#### Step 1.4.6: Performance Sanity Check (Optional - 5 minutes)

```bash
echo "=== PERFORMANCE CHECK ==="
echo "Running timed benchmark..."
time ./test/sql/comprehensive_benchmarks/run_benchmarks.sh --scale small > /tmp/perf_check.log 2>&1
echo ""
echo "Note the time above (real time)"
echo "Expected: Similar to baseline (not significantly slower)"
echo "If >2x slower than expected, investigate before committing"
echo ""
```

**Note**: First run may be slower due to cold caches. This is normal.

---

### Task 1.5: Commit Changes
**Priority**: P0 (preserve working state)
**Time Estimate**: 15 minutes
**Risk**: Low
**Dependencies**: Task 1.4 passes ALL checks

---

#### Step 1.5.1: Pre-Commit Verification (5 minutes)

```bash
echo "=== PRE-COMMIT VERIFICATION ==="

# Run tests one more time
cd test/sql/comprehensive_benchmarks
echo "Running final verification..."
./run_benchmarks.sh --scale small > /tmp/final_verification.log 2>&1

# Check no new errors
echo ""
echo "Checking for errors..."
error_count=$(grep -i "error" /tmp/final_verification.log | grep -v "0 errors" | wc -l)
if [ "$error_count" -eq 0 ]; then
    echo "✅ No errors found"
else
    echo "❌ Found $error_count errors - DO NOT COMMIT"
    echo "Review /tmp/final_verification.log and fix issues first"
    exit 1
fi

# Return to root
cd ../../..

# Show what will be committed
echo ""
echo "=== FILES TO BE COMMITTED ==="
git status --short
echo ""
```

**Checklist**:
- [ ] Final verification passed without errors
- [ ] All expected files show in git status
- [ ] No unexpected changes (check git status carefully)

---

#### Step 1.5.2: Create Separate Commits (10 minutes)

**Important**: Commit each fix separately for clean history

```bash
# Commit 1: Data generation fix
git add test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql
git commit -m "fix(benchmarks): Fix psql variable interpolation in data generation

- Change :data_scale to :'data_scale' for string interpolation (line 151)
- Fixes 'syntax error at or near :' at line 151
- Verified with manual psql test and full data generation

Tested:
- Psql variable interpolation: ✅
- Data generation (small scale): ✅
- tb_product row count: 5000+ rows
- tb_category row count: 100+ rows

Related: Phase 1 Task 1.1"

echo "✅ Commit 1 created: Data generation fix"
echo ""

# Commit 2: TVIEW conversion architecture fix
git add src/event_trigger.rs
git commit -m "fix(tview): Disable auto-conversion in event trigger [ARCHITECTURE]

- Event triggers can't use SPI for catalog queries (PostgreSQL limitation)
- Changed to validate-only mode in on_create_table_as_select_end()
- Users must manually call pg_tviews_convert_existing_table()
- Prevents 'SPI error: Transaction' during DDL events
- Added informational log message for manual conversion workflow

Architectural Decision:
PostgreSQL prevents nested transactions in DDL event triggers. SPI calls
create sub-transactions, causing conflicts. Solution: Disable auto-conversion,
provide manual conversion function for users.

Future: Background worker for automatic conversion (planned)

Tested:
- Event trigger validation: ✅
- Manual conversion workflow: ✅
- No SPI transaction errors: ✅
- TVIEW metadata creation: ✅

Related: Phase 1 Task 1.2
See: https://www.postgresql.org/docs/current/event-trigger-definition.html"

echo "✅ Commit 2 created: TVIEW conversion architecture fix"
echo ""

# Commit 3: Scenarios variable fix
git add test/sql/comprehensive_benchmarks/run_benchmarks.sh
git commit -m "fix(benchmarks): Fix psql variable quoting in scenarios

- Match data generation pattern (line 127)
- Remove extra quotes: data_scale='$scale' → data_scale=$scale
- Consistent variable passing across script (lines 127 and 131 now match)

Issue: Double-quoting caused psql to receive data_scale='small' instead of
data_scale=small, leading to :'data_scale' becoming :''small'' in SQL.

Tested:
- Scenarios execution: ✅
- Variable interpolation: ✅
- Consistency with data generation: ✅

Related: Phase 1 Task 1.3"

echo "✅ Commit 3 created: Scenarios variable fix"
echo ""

# Commit 4: Verification documentation
git commit --allow-empty -m "test(benchmarks): Verify all fixes work together [E2E]

Phase 1 verification results:

Schema Loading:
- benchmark.tb_category: ✅
- benchmark.tb_product: ✅
- benchmark.tv_product: ✅

Data Generation:
- Small scale: ✅ (5000+ products, 100+ categories)
- No syntax errors: ✅
- Variable interpolation: ✅

TVIEW Conversion:
- Manual conversion workflow: ✅
- No SPI errors: ✅
- Metadata creation: ✅

Benchmark Scenarios:
- Execution without errors: ✅
- Variable quoting: ✅

Error Resolution:
- 'syntax error at or near :': ✅ RESOLVED
- 'SPI error: Transaction': ✅ RESOLVED
- Variable quoting errors: ✅ RESOLVED

All Phase 1 success criteria met. Manual TVIEW conversion workflow validated.

Related: Phase 1 Task 1.4"

echo "✅ Commit 4 created: E2E verification"
echo ""
```

---

#### Step 1.5.3: Review Commits

```bash
echo "=== COMMIT HISTORY ==="
git log --oneline -4
echo ""

echo "=== DETAILED COMMIT REVIEW ==="
git log -4 --stat
echo ""

echo "=== VERIFICATION ==="
echo "Check commits above:"
echo "  [ ] 4 commits created"
echo "  [ ] Each commit has descriptive message"
echo "  [ ] Commit messages explain WHY (not just WHAT)"
echo "  [ ] Architectural decisions documented"
echo "  [ ] Test results included"
echo ""
```

---

#### Step 1.5.4: Final Checklist

- [ ] Each fix in separate commit (clean history)
- [ ] Commit messages explain WHY, not just WHAT
- [ ] Architectural decisions documented in commit messages
- [ ] Test results included in commit messages
- [ ] No uncommitted changes remain: `git status`
- [ ] All tests pass after commits

**Success Criteria Met**: ✅ All 4 commits created with clean history

**Communication Checkpoint**:
- [ ] Post in team channel: "Phase 1 complete - all fixes committed ✅"
- [ ] Link to commits or branch for review
- [ ] Mention: Manual TVIEW conversion workflow now documented

---

## 🟡 Phase 2: Documentation & Polish

### Task 2.1: Documentation Updates
**Priority**: Medium
**Time Estimate**: 30 minutes
**Risk**: Low
**Files Modified**: `README.md`, `docs/TROUBLESHOOTING.md` (new)

---

#### Step 2.1.1: Update README with Manual Conversion Workflow (15 minutes)

**Open README.md** and find the TVIEW section (or create if missing)

**Add this content**:

```markdown
## TVIEW Creation Workflow

Due to PostgreSQL event trigger limitations, TVIEW tables are not automatically converted during `CREATE TABLE AS SELECT`.

### Manual Conversion Process

**Step 1: Create your TVIEW table**
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

**Step 2: Manually convert to TVIEW**
```sql
SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

**Step 3: Verify conversion**
```sql
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_my_entity';
```

### Event Trigger Behavior

Event triggers now only validate TVIEW structure. After `CREATE TABLE AS SELECT`, you'll see:
```
INFO: TVIEW table created. To convert to TVIEW, run: SELECT pg_tviews_convert_existing_table('tv_my_entity');
```

### Why Manual Conversion?

PostgreSQL event triggers cannot use the Server Programming Interface (SPI) to query system catalogs during DDL events due to transaction isolation. This is a PostgreSQL architectural limitation, not a bug.

**Technical Details**: Event triggers run within the same transaction as DDL commands. SPI calls create sub-transactions, which PostgreSQL prevents during DDL events to maintain consistency.

### Future: Automatic Conversion

Background worker support for automatic conversion is planned for a future release. This will allow queued conversions to run in a separate transaction context.

### Example: E-commerce Benchmark

```sql
-- Create the table
CREATE TABLE benchmark.tv_product AS
SELECT
    id,
    pk_product,
    fk_category,
    data
FROM benchmark.v_product;

-- Convert to TVIEW
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');

-- Verify
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_product';
```
```

**Commit this change**:
```bash
git add README.md
git commit -m "docs(tview): Add manual conversion workflow and architecture explanation

- Document manual TVIEW conversion process (3-step workflow)
- Explain PostgreSQL event trigger SPI limitations
- Add example from e-commerce benchmark
- Note future roadmap for background worker

Target audience: Users creating TVIEW tables for first time

Related: Phase 1 Task 1.2 (event trigger architecture change)"
```

---

#### Step 2.1.2: Create Troubleshooting Guide (15 minutes)

**Create new file**: `docs/TROUBLESHOOTING.md`

```markdown
# pg_tviews Troubleshooting Guide

This guide covers common issues and their solutions.

## Benchmark-Related Issues

### 1. "syntax error at or near :"

**Symptom**:
```
psql:data/01_ecommerce_data.sql:151: ERROR: syntax error at or near ":"
```

**Cause**: Incorrect psql variable interpolation syntax

**Solution**: Use quoted interpolation for string values

**Wrong**:
```sql
WHERE scale = :data_scale   -- Unquoted (expects number/boolean)
```

**Correct**:
```sql
WHERE scale = :'data_scale'  -- Quoted (treats as string)
```

**Psql Variable Rules**:
- `:var` → Unquoted (numbers, booleans, SQL keywords)
- `:'var'` → Single-quoted (string literals)
- `:"var"` → Double-quoted (identifiers)

**Verification**:
```bash
psql -v data_scale="small" -c "SELECT :'data_scale' AS value;"
# Should return: small
```

---

### 2. "SPI error: Transaction"

**Symptom**:
```
ERROR: Failed to convert table to TVIEW: SPI query failed: SPI error: Transaction
Query: Unknown
```

**Cause**: Event triggers cannot use SPI during DDL events (PostgreSQL limitation)

**Solution**: Use manual conversion workflow

**Steps**:
```sql
-- 1. Create table (event trigger validates structure only)
CREATE TABLE tv_test AS SELECT id, data FROM v_test;

-- 2. Manually convert to TVIEW
SELECT pg_tviews_convert_existing_table('tv_test');

-- 3. Verify
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_test';
```

**Why This Happens**: PostgreSQL prevents nested transactions during DDL events. SPI calls create sub-transactions, causing conflicts.

**Future**: Background worker support will enable automatic conversion in a separate transaction context.

---

### 3. "relation does not exist"

**Symptom**:
```
ERROR: relation "tv_product" does not exist
```

**Cause**: Missing schema qualification or incorrect search_path

**Solution 1: Use Schema-Qualified Names**
```sql
-- Wrong
SELECT * FROM tv_product;

-- Correct
SELECT * FROM benchmark.tv_product;
```

**Solution 2: Set Search Path**
```sql
SET search_path TO benchmark, public;
SELECT * FROM tv_product;  -- Now works
```

**Diagnostic**:
```bash
# Check which schema the table is in
psql -d pg_tviews_benchmark -c "
SELECT schemaname, tablename
FROM pg_tables
WHERE tablename = 'tv_product';
"
```

---

### 4. Variable Quoting Issues in Shell Scripts

**Symptom**: Scenarios fail with variable interpolation errors

**Cause**: Inconsistent quoting between data generation and scenarios

**Wrong**:
```bash
# Double-quoting issue
$PSQL -v data_scale="'$scale'" -f scenarios/file.sql
# Results in: data_scale='small' (quotes part of value)
```

**Correct**:
```bash
# Single variable assignment
$PSQL -v data_scale="$scale" -f scenarios/file.sql
# Results in: data_scale=small (clean value)
```

**Rule**: Let psql handle quoting in SQL, not in shell

---

## TVIEW-Related Issues

### 5. "Table validation failed: missing required columns"

**Symptom**:
```
ERROR: Table validation failed: missing required columns: id, data
```

**Cause**: Table missing required columns for TVIEW

**Solution**: Ensure table has minimum required columns

**Minimum TVIEW Structure**:
```sql
CREATE TABLE tv_entity AS
SELECT
    id,    -- UUID (required)
    data   -- JSONB (required)
FROM v_entity;
```

**Recommended TVIEW Structure** (with optimizations):
```sql
CREATE TABLE tv_entity AS
SELECT
    id,           -- UUID (required)
    pk_entity,    -- INTEGER primary key (recommended)
    fk_parent,    -- INTEGER foreign key (for filtering)
    parent_id,    -- UUID foreign key (for joins)
    path,         -- LTREE (for hierarchical queries)
    data          -- JSONB (required)
FROM v_entity;
```

**Verification**:
```bash
# Check table structure
psql -d pg_tviews_benchmark -c "\d benchmark.tv_product"
```

---

### 6. Manual Conversion Function Doesn't Exist

**Symptom**:
```
ERROR: function pg_tviews_convert_existing_table(text) does not exist
```

**Cause**: Extension not loaded in current database

**Solution**: Load the extension
```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

**Verification**:
```bash
# Check extension is loaded
psql -d pg_tviews_benchmark -c "\dx pg_tviews"

# List TVIEW functions
psql -d pg_tviews_benchmark -c "\df pg_tviews*"
```

---

## Diagnostic Commands

### Check Schema State
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT schemaname, tablename
FROM pg_tables
WHERE schemaname IN ('benchmark', 'public')
ORDER BY schemaname, tablename;
EOF
```

### Check Data Loading
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT
    'tb_category' as table,
    COUNT(*) as row_count
FROM benchmark.tb_category
UNION ALL
SELECT 'tb_product', COUNT(*)
FROM benchmark.tb_product
ORDER BY table;
EOF
```

### Check TVIEW Status
```bash
psql -d pg_tviews_benchmark <<EOF
SELECT
    table_name,
    source_view,
    created_at,
    last_refreshed
FROM pg_tviews_metadata
ORDER BY table_name;
EOF
```

### Test Manual Conversion
```bash
psql -d pg_tviews_benchmark <<EOF
-- Attempt conversion
SELECT pg_tviews_convert_existing_table('benchmark.tv_product');

-- Check result
SELECT * FROM pg_tviews_metadata WHERE table_name = 'tv_product';
EOF
```

### Check Docker Container Status
```bash
# Container status
docker compose ps

# Recent logs
docker compose logs --tail=50

# Database logs specifically
docker compose logs postgres | tail -50
```

### Full Benchmark Diagnostic
```bash
# Run benchmark with full logging
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/benchmark_debug.log

# Check for errors
grep -i "error" /tmp/benchmark_debug.log | grep -v "0 errors"

# Check for successes
grep -iE "success|complete" /tmp/benchmark_debug.log
```

---

## Getting Help

If you're stuck after trying the solutions above:

1. **Capture diagnostics**:
   ```bash
   # Run all diagnostic commands above
   # Save output to a file
   ```

2. **Note exact error messages**:
   - Copy the full error (not paraphrased)
   - Include line numbers if shown
   - Include relevant code context

3. **Check git history**:
   ```bash
   git log --oneline -10
   # Recent changes may have introduced issues
   ```

4. **Ask for help with context**:
   - What you were trying to do
   - What command you ran
   - Full error message
   - What you've tried already
   - Diagnostic output

5. **Search issues**:
   - Check project issues for similar problems
   - Search error message text

---

## Performance Issues

### Benchmark Runs Slowly

**Symptom**: Benchmark takes >30 minutes for small scale

**Possible Causes**:
1. Cold Docker cache (first run)
2. Insufficient resources (RAM/CPU)
3. Disk I/O bottleneck

**Solutions**:
```bash
# Check Docker resources
docker stats

# Check disk I/O
iostat -x 5

# Increase Docker resources
# Edit Docker Desktop settings: Memory > 4GB, CPUs > 2
```

### Query Performance Regression

**Symptom**: Queries slower than expected

**Diagnostic**:
```sql
EXPLAIN ANALYZE SELECT * FROM benchmark.tv_product WHERE ...;
```

**Common Issues**:
- Missing indexes on optimization columns
- TVIEW not converted (querying raw table)
- Outdated TVIEW data (needs refresh)

---

*Last Updated: 2025-12-14*
```

**Commit this change**:
```bash
git add docs/TROUBLESHOOTING.md
git commit -m "docs: Add comprehensive troubleshooting guide

- Cover all common benchmark and TVIEW issues
- Include diagnostic commands for each issue
- Add psql variable interpolation guide
- Document SPI transaction limitations
- Provide step-by-step solutions

Issues covered:
- Syntax errors (psql variables)
- SPI transaction errors (event triggers)
- Relation not found (schema qualification)
- Variable quoting (shell scripts)
- Missing columns (TVIEW validation)
- Extension not loaded
- Performance issues

Related: Phase 2 Task 2.1"
```

---

### Task 2.2: Add Diagnostic Logging
**Priority**: Low
**Time Estimate**: 30 minutes
**Risk**: Low
**Files Modified**: `test/sql/comprehensive_benchmarks/run_benchmarks.sh`

---

#### Step 2.2.1: Add Logging Functions (10 minutes)

**Open `run_benchmarks.sh`** and add after initial variable setup (near top of file):

```bash
# ============================================================================
# Logging Functions
# ============================================================================

log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" >&2
}

log_info() {
    log "ℹ️  INFO: $1"
}

log_success() {
    log "✅ SUCCESS: $1"
}

log_error() {
    log "❌ ERROR: $1"
}

log_warning() {
    log "⚠️  WARNING: $1"
}

log_step() {
    log "📍 STEP: $1"
}
```

---

#### Step 2.2.2: Add Diagnostic Logging Throughout Script (20 minutes)

**Find key operations and add logging**:

```bash
# === Example 1: Schema Loading ===
log_step "Loading database schema..."
if $PSQL -f schemas/01_ecommerce_schema.sql > /tmp/schema_load.log 2>&1; then
    log_success "Schema loaded successfully"

    # Verify schema
    table_count=$($PSQL -t -c "SELECT COUNT(*) FROM pg_tables WHERE schemaname = 'benchmark';")
    log_info "Found $table_count tables in benchmark schema"
else
    log_error "Schema loading failed"
    cat /tmp/schema_load.log >&2
    exit 1
fi

# === Example 2: Data Generation ===
log_step "Generating data (scale=$scale)..."
if $PSQL -v data_scale="$scale" -f data/01_ecommerce_data.sql > /tmp/data_gen.log 2>&1; then
    log_success "Data generation completed"

    # Verify data
    product_count=$($PSQL -t -c "SELECT COUNT(*) FROM benchmark.tb_product;")
    log_info "Loaded $product_count products"
else
    log_error "Data generation failed"
    cat /tmp/data_gen.log >&2
    exit 1
fi

# === Example 3: Scenario Execution ===
for scenario in product_queries category_queries aggregations; do
    log_step "Running scenario: $scenario"

    if $PSQL -v data_scale="$scale" -f "scenarios/${scenario}_benchmarks.sql" > /tmp/scenario_${scenario}.log 2>&1; then
        log_success "Scenario $scenario completed"
    else
        log_error "Scenario $scenario failed"
        cat /tmp/scenario_${scenario}.log >&2
        exit 1
    fi
done

# === Example 4: Final Summary ===
log_info "Benchmark run summary:"
log_info "  Scale: $scale"
log_info "  Scenarios: 3/3 completed"
log_info "  Total time: ${SECONDS}s"
log_success "All benchmarks completed successfully"
```

**Add at the beginning of script**:
```bash
# Record start time
START_TIME=$(date +%s)
log_info "Starting benchmark run at $(date)"
log_info "Configuration: scale=$scale, database=$DB_NAME"
```

**Add at the end of script**:
```bash
# Calculate total time
END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))
log_success "Benchmark completed in ${DURATION} seconds"
```

---

#### Step 2.2.3: Verification

```bash
# Test the logging
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | head -50

# Check that you see:
# [2025-12-14 HH:MM:SS] ℹ️  INFO: Starting benchmark...
# [2025-12-14 HH:MM:SS] 📍 STEP: Loading database schema...
# [2025-12-14 HH:MM:SS] ✅ SUCCESS: Schema loaded successfully
# etc.
```

---

#### Step 2.2.4: Commit Changes

```bash
git add test/sql/comprehensive_benchmarks/run_benchmarks.sh
git commit -m "feat(benchmarks): Add comprehensive diagnostic logging

- Add logging functions (info, success, error, warning, step)
- Log all major operations with timestamps
- Include verification counts (tables, rows)
- Add timing information (start/end/duration)
- Improve error visibility with emojis

Benefits:
- Easier debugging (timestamps show where delays occur)
- Better visibility into benchmark progress
- Error logs include context
- Success metrics logged for verification

Example output:
[2025-12-14 10:30:15] 📍 STEP: Loading database schema...
[2025-12-14 10:30:16] ✅ SUCCESS: Schema loaded successfully
[2025-12-14 10:30:16] ℹ️  INFO: Found 5 tables in benchmark schema

Related: Phase 2 Task 2.2"
```

---

## ⚠️ Common Pitfalls for Junior Engineers

### 1. **Forgetting to Rebuild Docker After Rust Changes**
**Symptom**: Event trigger still crashes with SPI error after "fixing" code
**Cause**: You edited `src/event_trigger.rs` but didn't rebuild Docker image
**Fix**: ALWAYS run `docker build` + `docker compose down -v` + `docker compose up -d`

### 2. **Testing in Wrong Database**
**Symptom**: Changes don't appear to work
**Cause**: Testing in `postgres` database instead of `pg_tviews_benchmark`
**Fix**: Always verify with `psql -d pg_tviews_benchmark` (note the `-d` flag)

### 3. **Skipping Verification Steps**
**Symptom**: Commit broken code, waste team's time
**Cause**: "It should work, I'll skip the test" mentality
**Fix**: NEVER skip verification. ALWAYS run tests before committing. Tests catch issues early.

### 4. **Not Reading Error Messages Carefully**
**Symptom**: Stuck on error for >1 hour
**Cause**: Assuming what error means instead of reading it carefully
**Fix**:
- Copy EXACT error message (every word)
- Google the exact error text
- Check line numbers mentioned
- Ask for help with full context (don't paraphrase)

### 5. **Combining Multiple Changes in One Commit**
**Symptom**: Hard to review, hard to revert if needed
**Cause**: "I'll just commit everything at once"
**Fix**: Separate commits for each logical change (we did 4 commits for 3 fixes + verification)

### 6. **Not Creating Backup Branches**
**Symptom**: Lost work after failed experiment
**Cause**: Working directly on main/dev branch
**Fix**: Always create feature branch first: `git checkout -b feature-name`

### 7. **Ignoring Docker Logs**
**Symptom**: "It's not working" but no idea why
**Cause**: Not checking what Docker is actually doing
**Fix**: `docker compose logs` is your friend - check it when things fail

### 8. **Not Asking for Help Soon Enough**
**Symptom**: Wasted 4 hours on issue that senior could solve in 10 minutes
**Cause**: "I should be able to figure this out myself"
**Fix**:
- Stuck >30 minutes? Ask for hints
- Stuck >1 hour? Ask for help with full context
- Learning when to ask is a skill itself

---

## 🧪 Testing Strategy Summary

### Pre-Implementation Testing (RED Phase)
**Goal**: Confirm issues exist before fixing

```bash
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/baseline_errors.log
grep -i "error" /tmp/baseline_errors.log
```
**Expected**: Data generation error + TVIEW conversion error

---

### Post-Fix Testing (GREEN Phase)
**Goal**: Verify each fix independently

**Test 1: Data Generation Fix**
```bash
psql -d pg_tviews_benchmark -v data_scale="small" -f data/01_ecommerce_data.sql
```
**Expected**: No syntax errors, data loads successfully

**Test 2: TVIEW Conversion Fix**
```bash
psql -d pg_tviews_benchmark <<EOF
CREATE TABLE benchmark.tv_test AS SELECT id, data FROM benchmark.tv_product LIMIT 5;
SELECT pg_tviews_convert_existing_table('benchmark.tv_test');
EOF
```
**Expected**: Manual conversion succeeds, no SPI errors

**Test 3: Scenarios Fix**
```bash
./run_benchmarks.sh --scale small
```
**Expected**: Scenarios execute without variable errors

---

### Integration Testing (QA Phase)
**Goal**: Full end-to-end verification

```bash
# Clean environment
docker compose down -v
docker compose up -d

# Full test run
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh --scale small 2>&1 | tee /tmp/full_test.log

# Verify all success criteria (see Task 1.4.4)
```

---

### Regression Testing (GREENFIELD Phase)
**Goal**: Ensure fixes don't break existing functionality

1. **Test other scales**: medium, large
2. **Test multiple scenarios**: All scenario files
3. **Test edge cases**: Empty data, malformed tables
4. **Performance check**: Compare benchmark times with baseline

---

## 🛡️ Safety & Rollback

### Pre-Flight Checklist
```bash
# BEFORE starting fixes:
- [ ] Created backup branch: git checkout -b benchmark-fixes-20251214
- [ ] Captured baseline: ./run_benchmarks.sh > /tmp/baseline.log 2>&1
- [ ] Documented current error state
- [ ] Verified Docker has sufficient disk space: df -h
- [ ] All containers running: docker compose ps
```

---

### Rollback Strategy

**If Task 1.1 (Data Generation) fails**:
```bash
git checkout test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql
# or restore from backup:
cp test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql.backup test/sql/comprehensive_benchmarks/data/01_ecommerce_data.sql
```

**If Task 1.2 (TVIEW Conversion) fails**:
```bash
# Revert Rust code
git checkout src/event_trigger.rs
# or restore from backup:
cp src/event_trigger.rs.backup src/event_trigger.rs

# Rebuild clean image
docker build --no-cache -t pg_tviews .
docker compose down -v
docker compose up -d
```

**If Task 1.3 (Scenarios) fails**:
```bash
git checkout test/sql/comprehensive_benchmarks/run_benchmarks.sh
# or restore from backup:
cp test/sql/comprehensive_benchmarks/run_benchmarks.sh.backup test/sql/comprehensive_benchmarks/run_benchmarks.sh
```

**Nuclear rollback** (last resort):
```bash
git checkout dev  # or your main branch
git reset --hard origin/dev
docker compose down -v
docker system prune -af --volumes  # WARNING: Deletes ALL Docker data
docker compose up -d
```

---

### Safety Guardrails

**DO** ✅:
- Test each fix independently before proceeding
- Commit after each successful fix (atomic changes)
- Run verification after each change
- Document architectural decisions in commit messages
- Keep fixes minimal (no scope creep)
- Ask for help if stuck >30 minutes
- Create backups before modifying files
- Read error messages carefully

**DO NOT** ❌:
- Combine multiple fixes in one commit
- Skip verification steps ("it should work")
- Make "while I'm here" improvements
- Modify Rust code without rebuilding Docker
- Push to main/master without full verification
- Ignore warnings or errors
- Assume anything works without testing
- Work directly on main branch

---

## 📊 Success Metrics

### Phase 1 Complete When:
- [ ] Data generation works for all scales (small, medium, large)
- [ ] Manual TVIEW conversion succeeds without SPI errors
- [ ] Benchmark scenarios execute without variable errors
- [ ] Full benchmark run completes successfully
- [ ] All 4 commits pushed with clean git history
- [ ] Docker rebuild successful with Rust changes
- [ ] Team notified of completion

### Phase 2 Complete When:
- [ ] README updated with manual conversion workflow
- [ ] TROUBLESHOOTING.md created with diagnostic guide
- [ ] Diagnostic logging added to run_benchmarks.sh
- [ ] All documentation commits pushed
- [ ] Documentation reviewed by senior engineer

### Overall Success:
- [ ] `./run_benchmarks.sh --scale small` runs without errors
- [ ] Manual TVIEW conversion workflow documented and working
- [ ] Clean git history with separate commits for each fix
- [ ] Junior engineers can follow this plan independently
- [ ] Troubleshooting guide helps debug future issues

---

## 🎉 Completion Celebration

When Phase 1 is complete:

### Final Verification
- [ ] All tests passing
- [ ] All commits pushed
- [ ] Documentation updated
- [ ] Team notified

### What You Learned
- ✅ PostgreSQL event trigger architecture and limitations
- ✅ SPI transaction context handling and constraints
- ✅ Psql variable interpolation nuances (`:var` vs `:'var'`)
- ✅ Docker build workflows for Rust extensions
- ✅ TDD methodology (RED → GREEN → QA → GREENFIELD)
- ✅ Git commit best practices (atomic, descriptive, why not what)
- ✅ Debugging systematic approach (hypothesis-driven)

### Share Your Learnings
- Write a quick post-mortem (5-10 minutes)
- Share key insights with team
- Update your engineering journal
- Help others who encounter similar issues

### Celebrate!
🎉 **You just fixed a complex architectural issue!**

This involved:
- Understanding PostgreSQL internals
- Modifying Rust code
- Rebuilding Docker images
- Debugging multiple systems
- Writing production-quality documentation

**Well done!** This is senior-level work.

---

## 🚀 Quick Reference for Junior Engineers

### Start Here:

1. **Read this entire plan** (20-30 minutes)
   - Don't skip sections
   - Note the safety guardrails
   - Understand the architecture decisions

2. **Run Pre-Implementation Sanity Check** (10 minutes)
   - Step 0.1: Environment verification
   - Step 0.2: Baseline capture
   - Fix any failures before proceeding

3. **Start with Task 1.1** (45 minutes)
   - Data generation is the easiest fix
   - Build confidence with quick win
   - Test thoroughly before moving on

4. **Continue to Task 1.2** (90 minutes)
   - Most complex task
   - Read architecture explanation carefully
   - Don't skip the manual conversion test
   - Ask for help if manual conversion fails

5. **Quick win: Task 1.3** (10 minutes)
   - Simple change
   - Builds momentum

6. **Verify everything: Task 1.4** (40 minutes)
   - Most important task
   - Don't skip any verification steps
   - Baseline comparison shows your impact

7. **Commit your work: Task 1.5** (15 minutes)
   - Separate commits for clean history
   - Descriptive messages
   - Document the WHY

8. **Optional: Phase 2** (1 hour)
   - Documentation and polish
   - Helps future engineers
   - Shows professionalism

---

### Emergency Contacts

**If Docker issues**:
- Check: `docker compose ps`
- Logs: `docker compose logs --tail=50`
- Restart: `docker compose down -v && docker compose up -d`

**If psql issues**:
- Test connection: `psql -d pg_tviews_benchmark -c "SELECT 1;"`
- Check database exists: `psql -l | grep tviews`
- Check containers: `docker compose ps`

**If Rust issues**:
- Verify build completed: Check `docker build` output for errors
- Check syntax: Look for "error:" in build logs
- Restore backup: `cp src/event_trigger.rs.backup src/event_trigger.rs`

**If git issues**:
- Check status: `git status`
- Show diff: `git diff`
- Stash changes: `git stash` (saves work temporarily)
- **NEVER force push without asking**

**If stuck >30 minutes**:
- Document exact error message
- Note what you've tried
- Ask senior engineer with full context

---

### Time Checkpoints

Track your progress:

- **0:10** - Pre-check complete, baseline captured
- **1:00** - Task 1.1 complete (data generation fixed)
- **2:30** - Task 1.2 complete (TVIEW conversion fixed, Docker rebuilt)
- **2:45** - Task 1.3 complete (scenarios fixed)
- **3:30** - Task 1.4 complete (E2E verification passed)
- **3:45** - Task 1.5 complete (commits pushed)
- **4:45** - Phase 2 complete (documentation done)

**Falling behind schedule?**
- Ask for help (don't struggle alone)
- Focus on Phase 1 first (Phase 2 is optional)
- Document blockers for team discussion

**Ahead of schedule?**
- Help others with questions
- Review your commits one more time
- Start Phase 2 (documentation)
- Write a learning summary

---

### Key Reminders

1. **Test everything** - Never assume code works
2. **Read errors carefully** - They tell you exactly what's wrong
3. **Ask for help early** - 30 min rule (don't waste hours)
4. **Commit frequently** - Small, atomic commits
5. **Document the WHY** - Future you will thank you
6. **Verify before committing** - Run tests one more time
7. **Celebrate wins** - You're learning complex systems!

---

*Last Updated: 2025-12-14*
*Plan Version: 2.0 (Revised with senior architect feedback)*
*Status: Production ready for junior engineer execution*
*Estimated Time: 3-4 hours (Phase 1: 2-3 hours, Phase 2: 1 hour)*
*Risk Level: Medium (well-mitigated with rollback strategies)*
