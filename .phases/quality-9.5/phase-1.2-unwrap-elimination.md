# Phase 1.2: Unwrap Elimination

**Objective**: Eliminate all 180 `unwrap()` calls from production code, replacing with proper error handling

**Priority**: CRITICAL
**Estimated Time**: 1-2 days
**Blockers**: Phase 1.1 complete

---

## Context

**Current State**: 180 `unwrap()` calls across 19 files

```bash
# From codebase analysis
src/refresh/main.rs: 78 unwrap() calls  # ⚠️ HIGHEST RISK
src/dependency/graph.rs: 15 unwrap() calls
src/schema/inference.rs: 8 unwrap() calls
# ... 16 more files
```

**Why This Matters**:
- `unwrap()` causes **panic** on None/Err
- Panics in FFI code can **crash PostgreSQL**
- Production extensions must **never panic** in error paths
- This is the **#1 technical debt** preventing 9.5/10 rating

---

## Strategy

### Three-Tier Approach

1. **Tier 1: Replace with `?` operator** (80% of cases)
   - Functions that return `Result<T, TViewError>`
   - Clean propagation up the call stack

2. **Tier 2: Replace with safe defaults** (15% of cases)
   - Optional values where None is valid
   - Use `unwrap_or_default()`, `unwrap_or_else()`

3. **Tier 3: Add contextual errors** (5% of cases)
   - Cases where unwrap indicates programmer error
   - Convert to `expect()` with clear message (for debugging)
   - Eventually eliminate in Phase 1.4 (refactoring)

---

## Files to Modify (Priority Order)

### High Priority (Tier 1 - Critical Path)
1. `src/refresh/main.rs` (78 calls) - Core refresh logic
2. `src/dependency/graph.rs` (15 calls) - Dependency resolution
3. `src/schema/inference.rs` (8 calls) - Type inference
4. `src/queue/ops.rs`, `src/queue/cache.rs` - Transaction queue

### Medium Priority (Tier 2)
5. `src/ddl/convert.rs` - TVIEW conversion
6. `src/hooks.rs` - PostgreSQL hooks
7. `src/validation.rs` - Input validation

### Lower Priority (Tier 3)
8-19. Remaining 12 files with <5 calls each

---

## Implementation Steps

### Step 1: Add Deny Lint (Enforcement)

**File**: `src/lib.rs` (top of file, after imports)

```rust
// Deny unwrap() in production code (Phase 1.2)
#![deny(clippy::unwrap_used)]
// Allow in tests
#![cfg_attr(test, allow(clippy::unwrap_used))]
```

**Expected**: This will cause **compilation to fail** until all unwraps are fixed.

### Step 2: Systematic Fix - `refresh/main.rs`

**Pattern 1: SPI query results**

**Before**:
```rust
let row_count = row["count"].value::<i64>()?.unwrap_or(0);
let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
```

**After** (already safe!):
```rust
// These are CORRECT - unwrap_or provides fallback
let row_count = row["count"].value::<i64>()?.unwrap_or(0);
let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
```

**Pattern 2: Result propagation**

**Before**:
```rust
let metadata = get_tview_metadata(table_name).unwrap();
let deps = resolve_dependencies(&metadata).unwrap();
```

**After**:
```rust
let metadata = get_tview_metadata(table_name)?;
let deps = resolve_dependencies(&metadata)?;
```

**Pattern 3: Option extraction in infallible contexts**

**Before**:
```rust
let cache = CACHE.lock().unwrap();
let entry = cache.get(&key).unwrap();
```

**After**:
```rust
use std::sync::PoisonError;

// For Mutex - poison errors are unrecoverable
let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);

// For Option - use ok_or to convert to Result
let entry = cache.get(&key)
    .ok_or_else(|| TViewError::CacheMiss {
        key: key.to_string()
    })?;
```

**Pattern 4: String formatting (safe unwrap)**

**Before**:
```rust
let json_str = serde_json::to_string(&metadata).unwrap();
```

**After**:
```rust
// Serialization of internal types should never fail
// Use expect() with clear message for debugging
let json_str = serde_json::to_string(&metadata)
    .expect("BUG: Failed to serialize metadata - invalid internal state");
```

### Step 3: Batch Fix by File

**Process**:
1. Comment out `#![deny(clippy::unwrap_used)]`
2. Run: `cargo clippy --fix --allow-dirty -- -W clippy::unwrap_used`
3. Review automated fixes
4. Manual fixes for complex cases
5. Uncomment deny lint
6. Verify compilation

**File-by-file approach** (one commit per file for large files):

```bash
# Fix refresh/main.rs first (highest impact)
# 1. Analyze patterns
rg "\.unwrap\(\)" src/refresh/main.rs -C 2 > /tmp/unwraps.txt

# 2. Replace systematically
# (Manual editing with patterns above)

# 3. Test after each batch of 10-20 fixes
cargo test refresh::tests
cargo pgrx test pg17

# 4. Commit
git add src/refresh/main.rs
git commit -m "refactor(refresh): Eliminate unwrap() calls [PHASE1.2]"
```

### Step 4: Add Error Variants (If Needed)

**File**: `src/error/mod.rs`

If new error cases are discovered, add variants:

```rust
pub enum TViewError {
    // ... existing variants ...

    /// Cache miss when entry should exist
    CacheMiss {
        key: String,
    },

    /// Internal serialization failure
    SerializationFailed {
        type_name: &'static str,
        reason: String,
    },

    /// Mutex poison error (should be rare)
    LockPoisoned {
        resource: String,
    },
}
```

**Add Display implementations**:
```rust
TViewError::CacheMiss { key } => {
    write!(f, "Cache entry not found: {}", key)
}
```

### Step 5: Test Coverage for Error Paths

**File**: `src/refresh/main.rs` (or new test file)

```rust
#[cfg(test)]
mod error_handling_tests {
    use super::*;

    #[test]
    fn test_missing_metadata_returns_error() {
        let result = get_tview_metadata("nonexistent_table");
        assert!(result.is_err());
        match result.unwrap_err() {
            TViewError::MetadataNotFound { entity } => {
                assert_eq!(entity, "nonexistent_table");
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_cache_miss_returns_error() {
        // Test cache lookup with missing key
        // Should return CacheMiss error, not panic
    }
}
```

---

## Verification Commands

```bash
# 1. Count remaining unwraps (should be 0 in src/, OK in tests)
rg "\.unwrap\(\)" src/ --type rust | wc -l
# Expected: 0

# 2. Verify deny lint is active
rg "#\!\[deny\(clippy::unwrap_used\)\]" src/lib.rs
# Expected: Found

# 3. Run full test suite
cargo test --all

# 4. Run clippy with unwrap detection
cargo clippy --all-targets -- -D clippy::unwrap_used

# 5. Integration tests
cargo pgrx test pg17

# 6. Verify error handling works
cargo pgrx run pg17
# In psql, test error cases:
SELECT pg_tviews_metadata('nonexistent');  -- Should error gracefully
```

---

## Acceptance Criteria

- [x] Zero `unwrap()` calls in `src/` directory (excluding tests)
- [x] `#![deny(clippy::unwrap_used)]` enabled in `src/lib.rs`
- [x] All error paths return proper `TViewError` variants
- [x] No panics in FFI functions (all wrapped with `?` or safe defaults)
- [x] Tests added for new error paths
- [x] All existing tests still pass
- [x] No performance regression (cached paths still fast)

---

## DO NOT

- ❌ Replace `unwrap()` in test code (tests can panic)
- ❌ Add `expect()` everywhere - use proper error propagation
- ❌ Ignore lock poisoning - handle with `unwrap_or_else`
- ❌ Skip adding new error variants when needed
- ❌ Batch commit all changes - commit per file for reviewability

---

## Common Patterns Cheatsheet

```rust
// ❌ BEFORE                        ✅ AFTER

// Result unwrap
value.unwrap()                      value?

// Option unwrap
opt.unwrap()                        opt.ok_or(TViewError::...)?

// Result with default
result.unwrap_or(default)           result.unwrap_or(default)  // ✅ OK

// Mutex lock
mutex.lock().unwrap()               mutex.lock().unwrap_or_else(PoisonError::into_inner)

// Infallible operations (known safe)
json.to_string().unwrap()           json.to_string()
                                        .expect("BUG: serialization failed")

// HashMap/Cache lookup
cache.get(&k).unwrap()              cache.get(&k).ok_or(TViewError::CacheMiss{...})?
```

---

## Risk Mitigation

**Risk**: Breaking existing functionality

**Mitigation**:
1. Fix one file at a time
2. Run tests after each file
3. Keep commits small and atomic
4. Test integration with `cargo pgrx test` after each batch

**Risk**: Performance regression from error handling

**Mitigation**:
1. Profile hot paths before/after (use `cargo flamegraph`)
2. Error construction is lazy (only on error path)
3. `?` operator is zero-cost in success path

**Risk**: Uncovering hidden bugs

**Mitigation**:
1. This is GOOD - unwraps hide bugs
2. Add proper tests for newly discovered error cases
3. Document expected error behavior

---

## Performance Check

**Before/after benchmark**:
```bash
# Run benchmarks before changes
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh > before.txt

# After all unwrap fixes
./run_benchmarks.sh > after.txt

# Compare (should be identical)
diff -u before.txt after.txt
```

---

## Next Steps

After completion:
- Commit with message: `refactor(core): Eliminate all unwrap() calls [PHASE1.2]`
- Run full benchmark suite to verify no regressions
- Proceed to **Phase 1.3: Clippy Pedantic Compliance**
