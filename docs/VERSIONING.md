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
Release Notes: "pg_tviews_<old>() deprecated, use pg_tviews_<new>() instead"
```

**Step 2**: Add deprecation warnings (next version)
```sql
-- v1.1.0
SELECT pg_tviews_<old>();
-- WARNING: pg_tviews_<old>() is deprecated
-- See: docs/migration/v1.1-upgrade-guide.md
```

**Step 3**: Remove in next major version (major version only)
```
v2.0.0: pg_tviews_<old>() removed
```

### Timeline Requirement

- Announced: Version N
- Warning: Version N+1 (minimum)
- Removal: Version N+2 major (minimum 6 months later)

**Example**:
```
v0.2.0 (Aug 2025): Deprecate pg_tviews_<old>()
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
| 1.0.x | Future | 2 years after its release |
| 0.9.x | TBD | 6 months after 1.0 release |
| 0.1.x | Current | 6 months after 1.0 release |

## PostgreSQL Version Support by pg_tviews Version

| pg_tviews | pg13–pg15 | pg16 | pg17 | pg18 |
|-----------|-----------|------|------|------|
| 0.1.x | ❌ | ✅ | ✅ | ✅ |

- **✅**: Supported, built and tested in CI (clippy, regression and integration suites)
- **❌**: Not supported: `CREATE EXTENSION pg_tviews` refuses to install

## Version Checking in Code

### SQL
```sql
-- Check version
SELECT pg_tviews_version();
-- Result: "0.1.0-beta.21"

-- Programmatic check (major.minor.patch, pre-release suffix ignored)
SELECT string_to_array(split_part(pg_tviews_version(), '-', 1), '.')::int[] >= ARRAY[0, 2, 0];
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