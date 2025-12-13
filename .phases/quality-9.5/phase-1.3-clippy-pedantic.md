# Phase 1.3: Clippy Pedantic Compliance

**Objective**: Enable and fix all clippy::pedantic warnings for production-grade code quality

**Priority**: HIGH
**Estimated Time**: 1-2 days
**Blockers**: Phase 1.2 complete (unwrap elimination)

---

## Context

**Current State**: Clippy pedantic is disabled

```toml
# Cargo.toml
[lints.clippy]
all = { level = "deny", priority = -1 }
# pedantic = "warn"  # TODO: Enable after fixing warnings
```

**Why This Matters**:
- `clippy::pedantic` catches code smells and anti-patterns
- Enforces Rust idioms and best practices
- Improves readability and maintainability
- Industry standard for high-quality Rust projects

**Expected Warning Count**: 50-150 warnings (based on 11,632 LOC)

---

## Strategy

### Phased Enablement

1. **Phase A**: Run pedantic, categorize warnings
2. **Phase B**: Fix low-hanging fruit (auto-fixable)
3. **Phase C**: Manual fixes for complex warnings
4. **Phase D**: Enable pedantic lint, allow specific exceptions
5. **Phase E**: Incremental fixes until zero warnings

---

## Files to Modify

1. `Cargo.toml` - Enable pedantic lints
2. All `src/**/*.rs` files - Fix warnings
3. `src/lib.rs` - Add allow directives for justified exceptions

---

## Implementation Steps

### Step 1: Baseline Assessment

**Run pedantic without enforcing**:
```bash
# Generate warning report
cargo clippy --all-targets -- -W clippy::pedantic 2>&1 | tee /tmp/pedantic-warnings.txt

# Count warnings by type
rg "warning: " /tmp/pedantic-warnings.txt | cut -d':' -f4 | sort | uniq -c | sort -rn
```

**Expected warning categories**:
- `must_use_candidate` - Functions should return Result
- `missing_errors_doc` - Document error cases (already allowed)
- `module_name_repetitions` - pg_tviews_ prefix (already allowed)
- `similar_names` - Variable naming
- `too_many_lines` - Function length (will fix in Phase 1.4)
- `cast_possible_truncation` - Numeric conversions
- `cast_sign_loss` - Signed/unsigned conversions

### Step 2: Enable Auto-Fixes

**Apply automatic fixes**:
```bash
# Let clippy fix what it can
cargo clippy --fix --allow-dirty --all-targets -- -W clippy::pedantic

# Review changes
git diff

# Test that nothing broke
cargo test --all
cargo pgrx test pg17
```

**Common auto-fixes**:
- Add `#[must_use]` attributes
- Use `into()` instead of explicit `From::from()`
- Replace `match` with `if let`
- Use iterator methods instead of loops

### Step 3: Manual Fixes - Common Patterns

#### Pattern 1: `must_use_candidate`

**Before**:
```rust
pub fn refresh_tview(entity: &str) -> TViewResult<()> {
    // ... refresh logic
    Ok(())
}
```

**After**:
```rust
#[must_use = "Refresh result must be checked"]
pub fn refresh_tview(entity: &str) -> TViewResult<()> {
    // ... refresh logic
    Ok(())
}
```

#### Pattern 2: `similar_names`

**Before**:
```rust
let tview_name = get_tview_name();
let tview_nam = format!("tv_{}", entity);  // Typo/similar
```

**After**:
```rust
let tview_name = get_tview_name();
let tview_table = format!("tv_{}", entity);  // Distinct name
```

#### Pattern 3: `cast_possible_truncation`

**Before**:
```rust
let count = row_count as i32;  // i64 -> i32, may truncate
```

**After**:
```rust
let count = i32::try_from(row_count)
    .map_err(|_| TViewError::InvalidValue {
        value: row_count.to_string(),
        context: "row count exceeds i32::MAX".to_string(),
    })?;
```

**Or, if truncation is acceptable**:
```rust
#[allow(clippy::cast_possible_truncation)]
let count = row_count as i32;  // Intentional: PostgreSQL limits to i32
```

#### Pattern 4: `missing_panics_doc`

**Before**:
```rust
/// Refreshes the TVIEW
pub fn refresh(entity: &str) -> TViewResult<()> {
    let metadata = get_metadata(entity).expect("metadata must exist");
    // ...
}
```

**After**:
```rust
/// Refreshes the TVIEW
///
/// # Errors
/// Returns error if metadata not found or refresh fails
///
/// # Panics
/// Panics if metadata is missing (indicates bug in TVIEW setup)
pub fn refresh(entity: &str) -> TViewResult<()> {
    let metadata = get_metadata(entity).expect("metadata must exist");
    // ...
}
```

**Better**: Eliminate panic (covered in Phase 1.2)
```rust
/// Refreshes the TVIEW
///
/// # Errors
/// Returns `MetadataNotFound` if TVIEW not registered
pub fn refresh(entity: &str) -> TViewResult<()> {
    let metadata = get_metadata(entity)?;
    // ...
}
```

### Step 4: Enable Pedantic in Cargo.toml

**File**: `Cargo.toml`

```toml
[lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = 0 }  # ✅ Enable as warnings

# Allow specific pedantic lints that conflict with PostgreSQL FFI
missing_errors_doc = "allow"        # FFI functions can't document all PG errors
module_name_repetitions = "allow"   # pg_tviews_ prefix is intentional

# Additional allows for justified exceptions
too_many_lines = "allow"           # Will fix in Phase 1.4
cast_possible_truncation = "allow" # PostgreSQL uses i32/i64 casts extensively
cast_sign_loss = "allow"           # OID conversions require this
```

**Rationale**: Start with `warn` to see all issues without blocking builds. After fixes, promote to `deny`.

### Step 5: File-Specific Allows

**For files with justified exceptions**:

**File**: `src/refresh/main.rs`

```rust
// This file has complex SQL generation - some pedantic warnings are acceptable
#![allow(clippy::too_many_lines)]  // Will refactor in Phase 1.4

pub fn refresh_bulk(...) -> TViewResult<()> {
    // Large function - acceptable for now
    #[allow(clippy::cast_possible_truncation)]
    let oid = table_oid as u32;  // OID is u32 in PostgreSQL

    // ... rest of function
}
```

### Step 6: Progressive Cleanup

**Week-by-week approach**:

**Week 1**: Core modules (highest impact)
- `src/lib.rs`
- `src/error/mod.rs`
- `src/refresh/main.rs`

**Week 2**: Domain logic
- `src/catalog.rs`
- `src/dependency/graph.rs`
- `src/queue/`

**Week 3**: Peripheral modules
- `src/ddl/`
- `src/schema/`
- `src/validation.rs`

### Step 7: Promote to Deny

**After all warnings fixed**:

**File**: `Cargo.toml`

```toml
[lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "deny", priority = 0 }  # ✅ Zero tolerance

# Keep justified allows
missing_errors_doc = "allow"
module_name_repetitions = "allow"
```

---

## Verification Commands

```bash
# 1. Check current warning count
cargo clippy --all-targets -- -W clippy::pedantic 2>&1 | rg "^warning" | wc -l

# 2. Verify zero warnings (after fixes)
cargo clippy --all-targets -- -D clippy::pedantic

# 3. Check specific warning types
cargo clippy --all-targets -- -W clippy::pedantic 2>&1 | rg "must_use_candidate"

# 4. Run tests to ensure nothing broke
cargo test --all
cargo pgrx test pg17

# 5. Build in release mode
cargo build --release

# 6. Verify allow directives are justified
rg "#\[allow\(clippy::" src/ | wc -l
# Should be minimal (<20)
```

---

## Common Warnings and Fixes

| Warning | Description | Fix |
|---------|-------------|-----|
| `must_use_candidate` | Function returns Result/Option, should be checked | Add `#[must_use]` |
| `missing_errors_doc` | Missing `# Errors` section | Add doc comment or allow |
| `missing_panics_doc` | Missing `# Panics` section | Document or eliminate panic |
| `similar_names` | Variables with similar names | Rename for clarity |
| `too_many_lines` | Function >100 lines | Refactor in Phase 1.4 |
| `cast_possible_truncation` | i64 -> i32 cast | Use `try_from()` or allow |
| `cast_sign_loss` | Signed to unsigned | Use checked cast or allow |
| `items_after_statements` | Item defined after statement | Move to top of block |
| `inline_always` | Overuse of `#[inline(always)]` | Use `#[inline]` or remove |
| `wildcard_imports` | `use foo::*;` | Import explicitly |

---

## Acceptance Criteria

- [x] `clippy::pedantic` enabled in Cargo.toml
- [x] Zero pedantic warnings with `-D clippy::pedantic`
- [x] All `#[must_use]` attributes added appropriately
- [x] Documentation includes `# Errors` and `# Panics` sections
- [x] Numeric casts use `try_from()` or are explicitly allowed
- [x] All tests pass
- [x] No performance regression
- [x] Allow directives are documented and justified

---

## DO NOT

- ❌ Blanket allow pedantic warnings - fix them properly
- ❌ Add `#[allow]` without a comment explaining why
- ❌ Refactor large functions (save for Phase 1.4)
- ❌ Change public API (save for Phase 4)
- ❌ Fix warnings in test code (lower priority)

---

## Performance Impact

**Pedantic fixes should have ZERO performance impact**:
- Most fixes are style/documentation
- `#[must_use]` is compile-time only
- `try_from()` may add bounds checking (negligible)

**Benchmark verification**:
```bash
# Before fixes
cargo bench > /tmp/bench-before.txt

# After fixes
cargo bench > /tmp/bench-after.txt

# Compare
diff -u /tmp/bench-before.txt /tmp/bench-after.txt
# Should show no significant changes
```

---

## Documentation Template

**For public functions**:

```rust
/// Brief description (one line)
///
/// Longer description explaining what this does,
/// when to use it, and any important context.
///
/// # Arguments
/// * `entity` - The TVIEW entity name (without tv_ prefix)
/// * `force` - Whether to force refresh even if up-to-date
///
/// # Returns
/// Returns `Ok(())` on success, or error if refresh fails.
///
/// # Errors
/// This function returns an error if:
/// - TVIEW metadata not found (`MetadataNotFound`)
/// - Circular dependency detected (`CircularDependency`)
/// - Database connection fails (`DatabaseError`)
///
/// # Panics
/// This function does not panic. (Or: "Panics if metadata is corrupted")
///
/// # Examples
/// ```rust,ignore
/// refresh_tview("user_posts")?;
/// ```
#[must_use = "Refresh result must be checked"]
pub fn refresh_tview(entity: &str, force: bool) -> TViewResult<()> {
    // implementation
}
```

---

## Rollback Plan

If pedantic enforcement causes issues:

```toml
# Temporarily downgrade to warnings
[lints.clippy]
pedantic = { level = "warn", priority = 0 }
```

```bash
# Revert specific file changes
git checkout HEAD -- src/problematic_file.rs
```

---

## Next Steps

After completion:
- Commit with message: `refactor(quality): Enable clippy::pedantic compliance [PHASE1.3]`
- Update documentation: Add clippy badge to README
- Proceed to **Phase 1.4: Refactor Large Functions**
