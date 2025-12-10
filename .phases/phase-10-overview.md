# Phase 10: Clippy-Strict Compliance & Code Quality Hardening

**Status:** PLANNED (Post-Phase 7)
**Prerequisites:** Phase 7 Complete ✅
**Estimated Time:** 1-2 weeks
**Priority:** Medium (Code quality & maintainability)

---

## Objective

Make the codebase compliant with strict clippy lints and establish high code quality standards:

1. **Eliminate `unwrap()` / `panic!()`**: Replace with proper error handling
2. **Pedantic Lint Compliance**: Address all pedantic clippy warnings
3. **Nursery Lint Compliance**: Address experimental but valuable lints
4. **Documentation Quality**: Improve doc coverage and examples
5. **Error Handling Audit**: Ensure consistent error patterns throughout

---

## Context

### Current State (Phase 7)

**Baseline Quality:**
- ✅ 0 warnings with default clippy (`-D warnings`)
- ✅ Clean build with `cargo build --release`
- ✅ Basic documentation present

**Identified Issues:**
- ⚠️ **183 `unwrap()` calls** - Potential panic points
- ⚠️ **10 `expect()` calls** - Better than unwrap but still can panic
- ⚠️ **5 `panic!()` calls** - Explicit panics
- ⚠️ **11 TODO comments** - Unfinished work markers
- ⚠️ **Inconsistent error handling** - Mix of Result/Option/panic patterns

### Why Strict Clippy Compliance?

**Benefits:**
1. **Robustness**: Eliminate panic scenarios in production
2. **Maintainability**: Consistent code patterns easier to understand
3. **Safety**: Catch potential bugs earlier
4. **Best Practices**: Follow Rust community standards
5. **Documentation**: Better API docs for users

**PostgreSQL Extension Considerations:**
- Panics in C callbacks can corrupt database state
- Must handle all errors gracefully
- FFI boundaries require extra care

---

## Sub-Phases

Phase 10 is divided into 5 sub-tasks:

| Sub-Phase | Focus | Time | Dependencies |
|-----------|-------|------|--------------|
| **10A** | Unwrap Elimination | 3-4 days | Phase 7 ✅ |
| **10B** | Pedantic Lint Compliance | 2-3 days | Phase 10A |
| **10C** | Error Handling Audit | 2 days | Phase 10A |
| **10D** | Documentation Enhancement | 1-2 days | Phase 7 ✅ |
| **10E** | CI/CD Lint Integration | 1 day | Phase 10A-10D |

---

## Phase 10A: Unwrap Elimination

### Audit Current Unwrap Usage

**Locations with unwrap() calls:**
```bash
# Audit command:
grep -rn "unwrap()" src/ --include="*.rs" > unwrap_audit.txt
# Found: 183 instances across ~7,359 lines
```

**Categories of unwrap():**
1. **Mutex lock unwraps** (~50 instances) - Acceptable (poisoned mutex is fatal)
2. **Serde unwraps** (~30 instances) - Need proper error handling
3. **SPI unwraps** (~40 instances) - Need TViewError conversion
4. **HashMap unwraps** (~20 instances) - Need Option handling
5. **String unwraps** (~20 instances) - Need validation
6. **Other unwraps** (~23 instances) - Case-by-case analysis

### Strategy

**1. Acceptable unwraps (keep with justification):**
```rust
// BEFORE: Silent unwrap
let cache = ENTITY_GRAPH_CACHE.lock().unwrap();

// AFTER: Documented justification
let cache = ENTITY_GRAPH_CACHE.lock()
    .expect("ENTITY_GRAPH_CACHE mutex poisoned - fatal error, cannot recover");
```

**2. Unwraps that should return errors:**
```rust
// BEFORE: Panics on serialization error
fn to_jsonb(self) -> JsonB {
    let json = serde_json::to_value(self).unwrap();
    JsonB(json)
}

// AFTER: Returns Result
fn to_jsonb(self) -> TViewResult<JsonB> {
    let json = serde_json::to_value(self)
        .map_err(|e| TViewError::SerializationError {
            message: format!("Failed to serialize to JSONB: {}", e),
        })?;
    Ok(JsonB(json))
}
```

**3. Unwraps that should use default values:**
```rust
// BEFORE: Panics if key missing
let entity = cache.get(&table_oid).unwrap();

// AFTER: Returns None
let entity = cache.get(&table_oid);
```

**4. Unwraps in unsafe FFI contexts:**
```rust
// BEFORE: Can panic in C callback
unsafe extern "C" fn callback(...) {
    let result = some_operation().unwrap();
}

// AFTER: Log error and return early
unsafe extern "C" fn callback(...) {
    let result = match some_operation() {
        Ok(r) => r,
        Err(e) => {
            error!("Operation failed in FFI callback: {:?}", e);
            return; // Or appropriate error handling
        }
    };
}
```

### Implementation Checklist

- [ ] Audit all 183 unwrap() calls
- [ ] Categorize each unwrap (acceptable vs must-fix)
- [ ] Create TViewError variants for new error cases
- [ ] Replace unwraps in hot paths first (triggers, commit hooks)
- [ ] Replace unwraps in FFI callbacks (safety critical)
- [ ] Replace unwraps in serialization code
- [ ] Document acceptable unwraps with `expect()` and justification
- [ ] Add unit tests for error paths

---

## Phase 10B: Pedantic Lint Compliance

### Enable Pedantic Lints

**Add to Cargo.toml or rust-toolchain:**
```toml
[lints.clippy]
all = "warn"
pedantic = "warn"
nursery = "warn"
# Disable specific pedantic lints that conflict with PostgreSQL FFI
missing_errors_doc = "allow"  # FFI functions often can't document all Postgres errors
module_name_repetitions = "allow"  # pg_tviews_ prefix is intentional
```

### Common Pedantic Issues

**1. Missing `#[must_use]`:**
```rust
// BEFORE:
pub fn load_cached() -> TViewResult<EntityDepGraph> { ... }

// AFTER:
#[must_use]
pub fn load_cached() -> TViewResult<EntityDepGraph> { ... }
```

**2. Inefficient cloning:**
```rust
// BEFORE:
fn process(&self, entity: String) -> TViewResult<()> {
    let e = entity.clone();  // Unnecessary clone
}

// AFTER:
fn process(&self, entity: &str) -> TViewResult<()> {
    let e = entity;  // Borrow instead
}
```

**3. Public items without docs:**
```rust
// BEFORE:
pub fn max_propagation_depth() -> usize { 100 }

// AFTER:
/// Maximum depth for propagation iteration
///
/// Prevents infinite loops in circular dependencies.
/// Returns the configured limit (default: 100).
///
/// # Examples
/// ```
/// let max = max_propagation_depth();
/// assert_eq!(max, 100);
/// ```
pub fn max_propagation_depth() -> usize { 100 }
```

**4. Wildcard imports:**
```rust
// BEFORE:
use pgrx::prelude::*;

// AFTER:
use pgrx::{pg_extern, pg_guard, Spi, JsonB, PgOid};
```

**5. Implicit return:**
```rust
// BEFORE (pedantic prefers explicit return):
fn get_depth() -> usize {
    100
}

// AFTER:
fn get_depth() -> usize {
    return 100;
}
```

### Implementation Checklist

- [ ] Enable pedantic lints in CI
- [ ] Fix all pedantic warnings (expect 50-100 warnings)
- [ ] Add `#[must_use]` to Result-returning functions
- [ ] Add `#[inline]` to hot path functions
- [ ] Replace wildcard imports with explicit imports
- [ ] Add missing documentation
- [ ] Remove unnecessary clones
- [ ] Add examples to public API functions

---

## Phase 10C: Error Handling Audit

### Error Handling Principles

**1. Never panic in FFI callbacks:**
```rust
#[pg_guard]
unsafe extern "C" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    // Use catch_unwind to prevent panics from crossing FFI boundary
    let result = std::panic::catch_unwind(|| {
        match event {
            // ... handle events
        }
    });

    if result.is_err() {
        error!("PANIC in transaction callback - this is a bug!");
    }
}
```

**2. Consistent error conversion:**
```rust
// Create From implementations for common error types
impl From<serde_json::Error> for TViewError {
    fn from(e: serde_json::Error) -> Self {
        TViewError::SerializationError {
            message: e.to_string(),
        }
    }
}

// Then use ? operator instead of unwrap
let value = serde_json::to_value(data)?;  // Clean!
```

**3. Error context:**
```rust
// BEFORE: Generic error
Spi::run(query)?;

// AFTER: Contextual error
Spi::run(query)
    .map_err(|e| TViewError::SpiError {
        query: query.to_string(),
        error: format!("Failed to execute DDL: {}", e),
    })?;
```

### TViewError Variants Audit

**Add missing error types:**
```rust
pub enum TViewError {
    // Existing variants...

    // NEW: Add these for better error handling
    /// Configuration error (invalid GUC values)
    ConfigError {
        setting: String,
        value: String,
        reason: String,
    },

    /// Cache error (poisoned mutex, corruption)
    CacheError {
        cache_name: String,
        reason: String,
    },

    /// FFI callback error (panic in C context)
    CallbackError {
        callback_name: String,
        error: String,
    },

    /// Metrics error (tracking failure)
    MetricsError {
        operation: String,
        error: String,
    },
}
```

### Implementation Checklist

- [ ] Audit all error handling patterns
- [ ] Add TViewError variants for new error cases
- [ ] Implement From<> for common error types
- [ ] Add context to all Spi::run() calls
- [ ] Wrap FFI callbacks in catch_unwind
- [ ] Add error path tests for all TViewError variants
- [ ] Document error recovery strategies
- [ ] Create error handling guidelines doc

---

## Phase 10D: Documentation Enhancement

### Documentation Standards

**1. Module-level docs:**
```rust
//! # Queue Management Module
//!
//! This module handles the transaction-level refresh queue for TVIEWs.
//!
//! ## Architecture
//!
//! The queue uses thread-local storage to maintain transaction isolation...
//!
//! ## Examples
//!
//! ```rust
//! use pg_tviews::queue::enqueue_refresh;
//! enqueue_refresh("user", 1)?;
//! ```

pub mod queue;
```

**2. Public function docs:**
```rust
/// Enqueue a TVIEW refresh for end-of-transaction processing
///
/// This adds a refresh request to the thread-local queue. The actual
/// refresh will be performed during the PRE_COMMIT transaction callback.
///
/// # Arguments
///
/// * `entity` - Entity name (e.g., "user" for tv_user)
/// * `pk` - Primary key value of the changed row
///
/// # Errors
///
/// Returns [`TViewError::QueueError`] if the queue is in an invalid state.
///
/// # Examples
///
/// ```
/// use pg_tviews::queue::enqueue_refresh;
///
/// // Enqueue refresh for user with pk=1
/// enqueue_refresh("user", 1)?;
/// ```
///
/// # Safety
///
/// This function is safe to call from triggers and transaction callbacks.
/// The queue is automatically flushed and cleared at transaction boundaries.
pub fn enqueue_refresh(entity: &str, pk: i64) -> TViewResult<()> {
    // ...
}
```

**3. Complex algorithm docs:**
```rust
/// Process the refresh queue using topological sort
///
/// # Algorithm
///
/// 1. Load dependency graph from pg_tview_meta
/// 2. Topologically sort pending keys by dependency order
/// 3. Refresh each TVIEW in sorted order
/// 4. Collect new parent keys from dependency graph
/// 5. Repeat until no new keys enqueued (fixed-point)
///
/// # Performance
///
/// - Graph load: O(N) where N = number of TVIEWs
/// - Sorting: O(K log K) where K = queue size
/// - Refresh: O(K × R) where R = avg refresh cost
///
/// # Panics
///
/// This function does not panic. All errors are returned as TViewResult.
fn handle_pre_commit() -> TViewResult<()> {
    // ...
}
```

### Documentation Coverage Goal

**Target: 90%+ coverage**
```bash
# Check current coverage
cargo doc --no-deps 2>&1 | grep "warning:"
```

**Documentation checklist:**
- [ ] All pub modules have module-level docs
- [ ] All pub functions have doc comments
- [ ] All pub structs have doc comments
- [ ] All pub enums have variant docs
- [ ] Examples for all public API functions
- [ ] Performance characteristics documented
- [ ] Safety invariants documented
- [ ] Error conditions documented

---

## Phase 10E: CI/CD Lint Integration

### GitHub Actions Configuration

**Add strict clippy check:**
```yaml
# .github/workflows/clippy.yml
name: Clippy Strict

on: [push, pull_request]

jobs:
  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          components: clippy
      - name: Run strict clippy
        run: |
          cargo clippy --all-targets --all-features -- \
            -D warnings \
            -D clippy::all \
            -D clippy::pedantic \
            -W clippy::nursery \
            -W clippy::cargo \
            -A clippy::missing_errors_doc \
            -A clippy::module_name_repetitions
```

### Pre-commit Hooks

**Install git hooks:**
```bash
# .git/hooks/pre-commit
#!/bin/bash
echo "Running clippy checks..."
cargo clippy --all-targets -- -D warnings -D clippy::pedantic || exit 1

echo "Running tests..."
cargo test --release || exit 1

echo "All checks passed!"
```

### Documentation CI

```yaml
# .github/workflows/docs.yml
name: Documentation

on: [push, pull_request]

jobs:
  docs:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Build docs
        run: cargo doc --no-deps --all-features
      - name: Check for warnings
        run: cargo doc --no-deps 2>&1 | grep -q "warning:" && exit 1 || exit 0
```

### Implementation Checklist

- [ ] Create clippy CI workflow
- [ ] Create documentation CI workflow
- [ ] Add pre-commit hooks
- [ ] Configure allowed/denied lints in Cargo.toml
- [ ] Add clippy configuration file (.clippy.toml)
- [ ] Document CI/CD setup in README
- [ ] Add status badges to README

---

## Files to Create/Modify

### Phase 10A: Unwrap Elimination
- `src/error.rs` - Add new error variants
- `src/queue/*.rs` - Replace unwraps in queue management
- `src/catalog.rs` - Replace unwraps in catalog queries
- `src/metrics.rs` - Replace unwraps in metrics tracking

### Phase 10B: Pedantic Compliance
- All `src/**/*.rs` - Add missing docs, fix pedantic warnings
- `Cargo.toml` - Add lint configuration
- `.clippy.toml` - Clippy configuration file

### Phase 10C: Error Handling
- `src/error.rs` - Add From<> implementations
- `src/queue/xact.rs` - Add catch_unwind to FFI callbacks
- `src/lib.rs` - Add catch_unwind to FFI functions

### Phase 10D: Documentation
- `src/lib.rs` - Add crate-level docs
- All modules - Add module-level docs
- `CONTRIBUTING.md` - Add documentation guidelines

### Phase 10E: CI/CD
- `.github/workflows/clippy.yml` - New: Strict clippy CI
- `.github/workflows/docs.yml` - New: Documentation CI
- `.git/hooks/pre-commit` - New: Pre-commit hooks
- `.clippy.toml` - New: Clippy configuration

---

## Testing Strategy

### Unwrap Elimination Tests

```rust
#[test]
fn test_serialization_error_handling() {
    // Test that serialization errors are properly handled
    let invalid_data = /* some invalid data */;
    let result = SerializedQueue::to_jsonb(invalid_data);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), TViewError::SerializationError { .. }));
}

#[test]
fn test_cache_corruption_handling() {
    // Test that cache errors don't panic
    // Simulate poisoned mutex
    let result = graph_cache::load_cached();
    // Should return error, not panic
    assert!(result.is_ok() || result.is_err());
}
```

### FFI Callback Safety Tests

```rust
#[pg_test]
fn test_callback_panic_safety() {
    // Verify that panics in callbacks don't corrupt database
    // This is hard to test directly, but we can verify error logging
}
```

### Documentation Tests

```rust
// All doc examples should compile and run
/// ```
/// use pg_tviews::enqueue_refresh;
/// enqueue_refresh("user", 1)?;
/// # Ok::<(), pg_tviews::TViewError>(())
/// ```
```

---

## Acceptance Criteria

### Phase 10A: Unwrap Elimination
- ✅ Zero unwrap() calls outside of acceptable cases (mutex locks)
- ✅ All acceptable unwraps use expect() with justification
- ✅ All error paths tested
- ✅ No panics in FFI callbacks

### Phase 10B: Pedantic Compliance
- ✅ `cargo clippy -- -D clippy::pedantic` passes with 0 warnings
- ✅ All public items documented
- ✅ No wildcard imports in library code
- ✅ All #[must_use] annotations added

### Phase 10C: Error Handling
- ✅ Consistent error patterns throughout codebase
- ✅ All Spi operations have error context
- ✅ FFI callbacks wrapped in catch_unwind
- ✅ From<> implementations for common error types

### Phase 10D: Documentation
- ✅ 90%+ documentation coverage
- ✅ All public API functions have examples
- ✅ Module-level docs for all modules
- ✅ `cargo doc` produces 0 warnings

### Phase 10E: CI/CD
- ✅ Clippy CI workflow passing
- ✅ Documentation CI workflow passing
- ✅ Pre-commit hooks installed
- ✅ README badges showing CI status

---

## Performance Targets

**No performance regression:**
- Phase 10 focuses on code quality, not performance
- All optimizations from Phase 6-7 must be preserved
- Benchmark after each sub-phase to verify no slowdown

**Potential improvements:**
- Better inlining from clippy suggestions: 1-5% faster
- Reduced allocations from clone elimination: 2-10% less memory

---

## Known Trade-offs

### Verbosity vs Safety

**More verbose code:**
```rust
// Before (concise but can panic):
let cache = CACHE.lock().unwrap();

// After (verbose but safe):
let cache = CACHE.lock()
    .expect("CACHE mutex poisoned - fatal error");
```

**Decision**: Safety > Conciseness for database extensions

### Documentation Overhead

- More docs = more maintenance burden
- Must keep docs in sync with code
- Use `#[doc(hidden)]` for internal functions

### CI Time Increase

- Strict clippy adds ~30-60 seconds to CI
- Documentation checks add ~15-30 seconds
- Worth it for catching bugs early

---

## Migration Strategy

### Incremental Approach

**Do NOT fix everything at once:**

1. **Week 1: Phase 10A** - Fix hot path unwraps
   - Queue management unwraps
   - FFI callback unwraps
   - Serialization unwraps

2. **Week 1-2: Phase 10B** - Enable pedantic gradually
   - Fix one category at a time (docs, then clones, then imports)
   - Commit after each category

3. **Week 2: Phase 10C** - Error handling audit
   - Add new error variants
   - Implement From<> conversions
   - Add catch_unwind wrappers

4. **Week 2: Phase 10D** - Documentation sprint
   - Document one module per day
   - Review and polish

5. **Week 2: Phase 10E** - CI/CD integration
   - Add workflows
   - Verify all checks pass
   - Add badges

---

## Success Metrics

### Code Quality Metrics

- **Clippy warnings**: 0 (with pedantic + nursery)
- **Documentation coverage**: 90%+
- **Unwrap count**: <10 (all justified with expect)
- **Panic count**: 0 (except tests and unreachable branches)
- **TODO count**: 0 (all converted to GitHub issues)

### Maintainability Metrics

- **CI pass rate**: 100% (all PRs must pass strict clippy)
- **Doc examples working**: 100% (all examples compile and run)
- **Error test coverage**: 80%+ (all error paths tested)

---

## Read Next

After Phase 10 completion, codebase is ready for:
- **Phase 8**: Two-phase commit support (high quality foundation)
- **Phase 9**: Production hardening (clean slate for new features)
- **Production release**: v1.0.0 with confidence in code quality

---

**Status Legend:**
- 📋 **PLANNED**: Detailed plan complete, ready for implementation
- 🚧 **IN PROGRESS**: Currently being worked on
- ✅ **COMPLETE**: Implemented, tested, and documented

**Last Updated:** 2025-12-10
**Estimated Completion:** 2-3 weeks post-Phase 7
