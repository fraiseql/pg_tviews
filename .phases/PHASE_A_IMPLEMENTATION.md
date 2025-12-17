# Phase A: Low-Risk, High-Value Implementation Guide

**Errors to Fix**: 9
**Estimated Time**: 1-2 hours including testing

---

## Phase 1: Test Module Import Cleanup (2 errors)

### Problem
```
error: unused import: `pgrx::prelude::*`
   --> src/lib.rs:792:9

error: unused import: `crate::error::TViewError`
   --> src/lib.rs:793:9
```

### Root Cause
When building with `--no-default-features`, the test module functions are gated by `#[cfg(feature = "pg_test")]`, so the imports inside the module scope are unused.

### Solution
Wrap the imports with the same feature gate as the test functions.

### Current Code (src/lib.rs, around line 789-800)
```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;  // ← Line 792: UNUSED
    use crate::error::TViewError;  // ← Line 793: UNUSED

    #[cfg(feature = "pg_test")]
    use pgrx_tests::pg_test;

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_version_function() {
        // ...
    }
}
```

### Fixed Code
```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    #[cfg(feature = "pg_test")]
    use pgrx::prelude::*;

    #[cfg(feature = "pg_test")]
    use crate::error::TViewError;

    #[cfg(feature = "pg_test")]
    use pgrx_tests::pg_test;

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_version_function() {
        // ...
    }
}
```

### Why This Works
- `#[cfg(feature = "pg_test")]` ensures imports are only included when tests are actually compiled
- This matches how the `pg_test` macro is already gated
- Clean and follows Rust conventions

---

## Phase 2: Const Function Migrations (6 errors)

### Problem
Functions that only return constants should be marked `const fn` for compile-time evaluation.

### Benefit
- Better performance (evaluated at compile time)
- Can be used in const contexts
- Better optimization by compiler

### Error Pattern
```
error: this could be a `const fn`
  --> src/refresh/cache.rs:24:1
```

### Solution Pattern

#### Location 1: `src/refresh/cache.rs:24`
```rust
// Before:
pub fn some_default_value() -> usize {
    100
}

// After:
pub const fn some_default_value() -> usize {
    100
}
```

**Action**: Add `const` keyword between `pub` and `fn`.

#### Location 2: `src/queue/persistence.rs:143`
Same pattern - add `const` keyword.

#### Location 3: `src/error/mod.rs:172`
```rust
// Before:
impl SomeType {
    fn new_error() -> Self {
        Self::Variant(...)
    }
}

// After:
impl SomeType {
    const fn new_error() -> Self {
        Self::Variant(...)
    }
}
```

#### Location 4: `src/error/mod.rs:197`
Same pattern as location 3.

#### Location 5: `src/config/mod.rs:32` (max_propagation_depth)
```rust
// Before:
pub fn max_propagation_depth() -> usize {
    100
}

// After:
pub const fn max_propagation_depth() -> usize {
    100
}
```

#### Location 6: `src/config/mod.rs:39+` (graph_cache_enabled, table_cache_enabled, log_level, metrics_enabled)
```rust
// Before:
pub fn graph_cache_enabled() -> bool {
    true
}

// After:
pub const fn graph_cache_enabled() -> bool {
    true
}
```

Repeat for all similar functions.

#### Location 7: `src/schema/analyzer.rs:53`
Same pattern - add `const` keyword.

### Implementation Notes
- Only add `const` if function body is actually const-evaluable
- All the functions listed above contain only literals, so they're safe
- No complex logic or function calls in const functions

---

## Phase 8: Dependency Resolution (1 error)

### Problem
```
error: multiple versions for dependency `hashbrown`: 0.15.5, 0.16.1
```

### Root Cause
Transitive dependencies are pulling in two different versions of `hashbrown`.

### Solution

#### Step 1: Run cargo update
```bash
cargo update
```

This will attempt to resolve to a single version.

#### Step 2: Check Cargo.lock
```bash
grep -A 5 "name = \"hashbrown\"" Cargo.lock
```

Should show only one version.

#### Step 3: If still failing
Identify which crate depends on the older version:
```bash
cargo tree | grep hashbrown
```

Look for mismatches. May need to update a dependency to resolve.

#### Step 4: Verify
```bash
cargo build --no-default-features --features pg16
```

Should complete without the hashbrown error.

### Typical Fix
Usually one of these:
- Update pgrx or related dependencies
- Update serde or related crates
- Update any recently added dependencies

---

## Testing Phase A

### Local Verification
```bash
# Build with Clippy to verify fixes
cargo clippy --no-default-features --features pg16 -- -D warnings

# Expected output: Reduced error count from 55 to ~46
```

### Expected Results After Phase A
- Import unused errors: ✅ Fixed (2 errors)
- Const fn errors: ✅ Fixed (6 errors)
- Hashbrown error: ✅ Fixed (1 error)
- **Total Fixed**: 9 errors
- **Remaining**: ~46 errors

### Commit Message Template
```
fix(clippy): Phase A - Test imports and const functions

- Wrap test module imports with #[cfg(feature = "pg_test")]
- Add const keyword to compile-time evaluated functions
- Resolve hashbrown dependency conflict

Fixed errors:
- Unused imports in test module (2)
- Missing const fn markers (6)
- Dependency version conflict (1)
```

---

## Delegation to Local Model

### Prompt for Ministral-3-8B-Instruct

For Phase 1 (Test Imports):
```
Task: Fix unused import warnings in test modules by adding feature gates.

Pattern:
Current: use pgrx::prelude::*;
Fixed: #[cfg(feature = "pg_test")]\nuse pgrx::prelude::*;

Apply this pattern to ALL imports inside the test module that are only used
within #[pg_test] functions.

File: src/lib.rs (lines 792-793)
Module: mod tests starting at line 790

Only modify the imports. Leave the test functions unchanged.
Show the corrected import section.
```

For Phase 2 (Const Functions):
```
Task: Add const keyword to functions that return only constant values.

Pattern:
Before: pub fn max_propagation_depth() -> usize { 100 }
After:  pub const fn max_propagation_depth() -> usize { 100 }

Apply this to each location:
1. src/refresh/cache.rs:24 - const function
2. src/queue/persistence.rs:143 - const function
3. src/error/mod.rs:172 - impl function
4. src/error/mod.rs:197 - impl function
5. src/config/mod.rs:32+ - 5 config functions
6. src/schema/analyzer.rs:53 - const function

For each file and line number, show ONLY the function signature (1 line).
```

For Phase 8 (Dependencies):
```
Task: Resolve hashbrown version conflict

Run: cargo update

If that doesn't work:
1. Check Cargo.lock for multiple hashbrown versions
2. Identify which dependency needs updating
3. Suggest which crate version should be updated

Provide the exact cargo update command or dependency version to change.
```

---

## Checklist

- [ ] Phase 1 complete: Test imports fixed
- [ ] Phase 2 complete: Const functions added
- [ ] Phase 8 complete: Hashbrown resolved
- [ ] Local build passes without Phase A errors
- [ ] Commit created with descriptive message
- [ ] Push to dev branch
- [ ] Verify CI runs and captures the fix

