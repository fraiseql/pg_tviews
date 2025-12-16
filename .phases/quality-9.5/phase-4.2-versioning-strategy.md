# Phase 4.2: Semantic Versioning & Deprecation Strategy

**Objective**: Define and implement semantic versioning (semver) policy with clear deprecation procedures and release guidelines

**Priority**: HIGH
**Estimated Time**: 2-3 days
**Blockers**: Phase 4.1 complete (API audit)

---

## Context

**Current State**: Using 0.1.0-beta.1, but no formal versioning policy documented

```
Package Manifest (Cargo.toml):
version = "0.1.0-beta.1"

Release History:
├── v0.1.0-beta.1 (current)
└── (No prior releases)

No documented:
- When to bump major/minor/patch
- Deprecation procedures
- Prerelease/RC conventions
- Security patch handling
```

**Why This Matters**:
- Users need predictable version contracts
- 1.0 release signals production stability
- API stability depends on clear versioning
- Security updates must be deployed quickly
- Users can make informed upgrade decisions

**Deliverable**: Comprehensive versioning strategy with automated checks and release procedures

---

## Semantic Versioning Policy

### Format: MAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]

**Example**: `1.2.3-rc.1+build.123`

### Version Components

**MAJOR** (breaking changes):
- Increment: 0 → 1 (1.0 release), 1 → 2 (major update)
- When: Incompatible API changes, data format changes, PostgreSQL version drop
- Deprecation notice: 12+ months before removal
- Release cadence: Every 1-2 years

**MINOR** (new features, backward compatible):
- Increment: 1.0 → 1.1 (new features), 1.1 → 1.2 (enhancements)
- When: New functionality, performance improvements, new optimization options
- Breaking changes: None (STABLE APIs only)
- Release cadence: Every 2-4 months

**PATCH** (bug fixes, backward compatible):
- Increment: 1.2.0 → 1.2.1 (bug fix), 1.2.1 → 1.2.2 (more fixes)
- When: Bug fixes, security patches, internal refactoring
- Breaking changes: None
- Release cadence: As needed (immediately for security)

**PRERELEASE** (alpha, beta, release candidate):
- Format: `1.0.0-alpha.1`, `1.0.0-beta.2`, `1.0.0-rc.1`
- When: Testing new major/minor versions before release
- Policy: Follows same semver rules, but no compatibility guarantee
- Testing: Full regression test before promotion

**BUILD** (metadata only):
- Format: `1.0.0+git.abc1234` or `1.0.0+date.20251213`
- When: Automated builds, CI/CD info
- Sorting: Ignored in version comparisons
- Example: `0.1.0-beta.1+build.42` sorts same as `0.1.0-beta.1+build.100`

### Decision Matrix

| Change Type | Major | Minor | Patch |
|-------------|-------|-------|-------|
| Add STABLE API | ✅ | ✅ | ❌ |
| Change STABLE API | ✅ | ❌ | ❌ |
| Add EVOLVING API | | ✅ | ✅ |
| Bug fix | | | ✅ |
| Performance optimization | | ✅ | ✅ |
| Security vulnerability | | | ✅ |
| Drop PostgreSQL version | ✅ | ❌ | ❌ |
| Refactor internal code | | | ✅ |

---

## Release Lifecycle

### Phase 1: Development (Continuous)

**Branch**: `main` or `develop`

```
v0.1.0-beta.1
    ↓
Commit commits (feature branches)
    ↓
Main branch
    ↓
CI/CD runs tests
```

**Version format**: `X.Y.Z-dev` or commit-based
**Release frequency**: Continuous integration

### Phase 2: Pre-release (2 weeks before release)

**Branch**: `release/v1.2.0`

1. **Code Freeze**: No new features, bug fixes only
2. **Release Notes**: Draft CHANGELOG.md
3. **Testing**: Full regression test suite
4. **Version Bump**: Set to `1.2.0-rc.1`

```bash
# Cut release branch
git checkout -b release/v1.2.0 main

# Update version
cargo metadata --format-version 1 | jq '.packages[0].version'
# Update Cargo.toml: 1.1.0 → 1.2.0-rc.1

# Commit
git commit -am "chore: Prepare v1.2.0-rc.1 release"
git tag v1.2.0-rc.1
git push origin release/v1.2.0 --tags
```

4. **RC Progression**: RC.1 → RC.2 → RC.N as bugs found/fixed

### Phase 3: Release (1 day before)

1. **Final Testing**: Run full integration suite
2. **Release Notes**: Finalize CHANGELOG.md
3. **Version Update**: Remove `-rc.X`, set to `1.2.0`

```bash
# In release branch
cargo metadata --format-version 1 | jq '.packages[0].version'
# Update Cargo.toml: 1.2.0-rc.5 → 1.2.0

git commit -am "chore: Release v1.2.0"
git tag v1.2.0
git push origin release/v1.2.0 --tags

# Merge back to main
git checkout main
git merge --no-ff release/v1.2.0
git push origin main

# Clean up
git branch -d release/v1.2.0
git push origin --delete release/v1.2.0
```

4. **Release Artifacts**: Build and publish binaries

### Phase 4: Post-release (Continuous)

**Branch**: `main` (development continues)

```
v1.2.0 (tag)
    ↓
Main branch development
    ↓
Bugfix commits for v1.2.1
    ↓
v1.2.1 release (patch)
```

---

## Implementation Steps

### Step 1: Create Versioning Policy Document

**File**: `docs/VERSIONING.md`

```markdown
# Semantic Versioning Policy for pg_tviews

## Overview

pg_tviews follows [Semantic Versioning 2.0.0](https://semver.org/).

Version format: `MAJOR.MINOR.PATCH[-PRERELEASE]`

## Stability Guarantees by Version

### v0.1.x (Current Beta)
- No API stability guarantee
- STABLE APIs committed within this line
- Ready for limited production use
- Breaking changes documented in CHANGELOG

### v0.2-0.9.x (Pre-release Stabilization)
- Approaches production readiness
- STABLE APIs solidify
- Fewer breaking changes
- Explicit migration guides for breaking changes

### v1.0.x and Later (Production Stable)
- Long-term API stability
- STABLE APIs guaranteed compatible
- Breaking changes only in major versions
- 12+ month deprecation notice required

## Backward Compatibility Guarantee

### Forward Guaranteed (Upgrade Safe)

**Always safe to upgrade** to newer PATCH versions:
```
0.1.0 → 0.1.1 (safe, bug fixes only)
0.1.0 → 0.1.2 (safe, bug fixes only)
0.1.0 → 0.1.N (safe)

1.0.0 → 1.0.1 (safe)
1.0.0 → 1.1.0 (safe if code uses STABLE APIs)
1.0.0 → 1.5.0 (safe if code uses STABLE APIs)
```

**Usually safe to upgrade** to newer MINOR versions:
```
1.0.0 → 1.1.0 (safe if using STABLE APIs only)
1.0.0 → 1.2.0 (safe if using STABLE APIs only)
```

**May need code changes** for MAJOR versions:
```
0.1.0 → 1.0.0 (check breaking changes in CHANGELOG)
1.0.0 → 2.0.0 (check breaking changes in CHANGELOG)
```

### Backward NOT Guaranteed

**Downgrade not supported**:
```
0.1.1 → 0.1.0 (data corruption risk)
1.0.0 → 0.9.0 (compatibility loss)
```

## Breaking Changes Policy

### What Constitutes a Breaking Change?

❌ **Breaking**:
- Removing a STABLE function/type
- Changing STABLE function signature
- Changing STABLE return value type
- Changing error codes for STABLE functions
- Dropping PostgreSQL version support

✅ **Not Breaking**:
- Adding new optional parameters
- Adding new EVOLVING functions
- Improving performance
- Changing EXPERIMENTAL functions
- Improving error messages

### Breaking Change Procedure

**Step 1**: Announce deprecation (current version)
```
Release Notes: "pg_tviews_legacy_func() deprecated, use pg_tviews_new_func() instead"
```

**Step 2**: Add deprecation warnings (next version)
```sql
-- v1.1.0
SELECT pg_tviews_legacy_func();
-- WARNING: pg_tviews_legacy_func() is deprecated
-- See: docs/migration/v1.1-upgrade-guide.md
```

**Step 3**: Remove in next major version (major version only)
```
v2.0.0: pg_tviews_legacy_func() removed
```

### Timeline Requirement

- Announced: Version N
- Warning: Version N+1 (minimum)
- Removal: Version N+2 major (minimum 6 months later)

**Example**:
```
v0.2.0 (Aug 2025): Deprecate pg_tviews_legacy_func()
v0.3.0 (Oct 2025): Add deprecation warning
v1.0.0 (Apr 2026): Safe to remove (6+ months)
```

## Security Patch Policy

Security vulnerabilities get **immediate patch release**:

```
v1.0.5 (released)
    ↓
CVE found
    ↓
v1.0.6 (security patch, same day if possible)
```

- Applied to **all supported versions**
- Released outside normal cadence
- Announced prominently
- No new features in security patches

### Supported Versions

| Version | Release Date | End of Life |
|---------|--------------|------------|
| 2.0.x | Future | TBD |
| 1.5.x | Future | TBD |
| 1.4.x | Future | 1 year after 1.5 release |
| 1.3.x | Future | 1 year after 1.4 release |
| 1.0.x | Apr 2026 | 2 years (Apr 2028) |
| 0.9.x | TBD | 6 months after 1.0 release |
| 0.1.x | Current | 6 months after 1.0 release |

## PostgreSQL Version Support by pg_tviews Version

| pg_tviews | pg13 | pg14 | pg15 | pg16 | pg17 | pg18 |
|-----------|------|------|------|------|------|------|
| 0.1.x | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 1.0.x | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 1.5.x | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 2.0.x | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ |

- **✅**: Fully supported and tested
- **⚠️**: May work but not tested
- **❌**: Not supported, upgrade required

## Version Checking in Code

### SQL
```sql
-- Check version
SELECT pg_tviews_version();
-- Result: "0.1.0-beta.1"

-- Programmatic check
SELECT current_setting('pg_tviews.version')::semver >= '0.2.0'::semver;
```

### Rust
```rust
const VERSION: &str = env!("CARGO_PKG_VERSION");

if version_greater_than(VERSION, "1.0.0") {
    // Use v1.0+ APIs
}
```

## Release Checklist

- [ ] All tests pass on target PostgreSQL versions
- [ ] CHANGELOG.md updated with all changes
- [ ] Version updated in Cargo.toml
- [ ] Git tag created: `v1.2.3`
- [ ] Release notes published
- [ ] Binaries built and tested
- [ ] Documentation updated
- [ ] Breaking changes documented
- [ ] Migration guide published (if needed)
- [ ] Announce on community channels
```

### Step 2: Create Version Bump Automation

**File**: `scripts/bump-version.sh`

```bash
#!/bin/bash
set -euo pipefail

# Semantic version bump script for pg_tviews
# Usage: ./bump-version.sh major|minor|patch|prerelease [--dry-run]

if [ $# -lt 1 ]; then
    echo "Usage: $0 major|minor|patch|prerelease|release [--dry-run]"
    echo ""
    echo "Examples:"
    echo "  $0 minor          # 0.1.0 → 0.2.0"
    echo "  $0 patch          # 0.1.0 → 0.1.1"
    echo "  $0 prerelease     # 0.2.0 → 0.2.0-rc.1"
    echo "  $0 release        # 0.2.0-rc.1 → 0.2.0"
    exit 1
fi

BUMP_TYPE=$1
DRY_RUN=${2:-}

# Parse current version
CURRENT_VERSION=$(grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/')
echo "Current version: $CURRENT_VERSION"

# Function to compare versions
semver_bump() {
    local version=$1
    local bump_type=$2

    # Remove prerelease suffix
    base_version=$(echo "$version" | sed 's/-.*$//')

    # Parse components
    major=$(echo "$base_version" | cut -d. -f1)
    minor=$(echo "$base_version" | cut -d. -f2)
    patch=$(echo "$base_version" | cut -d. -f3)

    case "$bump_type" in
        major)
            echo "$((major + 1)).0.0"
            ;;
        minor)
            echo "$major.$((minor + 1)).0"
            ;;
        patch)
            echo "$major.$minor.$((patch + 1))"
            ;;
        prerelease)
            echo "$major.$minor.$patch-rc.1"
            ;;
        release)
            # Remove prerelease suffix
            echo "$base_version"
            ;;
        *)
            echo "Unknown bump type: $bump_type" >&2
            exit 1
            ;;
    esac
}

NEW_VERSION=$(semver_bump "$CURRENT_VERSION" "$BUMP_TYPE")
echo "New version: $NEW_VERSION"

if [ -z "$DRY_RUN" ]; then
    # Update Cargo.toml
    sed -i.bak "s/^version = .*/version = \"$NEW_VERSION\"/" Cargo.toml
    rm Cargo.toml.bak

    # Update lock file
    cargo update --offline || true

    # Create commit
    git add Cargo.toml Cargo.lock
    git commit -m "chore: Bump version to $NEW_VERSION"
    git tag "v$NEW_VERSION"

    echo "✅ Version bumped to $NEW_VERSION"
    echo "Next: Push with: git push origin main --tags"
else
    echo "🔍 Dry run (no changes made)"
fi
```

### Step 3: Add Version Enforcement in CI

**File**: `.github/workflows/version-check.yml`

```yaml
name: Version Check

on:
  pull_request:
    paths:
      - 'Cargo.toml'
      - 'CHANGELOG.md'

jobs:
  version-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Check semver format
        run: |
          VERSION=$(grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/')
          echo "Checking version: $VERSION"

          # Regex for semver: X.Y.Z[-prerelease][+build]
          if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$'; then
              echo "❌ Version $VERSION doesn't match semantic versioning format"
              exit 1
          fi

          echo "✅ Version format valid"

      - name: Check CHANGELOG updated
        run: |
          # For non-dev versions, CHANGELOG should be updated
          VERSION=$(grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/')

          if [[ ! "$VERSION" =~ dev$ ]]; then
              if ! grep -q "## \[$VERSION\]" CHANGELOG.md; then
                  echo "❌ CHANGELOG.md not updated for version $VERSION"
                  exit 1
              fi
          fi

          echo "✅ CHANGELOG.md updated"

      - name: Verify no duplicate versions in CHANGELOG
        run: |
          # Check for duplicate version headers
          if [ "$(grep -c '^## \[' CHANGELOG.md)" != \
               "$(grep '^## \[' CHANGELOG.md | sort -u | wc -l)" ]; then
              echo "❌ CHANGELOG.md has duplicate version headers"
              exit 1
          fi

          echo "✅ No duplicate versions in CHANGELOG"
```

### Step 4: Create CHANGELOG Template

**File**: `CHANGELOG.md` (initialize with template)

```markdown
# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- (To be filled in during next release)

### Changed
- (To be filled in during next release)

### Deprecated
- (To be filled in during next release)

### Removed
- (To be filled in during next release)

### Fixed
- (To be filled in during next release)

### Security
- (To be filled in during next release)

---

## [0.1.0-beta.1] - 2025-12-13

### Added
- Initial public API for TVIEW conversion
- Incremental refresh with transaction queue
- Dependency graph analysis
- PostgreSQL 13-18 support
- Basic TVIEW metadata introspection

### Known Limitations
- Queue debugging functions experimental
- Performance claims not yet validated
- Limited error path testing
- No deprecation policy (beta period)

---

## Unreleased Changes (Development Only)

Track ongoing work in GitHub Issues and PRs.
```

### Step 5: Document Deprecation Warning System

**File**: `docs/DEPRECATION_WARNINGS.md`

```markdown
# Deprecation Warning System

## For SQL Users

### Example: Deprecated Function

```sql
-- Function deprecated in 0.2.0
-- Will be removed in 1.0.0 (Apr 2026)
SELECT pg_tviews_legacy_function();

-- Output includes warning:
-- WARNING: pg_tviews_legacy_function() is deprecated
-- Use: pg_tviews_new_function() instead
-- See: docs/migration/0.2-upgrade-guide.md
```

### Checking for Deprecations

```sql
-- Query deprecation status of all functions
SELECT pg_describe_object(classid, objid, objsubid) as function,
       obj_description(objid, 'pg_proc') as description
FROM pg_depend
WHERE deptype = 'n'  -- Dependency on normal object
  AND classid = 'pg_proc'::regclass
ORDER BY function;
```

## For Rust Users

### Example: Deprecated Struct

```rust
#[deprecated(
    since = "0.2.0",
    note = "Use `ViewRow` instead. See docs/migration/0.2-upgrade-guide.md"
)]
pub struct LegacyViewRow { /* ... */ }

// When used:
// warning: use of deprecated struct `LegacyViewRow`
//   --> src/main.rs:10:5
//    |
// 10 |     let row = LegacyViewRow::new();
//    |         ^^^
//    |
//    = note: Use `ViewRow` instead...
```

### Suppressing Deprecation Warnings (Temporary)

```rust
#[allow(deprecated)]
fn legacy_code() {
    let row = LegacyViewRow::new();  // No warning
}
```

## Timeline for Deprecation

### Phase 1: Announce (Released)
- Deprecation noted in release notes
- Documentation updated with alternative
- Migration guide published (if complex)

### Phase 2: Warn (Next version)
- Deprecation warning added to code
- Warning appears when function/type used
- Still fully functional

### Phase 3: Remove (Major version only)
- Function/type removed completely
- Listed in breaking changes
- Migration guide required

### Example Timeline

```
v0.2.0 (Released Aug 2025)
  ├─ ANNOUNCE: "legacy_func() deprecated"
  ├─ New alternative: "new_func()"
  └─ Guide: docs/migration/0.2-upgrade.md

v0.3.0 (Released Oct 2025)
  ├─ WARN: legacy_func() shows deprecation warning
  └─ Still works, but warns users

v1.0.0 (Released Apr 2026) [NEW MAJOR VERSION]
  ├─ REMOVED: legacy_func() no longer exists
  └─ Users MUST migrate by this date

Timeline: Aug 2025 → Apr 2026 = 8 months notice
Policy minimum: 6 months
```

## How to Report Deprecations

When deprecating a feature:

1. **Add to code**:
```rust
#[deprecated(
    since = "0.2.0",
    note = "Use alternative. See [migration guide](docs/url)"
)]
```

2. **Update CHANGELOG.md**:
```
## [0.2.0]
### Deprecated
- `legacy_function()` in favor of `new_function()`
```

3. **Create migration guide**:
- File: `docs/migration/0.2-upgrade-guide.md`
- Include before/after examples
- Common gotchas
- Troubleshooting

4. **Announce in release notes**:
- Prominent section: "Deprecations & Migrations"
- Timeline: When will be removed
- Link to migration guide
```

### Step 6: Create Release Process Document

**File**: `docs/RELEASE_PROCESS.md`

```markdown
# Release Process for pg_tviews

## Pre-Release Checklist (2 weeks before)

### Code Preparation
- [ ] All features merged to main
- [ ] All tests passing locally
- [ ] Code review completed
- [ ] No open blockers

### Documentation
- [ ] README.md updated for new features
- [ ] API documentation up to date
- [ ] Migration guides created (if breaking changes)
- [ ] CHANGELOG.md drafted

### Quality Gates
- [ ] All tests pass: `cargo pgrx test --all`
- [ ] No clippy warnings: `cargo clippy --all-targets -- -D warnings`
- [ ] Documentation builds: `cargo doc --no-deps`
- [ ] Version bumped in Cargo.toml

### Release Candidate Steps

1. Create release branch:
```bash
git checkout -b release/v1.2.0 main
```

2. Update version:
```bash
./scripts/bump-version.sh minor
# Changes 1.1.0 → 1.2.0-rc.1
```

3. Update CHANGELOG.md with version header:
```
## [1.2.0-rc.1] - YYYY-MM-DD

### Added
...
```

4. Commit and tag:
```bash
git commit -am "chore: Prepare v1.2.0-rc.1"
git tag v1.2.0-rc.1
git push origin release/v1.2.0 --tags
```

5. Run full test suite on target versions:
```bash
cargo pgrx test --all
```

6. Address any issues and create RC.2, RC.3, etc. as needed

## Release Day (When RC is Stable)

### Final Steps

1. Update version to final (remove -rc suffix):
```bash
./scripts/bump-version.sh release
# Changes 1.2.0-rc.5 → 1.2.0
```

2. Update CHANGELOG.md:
```
## [1.2.0] - 2025-12-13  # ← Set actual date
```

3. Create final commit:
```bash
git commit -am "chore: Release v1.2.0"
git tag v1.2.0
git push origin release/v1.2.0 --tags
```

4. Merge back to main:
```bash
git checkout main
git merge --no-ff release/v1.2.0
git push origin main
```

5. Create GitHub Release:
- Copy CHANGELOG.md section
- Add download links
- Mark as pre-release if RC
- Mark as latest release if final

6. Publish binaries and artifacts

### Post-Release

1. Delete release branch:
```bash
git branch -d release/v1.2.0
git push origin --delete release/v1.2.0
```

2. Update development version:
```bash
./scripts/bump-version.sh minor
# Start development for next version
git commit -am "chore: Start development for v1.3.0-dev"
```

3. Announce release:
- GitHub release page
- Community forums
- Social media
- Email newsletter (if applicable)

4. Monitor for issues:
- Watch bug reports
- Prepare patch releases if needed
```

---

## Verification Commands

```bash
# 1. Check current version
grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/'

# 2. Validate semver format
VERSION=$(grep "^version" Cargo.toml | sed 's/.*version = "\(.*\)".*/\1/')
echo "$VERSION" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?(\+[a-zA-Z0-9.]+)?$' && echo "✅ Valid"

# 3. Check CHANGELOG has matching version
grep "## \[$(cargo metadata --format-version 1 | jq -r '.packages[0].version')\]" CHANGELOG.md

# 4. Verify version matches across all files
grep -r "0\.1\.0-beta\.1" . --include="*.rs" --include="*.toml" --include="*.md"

# 5. Test release script
./scripts/bump-version.sh patch --dry-run

# 6. Simulate full release
git status  # Must be clean
cargo test --all
cargo clippy --all-targets -- -D warnings
```

---

## Acceptance Criteria

- [ ] VERSIONING.md policy created and comprehensive
- [ ] bump-version.sh script working (tested in dry-run)
- [ ] CHANGELOG.md initialized with current version
- [ ] Version check CI workflow added
- [ ] RELEASE_PROCESS.md with full checklists
- [ ] DEPRECATION_WARNINGS.md guidelines documented
- [ ] All version references updated consistently
- [ ] Backward compatibility guarantees documented
- [ ] Security patch policy defined
- [ ] PostgreSQL version support matrix defined

---

## DO NOT

- ❌ Bump major version for API enhancements (minor/patch only)
- ❌ Skip pre-release testing before release
- ❌ Make breaking changes in patch/minor versions
- ❌ Remove deprecated APIs without 6+ month notice
- ❌ Forget to update CHANGELOG.md for each release
- ❌ Use inconsistent version format across files

---

## Common Versioning Mistakes

**❌ WRONG**: Bumping patch for a new feature
- Patch = 1.2.3 → 1.2.4 (bug fixes only)
- Should be: 1.2.3 → 1.3.0 (new feature = minor)

**✅ RIGHT**: Bumping minor for a new feature
- 1.2.3 → 1.3.0 (new feature, backward compatible)

**❌ WRONG**: Breaking changes in minor version
- 1.0.0 → 1.1.0 but API changed
- Users expect 1.1.0 to be backward compatible
- Should be: 1.0.0 → 2.0.0 (breaking = major)

**✅ RIGHT**: Breaking changes only in major
- 1.5.0 → 2.0.0 (breaking changes allowed)
- Users know major version may require code changes

---

## Related Documentation

- [Semantic Versioning 2.0.0](https://semver.org/) - Official specification
- [Keep a Changelog](https://keepachangelog.com/) - CHANGELOG format
- [Phase 4.1: API Audit](./phase-4.1-api-audit.md) - API stability levels
- [Phase 4.3: Breaking Changes](./phase-4.3-breaking-changes.md) - Future breaking changes roadmap

---

## Next Steps

After completion:
- Commit with message: `docs(versioning): Add semantic versioning and deprecation strategy [PHASE4.2]`
- Review release process with maintainers
- Proceed to **Phase 4.3: Breaking Changes Roadmap for 2.0**
