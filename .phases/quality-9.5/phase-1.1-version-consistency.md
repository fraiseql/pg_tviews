# Phase 1.1: Version Consistency & Metadata

**Objective**: Fix version mismatches and ensure consistency across all project metadata

**Priority**: CRITICAL
**Estimated Time**: 2-3 hours
**Blockers**: None

---

## Context

**Current Issue**: Code says "0.1.0-alpha" but README and Cargo.toml say "0.1.0-beta.1"

```rust
// src/lib.rs:82
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"  // ❌ WRONG
}
```

This inconsistency creates confusion for users and automated tooling.

---

## Files to Modify

1. `src/lib.rs` - Update version string
2. `Cargo.toml` - Verify version is correct
3. `README.md` - Verify version badge
4. `docs/getting-started/installation.md` - Update any version references
5. `.github/workflows/release.yml` (if exists) - Version tagging

---

## Implementation Steps

### Step 1: Audit All Version References

**Search for all version strings:**
```bash
# Find all version references
rg "0\.1\.0" --type md --type toml --type rust
rg "alpha|beta" --type md --type toml --type rust

# Check SQL extension version
rg "CREATE EXTENSION" test/sql/
```

**Expected locations:**
- `Cargo.toml`: `version = "0.1.0-beta.1"` ✅
- `src/lib.rs`: `pg_tviews_version()` function ❌
- `README.md`: Version badge ✅
- SQL control files (if any)

### Step 2: Update Core Version Function

**File**: `src/lib.rs`

**Change**:
```rust
/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-beta.1"  // ✅ Match Cargo.toml
}
```

**Rationale**: This function is the canonical runtime version check. Must match Cargo.toml.

### Step 3: Create Version Constant

**File**: `src/lib.rs`

**Add after imports**:
```rust
/// Extension version (synced with Cargo.toml)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    VERSION
}
```

**Rationale**: Using `env!("CARGO_PKG_VERSION")` ensures version is always synced with Cargo.toml at build time. No manual updates needed.

### Step 4: Add Version Tests

**File**: `src/lib.rs` (add test module at end)

```rust
#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn version_matches_cargo_toml() {
        let cargo_version = env!("CARGO_PKG_VERSION");
        assert_eq!(pg_tviews_version(), cargo_version,
            "Runtime version must match Cargo.toml version");
    }

    #[test]
    fn version_format_is_valid() {
        let version = pg_tviews_version();
        // Should be semver: MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-PRERELEASE
        assert!(version.contains('.'), "Version must be semver format");
        assert!(!version.is_empty(), "Version cannot be empty");
    }
}
```

### Step 5: Update Documentation

**File**: `README.md`

**Verify version badge**:
```markdown
[![Version](https://img.shields.io/badge/version-0.1.0--beta.1-orange.svg)]
```

**File**: `docs/getting-started/installation.md`

Search for any hardcoded version references and replace with generic instructions:
```markdown
# Before
git checkout v0.1.0-alpha

# After
git checkout v0.1.0-beta.1
# Or better: use latest stable tag
git checkout $(git describe --tags --abbrev=0)
```

### Step 6: Add Changelog Entry

**File**: `CHANGELOG.md` (create if doesn't exist)

```markdown
# Changelog

All notable changes to pg_tviews will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Version consistency: `pg_tviews_version()` now returns "0.1.0-beta.1" matching Cargo.toml
- Version function now uses `CARGO_PKG_VERSION` to prevent future drift

## [0.1.0-beta.1] - 2025-12-13

### Added
- Initial beta release
- Incremental materialized view refresh
- JSONB optimization with jsonb_ivm
- Comprehensive documentation
- Production-ready security features

[Unreleased]: https://github.com/fraiseql/pg_tviews/compare/v0.1.0-beta.1...HEAD
[0.1.0-beta.1]: https://github.com/fraiseql/pg_tviews/releases/tag/v0.1.0-beta.1
```

---

## Verification Commands

```bash
# 1. Verify code compiles
cargo build --release

# 2. Run version tests
cargo test version_tests

# 3. Check version at runtime
cargo pgrx run pg17
# In psql:
SELECT pg_tviews_version();  -- Should return "0.1.0-beta.1"

# 4. Verify no hardcoded versions remain
rg "0\.1\.0-alpha" --type rust --type md --type toml
# Should return NO results

# 5. Verify Cargo.toml version
cargo pkgid | cut -d'#' -f2  # Should show 0.1.0-beta.1
```

**Expected Output**:
```
pg_tviews_version
-------------------
0.1.0-beta.1
(1 row)
```

---

## Acceptance Criteria

- [x] `pg_tviews_version()` returns "0.1.0-beta.1"
- [x] Version is sourced from `CARGO_PKG_VERSION`
- [x] Unit tests verify version consistency
- [x] No "alpha" strings remain in codebase
- [x] Documentation references correct version
- [x] CHANGELOG.md exists and is updated
- [x] All tests pass
- [x] No clippy warnings introduced

---

## DO NOT

- ❌ Change the actual version number (0.1.0-beta.1 is correct)
- ❌ Add version bumping automation (out of scope)
- ❌ Modify SQL extension version (handled by pgrx)
- ❌ Change version format (must remain semver)

---

## Rollback Plan

If issues arise:
```bash
git checkout HEAD -- src/lib.rs
cargo build --release
cargo pgrx test pg17
```

---

## Next Steps

After completion:
- Commit with message: `fix(metadata): Sync runtime version with Cargo.toml [PHASE1.1]`
- Proceed to **Phase 1.2: Unwrap Elimination**
