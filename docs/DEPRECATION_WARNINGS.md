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
  ├─ ANNOUNCE: Deprecate pg_tviews_legacy_func()
  ├─ New alternative: pg_tviews_new_func()
  └─ Guide: docs/migration/0.2-upgrade.md

v0.3.0 (Released Oct 2025)
  ├─ WARN: legacy_func() shows deprecation warning
  └─ Still works, but warns users

v1.0.0 (Released Apr 2026) [NEW MAJOR VERSION]
  ├─ REMOVED: legacy_func() no longer exists
  └─ Users MUST migrate by this date
```

Timeline: Aug 2025 → Apr 2026 = 8 months notice
Policy minimum: 6 months

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