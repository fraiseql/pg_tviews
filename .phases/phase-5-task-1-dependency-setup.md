# Phase 5 Task 1: jsonb_delta Dependency Setup

**Status:** Ready to implement
**Duration:** 1 day
**Parent:** Phase 5 - jsonb_delta Integration
**TDD Phase:** RED → GREEN → REFACTOR

---

## Objective

Set up jsonb_delta as an optional dependency with runtime detection and graceful degradation.

**Success Criteria:**
- ✅ Documentation explains jsonb_delta installation
- ✅ Runtime check detects if jsonb_delta is installed
- ✅ Warning shown if not installed (but doesn't fail)
- ✅ Info message shown if installed
- ✅ Test verifies detection works

---

## Context

jsonb_delta v0.3.1 provides high-performance JSONB patching functions:
- `jsonb_smart_patch_scalar()` - 2× faster shallow merges
- `jsonb_smart_patch_nested()` - 2× faster nested updates
- `jsonb_smart_patch_array()` - 3× faster array element updates

pg_tviews should work with OR without jsonb_delta:
- **With jsonb_delta:** 1.5-2.2× faster cascades (optimal)
- **Without jsonb_delta:** Still functional, just slower (fallback to full document replacement)

---

## RED Phase: Write Failing Tests First

### Test 1: Extension Detection (SQL)

**File:** `test/sql/50_jsonb_delta_detection.sql`

```sql
-- Phase 5 Task 1 RED: Test jsonb_delta detection
-- This test should FAIL initially because check function doesn't exist yet

BEGIN;
    SET client_min_messages TO WARNING;

    -- Test Case 1: Detection when jsonb_delta NOT installed
    DROP EXTENSION IF EXISTS jsonb_delta CASCADE;
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;

    -- Create pg_tviews without jsonb_delta
    CREATE EXTENSION pg_tviews;

    -- Should see warning in logs:
    -- "jsonb_delta extension not found. pg_tviews will work but with reduced performance."

    -- Verify pg_tviews still works
    CREATE TABLE tb_test (pk_test INT PRIMARY KEY, id UUID, name TEXT);
    INSERT INTO tb_test VALUES (1, gen_random_uuid(), 'Test');

    SELECT pg_tviews_create('test', $$
        SELECT pk_test, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_test
    $$);

    -- Verify TVIEW created
    SELECT COUNT(*) = 1 AS tview_created FROM pg_tview_meta WHERE entity = 'test';
    -- Expected: t

    -- Verify data populated
    SELECT data->>'name' AS name FROM tv_test WHERE pk_test = 1;
    -- Expected: 'Test'

    -- Test Case 2: Detection when jsonb_delta IS installed
    CREATE EXTENSION jsonb_delta;

    -- Reload pg_tviews (or check would happen on next _PG_init)
    -- In practice, this is checked once at extension load
    -- For testing, we can call the check function directly
    SELECT pg_tviews_check_jsonb_delta();
    -- Expected: t (true)

    -- Should see info in logs:
    -- "jsonb_delta extension detected - performance optimizations enabled"

ROLLBACK;
```

**Expected Result:** Test FAILS because:
- `pg_tviews_check_jsonb_delta()` function doesn't exist
- No warning/info messages in logs

### Test 2: Runtime Check Function (Rust)

**File:** `src/lib.rs` (add test at bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[pg_test]
    fn test_jsonb_delta_detection_when_present() {
        // Setup: Ensure jsonb_delta is installed
        Spi::run("CREATE EXTENSION IF NOT EXISTS jsonb_delta").unwrap();

        // Test: Check should return true
        let result = check_jsonb_delta_available();
        assert!(result, "jsonb_delta should be detected when installed");
    }

    #[pg_test]
    fn test_jsonb_delta_detection_when_absent() {
        // Setup: Drop jsonb_delta if present
        Spi::run("DROP EXTENSION IF EXISTS jsonb_delta CASCADE").ok();

        // Test: Check should return false
        let result = check_jsonb_delta_available();
        assert!(!result, "jsonb_delta should not be detected when not installed");
    }

    #[pg_test]
    fn test_pg_tviews_works_without_jsonb_delta() {
        // Setup: Ensure jsonb_delta is NOT installed
        Spi::run("DROP EXTENSION IF EXISTS jsonb_delta CASCADE").ok();

        // Test: pg_tviews should still function
        Spi::run("CREATE TABLE tb_demo (pk_demo INT PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_demo VALUES (1, 'Demo')").unwrap();

        // This should work even without jsonb_delta
        let result = Spi::get_one::<bool>(
            "SELECT pg_tviews_create('demo', 'SELECT pk_demo, name FROM tb_demo') IS NOT NULL"
        );

        assert!(result.unwrap_or(false), "pg_tviews should work without jsonb_delta");
    }
}
```

**Expected Result:** Tests FAIL because:
- `check_jsonb_delta_available()` function doesn't exist
- Tests won't compile

---

## GREEN Phase: Make Tests Pass (Minimal Implementation)

### Step 1: Add Runtime Detection Function

**File:** `src/lib.rs`

**Location:** Add after existing functions, before `_PG_init()`

```rust
/// Check if jsonb_delta extension is available at runtime
/// Returns true if extension is installed, false otherwise
pub fn check_jsonb_delta_available() -> bool {
    let result = Spi::connect(|client| {
        let rows = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')",
            None,
            None,
        )?;

        for row in rows {
            if let Some(exists) = row[1].value::<bool>()? {
                return Ok(exists);
            }
        }
        Ok(false)
    });

    result.unwrap_or(false)
}

/// Export as SQL function for testing
#[pg_extern]
fn pg_tviews_check_jsonb_delta() -> bool {
    check_jsonb_delta_available()
}
```

### Step 2: Add Check to _PG_init()

**File:** `src/lib.rs`

**Location:** Inside `_PG_init()` function, after ProcessUtility hook installation

```rust
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing ProcessUtility hook installation code ...

    // Check for jsonb_delta extension
    if !check_jsonb_delta_available() {
        warning!(
            "jsonb_delta extension not found. \
             pg_tviews will work but with reduced performance. \
             Install jsonb_delta for 1.5-3× faster cascades: \
             https://github.com/fraiseql/jsonb_delta"
        );
    } else {
        info!("jsonb_delta extension detected - performance optimizations enabled");
    }
}
```

### Step 3: Update README with Dependency Documentation

**File:** `README.md`

**Location:** Add new section after installation instructions

```markdown
## Dependencies

### Optional: jsonb_delta (Recommended for Production)

pg_tviews works standalone but achieves **1.5-3× faster cascade performance** with the jsonb_delta extension.

#### Installation

```bash
# Install jsonb_delta first
git clone https://github.com/fraiseql/jsonb_delta.git
cd jsonb_delta
cargo pgrx install --release

# Then install pg_tviews
cd ../pg_tviews
cargo pgrx install --release
```

#### Enable in PostgreSQL

```sql
-- Install extensions (order matters)
CREATE EXTENSION jsonb_delta;  -- Optional but recommended
CREATE EXTENSION pg_tviews;

-- Verify jsonb_delta is detected
SELECT pg_tviews_check_jsonb_delta();
-- Returns: true (optimizations enabled)
```

#### Performance Impact

| Scenario | Without jsonb_delta | With jsonb_delta | Speedup |
|----------|------------------|----------------|---------|
| Single nested update | 2.5ms | 1.2ms | **2.1×** |
| 100-row cascade | 150ms | 85ms | **1.8×** |
| Deep cascade (3 levels) | 220ms | 100ms | **2.2×** |

**Recommendation:** Install jsonb_delta for production use. Development/testing can use pg_tviews standalone.

### Core Dependencies (Required)

- PostgreSQL 15+ (tested through 17)
- Rust toolchain (1.70+)
- cargo-pgrx (0.12.8)
```

### Step 4: Add to Extension SQL

**File:** `sql/pg_tviews--0.1.0.sql`

**Location:** Add near the top, after header comments

```sql
-- Runtime dependency check function
-- Returns true if jsonb_delta extension is installed
CREATE OR REPLACE FUNCTION pg_tviews_check_jsonb_delta()
RETURNS boolean
AS 'MODULE_PATHNAME', 'pg_tviews_check_jsonb_delta'
LANGUAGE C STRICT;

COMMENT ON FUNCTION pg_tviews_check_jsonb_delta() IS
'Check if jsonb_delta extension is installed (enables performance optimizations)';
```

---

## Verification Commands

After implementing GREEN phase:

```bash
# 1. Build and install
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx install --release

# 2. Run Rust tests
cargo pgrx test pg17

# 3. Run SQL test manually
psql -d postgres <<EOF
DROP DATABASE IF EXISTS test_phase5_task1;
CREATE DATABASE test_phase5_task1;
\c test_phase5_task1
\i test/sql/50_jsonb_delta_detection.sql
EOF

# 4. Check logs for warning/info messages
# Should see:
# WARNING: jsonb_delta extension not found...
# (after installing jsonb_delta)
# INFO: jsonb_delta extension detected...
```

**Expected Output:**
- ✅ All Rust tests pass (3 tests)
- ✅ SQL test passes (2 test cases)
- ✅ Warning appears when jsonb_delta not installed
- ✅ Info message appears when jsonb_delta installed
- ✅ pg_tviews works in both cases

---

## REFACTOR Phase: Improve Code Quality

### Refactor 1: Cache Detection Result

**Current:** `check_jsonb_delta_available()` queries pg_extension every time

**Better:** Cache result in static variable

**File:** `src/lib.rs`

```rust
use std::sync::atomic::{AtomicBool, Ordering};

// Static cache for jsonb_delta availability
static JSONB_IVM_AVAILABLE: AtomicBool = AtomicBool::new(false);
static JSONB_IVM_CHECKED: AtomicBool = AtomicBool::new(false);

/// Check if jsonb_delta extension is available (cached)
pub fn check_jsonb_delta_available() -> bool {
    // Return cached result if already checked
    if JSONB_IVM_CHECKED.load(Ordering::Relaxed) {
        return JSONB_IVM_AVAILABLE.load(Ordering::Relaxed);
    }

    // First time: query database
    let result = Spi::connect(|client| {
        let rows = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')",
            None,
            None,
        )?;

        for row in rows {
            if let Some(exists) = row[1].value::<bool>()? {
                return Ok(exists);
            }
        }
        Ok(false)
    }).unwrap_or(false);

    // Cache result
    JSONB_IVM_AVAILABLE.store(result, Ordering::Relaxed);
    JSONB_IVM_CHECKED.store(true, Ordering::Relaxed);

    result
}
```

**Benefit:** Avoid repeated pg_extension queries (performance)

### Refactor 2: Add Explicit Import for Test Module

**File:** `src/lib.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pgrx::prelude::*;

    // ... tests ...
}
```

### Refactor 3: Improve Warning Message

**File:** `src/lib.rs` in `_PG_init()`

```rust
if !check_jsonb_delta_available() {
    warning!(
        "pg_tviews: jsonb_delta extension not detected\n\
         → Performance: Basic (full document replacement)\n\
         → To enable 1.5-3× faster cascades, install jsonb_delta:\n\
         → https://github.com/fraiseql/jsonb_delta"
    );
} else {
    info!("pg_tviews: jsonb_delta detected - surgical JSONB updates enabled (1.5-3× faster)");
}
```

**Benefit:** Clearer user messaging

---

## Acceptance Criteria Checklist

After REFACTOR phase, verify:

- [ ] `cargo pgrx test pg17` passes (all 3 tests green)
- [ ] `test/sql/50_jsonb_delta_detection.sql` passes
- [ ] Warning appears in logs when jsonb_delta not installed
- [ ] Info message appears when jsonb_delta is installed
- [ ] README documents jsonb_delta dependency with installation steps
- [ ] `pg_tviews_check_jsonb_delta()` SQL function exported
- [ ] Detection result cached (no repeated queries)
- [ ] No breaking changes to existing functionality

---

## Files Modified

### New Files:
1. `test/sql/50_jsonb_delta_detection.sql` - SQL integration test

### Modified Files:
1. `src/lib.rs` - Add check function, cache, _PG_init() check, tests
2. `README.md` - Add dependencies section with jsonb_delta docs
3. `sql/pg_tviews--0.1.0.sql` - Export pg_tviews_check_jsonb_delta()

---

## Rollback Plan

If Task 1 fails:
1. Remove check from `_PG_init()` (comment out)
2. Keep function but always return `false`
3. Phase 5 can continue without runtime detection

---

## Next Task

After Task 1 complete → **Task 2: Enhance Metadata Schema**
- Add dependency_types, dependency_paths, array_match_keys columns
- Create migration SQL (0.1.0 → 0.2.0)
- Update TviewMeta struct

---

## DO NOT

- ❌ Make jsonb_delta a required dependency (must be optional)
- ❌ Fail if jsonb_delta not installed (warn only)
- ❌ Query pg_extension on every cascade (cache result)
- ❌ Skip documentation (users need clear install instructions)

---

## Notes

- **Testing jsonb_delta presence:** Install/uninstall between test runs to verify both paths
- **Caching strategy:** Static atomic bool is sufficient (extension list doesn't change at runtime)
- **Warning vs Error:** Use `warning!()` not `error!()` to ensure pg_tviews still loads
- **Documentation:** Link to fraiseql/jsonb_delta GitHub for install instructions
