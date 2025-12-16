# Phase 4.1: Public API Audit & Stability Classification

**Objective**: Audit all public APIs, classify stability levels, and document contract guarantees for long-term compatibility

**Priority**: HIGH
**Estimated Time**: 2-3 days
**Blockers**: None (can run in parallel with Phase 2)

---

## Context

**Current State**: 40+ public functions exported, mostly undocumented stability levels

```
Exported Functions (via SQL):
├── pg_tviews_convert_existing_table() - Core function
├── pg_tviews_metadata() - Introspection
├── pg_tviews_get_queue() - Debugging
├── pg_tviews_clear_queue() - Maintenance
├── pg_tviews_refresh_all() - Batch refresh
├── pg_tviews_dependency_graph() - Analysis
├── pg_tviews_version() - Version info
└── ... 10+ more

Exported Rust Symbols (via lib.rs):
├── pub fn refresh_pk() - Core refresh logic
├── pub struct ViewRow - Data representation
├── pub enum TViewError - Error handling
├── pub struct DependencyGraph - Analysis
└── ... 20+ more types and functions
```

**Why This Matters**:
- Beta status (0.1.0-beta.1) means API can still change
- Users need to know which APIs are stable vs experimental
- Version 1.0 requires commitment to stability
- Breaking changes must be planned and documented
- Deprecation cycles need clear timelines

**Deliverable**: Public API audit with stability classifications and migration guidance

---

## API Classification System

### Stability Levels

**STABLE (Guaranteed Compatibility)**
- Will not change in minor versions (1.x)
- Breaking changes only in major versions (2.0+)
- Examples: Core TVIEW operations, basic refresh, dependency graph
- Commitment: 12+ month deprecation notice for breaking changes

**EVOLVING (Likely Changes)**
- May change in minor versions without notice
- Stabilize after 1-2 releases
- Examples: Performance tuning APIs, new features, optimization hints
- Commitment: 6+ month deprecation notice for breaking changes

**EXPERIMENTAL (No Compatibility Guarantee)**
- Can change or disappear in any release
- Not recommended for production code
- Examples: Debugging functions, internal introspection, caching control
- Commitment: None, but will document removal

**DEPRECATED (Scheduled Removal)**
- Will be removed in specified future version
- Replacement API recommended
- Warning messages on use
- Commitment: 1-2 release cycle before removal (minimum 6 months)

### Decision Matrix

| API Type | Usage | Maturity | Classification |
|----------|-------|----------|-----------------|
| Core TVIEW conversion | Production | Months in use | STABLE |
| Bulk refresh operations | Production | Well-tested | STABLE |
| Dependency introspection | Analytics | Widely used | STABLE |
| Performance tuning params | Advanced | Limited feedback | EVOLVING |
| Queue debugging functions | Operations | Changing needs | EXPERIMENTAL |
| Legacy feature X | Rare | Superseded | DEPRECATED |

---

## Implementation Steps

### Step 1: Audit SQL API Surface

**File**: `docs/api/SQL_FUNCTIONS.md`

```markdown
# pg_tviews SQL API Reference

## STABLE Functions

### pg_tviews_convert_existing_table(table_name TEXT)
**Status**: STABLE (v0.1+)
**Last Updated**: 2025-12-13
**Description**: Convert a regular table to a TVIEW with incremental refresh
**Parameters**:
- `table_name`: Schema-qualified table name (required)
**Returns**: void
**Errors**:
- `42704` - Table not found
- `42809` - Already a TVIEW
- `42701` - Invalid table structure

**Contract Guarantees**:
- Behavior unchanged except for performance optimizations
- Error codes maintained (unless documented as changed)
- All dependent views continue to work after upgrade
- May add optional parameters in minor versions

**Example**:
```sql
CREATE TABLE sales (
    id SERIAL PRIMARY KEY,
    amount DECIMAL(10,2)
);
SELECT pg_tviews_convert_existing_table('public.sales');
```

**Breaking Changes**: None planned through v1.x

---

### pg_tviews_metadata(tview_name TEXT)
**Status**: STABLE (v0.1+)
**Description**: Retrieve TVIEW metadata and configuration
**Returns**: TABLE(
    entity_name TEXT,
    primary_key TEXT,
    created_at TIMESTAMP,
    last_refreshed TIMESTAMP,
    rows_cached INT
)
**Contract**: Schema guaranteed stable (may add optional columns)

---

## EVOLVING Functions

### pg_tviews_get_queue()
**Status**: EVOLVING
**Description**: Inspect current refresh queue (debugging)
**Stability Target**: STABLE in v1.1

**Known Future Changes**:
- May restructure output columns for performance
- Add additional diagnostic fields
- Change refresh order/priority algorithm

**Migration Path**:
```sql
-- v0.1: Current implementation
SELECT * FROM pg_tviews_get_queue();

-- v1.0: Potentially different schema
SELECT queue_id, entity, priority FROM pg_tviews_refresh_queue();
```

---

## EXPERIMENTAL Functions

### pg_tviews_clear_queue()
**Status**: EXPERIMENTAL
**Description**: Force-clear refresh queue (advanced debugging only)
**Warning**: Can cause data inconsistency if used incorrectly

**This function may be removed or significantly changed**:
- Only use under guidance from pg_tviews team
- Not recommended for automated operations
- May be replaced with safer alternative

---

## DEPRECATED Functions

*None currently, but example format:*

### pg_tviews_legacy_refresh_all() [DEPRECATED in 0.2]
**Status**: DEPRECATED (Remove in v1.0)
**Replacement**: `pg_tviews_refresh_all(filter_pattern TEXT DEFAULT '%')`
**Migration**: See PHASE_4.3 breaking changes guide
**Removal Date**: 2026-06-01

```sql
-- OLD (deprecated)
SELECT pg_tviews_legacy_refresh_all();

-- NEW (use instead)
SELECT pg_tviews_refresh_all('%');
```
```

### Step 2: Audit Rust API Surface

**File**: `src/lib.rs` (add documentation)

Add to each public export:

```rust
//! # Public API Documentation
//!
//! This module exports the public Rust API for pg_tviews.
//! Stability levels defined in `docs/api/RUST_FUNCTIONS.md`

pub mod refresh {
    //! Core refresh operations (STABLE)
    //!
    //! These functions form the foundation of pg_tviews and are committed
    //! to long-term compatibility.

    pub use crate::refresh::main::refresh_pk;
    pub use crate::refresh::batch::refresh_batch;
    // All functions in this module are STABLE
}

pub mod dependency {
    //! Dependency analysis and graph traversal (STABLE)
    //!
    //! Stable API for understanding TVIEW dependencies.

    pub use crate::dependency::graph::DependencyGraph;
    pub use crate::dependency::graph::find_base_tables;
    // STABLE functions
}

pub mod error {
    //! Error types and handling (STABLE)
    //!
    //! Error types are part of the public API contract.
    //! New error variants may be added in minor versions.

    pub use crate::error::mod::{TViewError, TViewResult};
}

pub mod catalog {
    //! Internal metadata queries (EVOLVING)
    //!
    //! These functions allow introspection into the internal catalog.
    //! API may change as catalog representation evolves.
    //! Do not rely in production code.

    pub use crate::catalog::{TviewMeta, DependencyDetail};
}

pub mod hooks {
    //! PostgreSQL lifecycle hooks (INTERNAL)
    //!
    //! These are internal hooks called by PostgreSQL.
    //! Not part of the public API.
    //! Usage by external code is not supported.

    pub(crate) use crate::hooks::*;
}
```

**Create**: `docs/api/RUST_FUNCTIONS.md`

```markdown
# pg_tviews Rust API Reference

## Module: refresh (STABLE)

### refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()>
**Status**: STABLE
**Description**: Refresh a single row in a TVIEW by primary key
**Guarantees**:
- Function signature unchanged
- Behavior unchanged except performance optimization
- Error handling maintained

---

### refresh_batch(entity: &str, pk_values: &[i64]) -> TViewResult<usize>
**Status**: STABLE
**Description**: Batch refresh multiple rows
**Guarantees**: Same as refresh_pk, plus return value stability

---

## Module: dependency (STABLE)

### find_base_tables(view_name: &str) -> TViewResult<DependencyGraph>
**Status**: STABLE
**Description**: Determine base tables for a TVIEW
**Guarantees**: DependencyGraph structure stable

---

## Type: ViewRow (STABLE)

**Status**: STABLE
**Fields**: All fields guaranteed stable (may add optional fields)
**Methods**: All methods guaranteed stable

---

## Type: TViewError (STABLE)

**Status**: STABLE (enum variants backward compatible)
**Guarantee**: New variants added, never removed
**Matching**: Use non-exhaustive patterns or match ALL branches

```rust
match error {
    TViewError::MetadataNotFound { entity } => ...,
    TViewError::RefreshFailed { .. } => ...,
    // Always include wildcard for forward compatibility
    _ => ...
}
```

---

## Module: catalog (EVOLVING)

**Status**: EVOLVING
**Warning**: Internal catalog representation may change
**Known Future Changes**:
- Cache invalidation strategy
- Metadata storage format
- Query performance optimizations

---

## Module: hooks (INTERNAL)

Not part of public API. Do not use in external code.
```

### Step 3: Create API Stability Registry

**File**: `docs/api/STABILITY_REGISTRY.json`

```json
{
  "version": "0.1.0-beta.1",
  "last_updated": "2025-12-13",
  "api_items": [
    {
      "name": "pg_tviews_convert_existing_table",
      "type": "sql_function",
      "stability": "STABLE",
      "since_version": "0.1.0-beta.1",
      "breaking_changes": [],
      "deprecated": false,
      "notes": "Core functionality, long-term commitment"
    },
    {
      "name": "pg_tviews_metadata",
      "type": "sql_function",
      "stability": "STABLE",
      "since_version": "0.1.0-beta.1",
      "breaking_changes": [],
      "deprecated": false,
      "output_columns": ["entity_name", "primary_key", "created_at", "last_refreshed", "rows_cached"]
    },
    {
      "name": "pg_tviews_get_queue",
      "type": "sql_function",
      "stability": "EVOLVING",
      "since_version": "0.1.0-beta.1",
      "planned_changes": "Output structure may change for performance",
      "target_stability_version": "1.1.0",
      "deprecated": false
    },
    {
      "name": "pg_tviews_clear_queue",
      "type": "sql_function",
      "stability": "EXPERIMENTAL",
      "since_version": "0.1.0-beta.1",
      "warning": "Advanced debugging only, not for production use",
      "deprecated": false
    },
    {
      "name": "refresh_pk",
      "type": "rust_function",
      "stability": "STABLE",
      "since_version": "0.1.0-beta.1",
      "module": "refresh",
      "breaking_changes": []
    },
    {
      "name": "ViewRow",
      "type": "rust_struct",
      "stability": "STABLE",
      "since_version": "0.1.0-beta.1",
      "module": "refresh",
      "breaking_changes": []
    },
    {
      "name": "TViewError",
      "type": "rust_enum",
      "stability": "STABLE",
      "since_version": "0.1.0-beta.1",
      "module": "error",
      "breaking_changes": "New variants only (never removed)",
      "variants_as_of_latest": [
        "MetadataNotFound",
        "RefreshFailed",
        "CacheMiss",
        "SerializationFailed"
      ]
    }
  ],
  "stability_policy": {
    "STABLE": {
      "commitment": "Guaranteed compatible within major version",
      "deprecation_notice_months": 12,
      "breaking_change_policy": "Major version only"
    },
    "EVOLVING": {
      "commitment": "May change in minor versions",
      "deprecation_notice_months": 6,
      "target_stability": "Future minor version"
    },
    "EXPERIMENTAL": {
      "commitment": "No compatibility guarantee",
      "deprecation_notice_months": 0,
      "warning": "Use only for testing, not production"
    },
    "DEPRECATED": {
      "commitment": "Scheduled removal",
      "removal_version": "TBD",
      "deprecation_notice_months": 6,
      "replacement_api": "Documented in phase-4.3"
    }
  }
}
```

### Step 4: Generate API Documentation Matrix

**File**: `docs/api/API_MATRIX.md`

```markdown
# pg_tviews API Stability Matrix

## Quick Reference

| Function | Type | Stability | Since | Maturity | Recommendation |
|----------|------|-----------|-------|----------|-----------------|
| pg_tviews_convert_existing_table | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_metadata | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_get_queue | SQL | EVOLVING | 0.1.0-beta.1 | Debug only | ⚠️ May change |
| pg_tviews_clear_queue | SQL | EXPERIMENTAL | 0.1.0-beta.1 | Advanced | ❌ Experts only |
| pg_tviews_refresh_all | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| pg_tviews_dependency_graph | SQL | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| refresh_pk | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| refresh_batch | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| ViewRow | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |
| TViewError | Rust | STABLE | 0.1.0-beta.1 | Production | ✅ Safe |

## Stability Guarantees by Version

### 0.1.x - Beta Period
- All STABLE functions guaranteed compatible
- EVOLVING functions may change
- EXPERIMENTAL functions may disappear

### 1.0.x - Production Release
- All STABLE functions guaranteed compatible
- EVOLVING functions may change in 1.1+
- EXPERIMENTAL functions stabilize or deprecate

### 2.0.x - Next Major Release
- Breaking changes allowed for all APIs
- Clear migration path required for each change
- 12+ month deprecation notice

## Using Stable APIs in Production

✅ **Recommended**: Use STABLE functions in production
- Safe to upgrade minor versions (0.1 → 0.2 → 1.0)
- Breaking changes only in major versions
- 12+ month deprecation notice for any removals

⚠️ **Caution**: EVOLVING APIs in production
- May change in minor versions
- Monitor release notes carefully
- Consider pinning to specific version

❌ **Not Recommended**: EXPERIMENTAL APIs in production
- No compatibility guarantee
- Use only for debugging/testing
- Do not rely on in automation

---

## Future Stability Targets (v1.0+)

| Current Status | Target Status | Target Version | Notes |
|---|---|---|---|
| EVOLVING | STABLE | 1.1 | pg_tviews_get_queue output schema |
| EXPERIMENTAL | Deprecated | 1.0 | pg_tviews_clear_queue (needs safer alternative) |
| EXPERIMENTAL | STABLE | 1.0 | Advanced refresh tuning APIs |
```

### Step 5: Add Compatibility Notes to README

**File**: `README.md` (append section)

```markdown
## API Stability & Compatibility

### For Users

pg_tviews follows semantic versioning with three stability tiers:

- **STABLE APIs**: Guaranteed compatible across minor versions (0.1 → 0.2 → 1.0)
- **EVOLVING APIs**: May change in minor versions, stabilize before 1.0
- **EXPERIMENTAL APIs**: No compatibility guarantee, debugging only

See [API Stability Guide](./docs/api/SQL_FUNCTIONS.md) for detailed contracts.

### Version Guarantees

| Version | Stability | Production Ready |
|---------|-----------|------------------|
| 0.1.x | Beta | Yes, with caution |
| 1.0.x | Stable | Yes |
| 2.0.x | Next Gen | Future |

### Migration Guide

Upgrading between versions:
- **0.1 → 0.2**: STABLE functions guaranteed to work
- **0.2 → 1.0**: STABLE functions guaranteed to work
- **1.0 → 1.1**: Minor version updates, STABLE functions only
- **1.x → 2.0**: Breaking changes possible, migration guide required

See [CHANGELOG.md](./CHANGELOG.md) and [PHASE 4.3 Breaking Changes](./docs/phases/phase-4.3-breaking-changes.md).

### Getting Help

- **STABLE APIs**: Safe to use, fully supported
- **EVOLVING APIs**: Consider alternatives, report issues
- **EXPERIMENTAL APIs**: Development only, not supported for production
```

### Step 6: Create Breaking Changes Register (Baseline)

**File**: `docs/api/BREAKING_CHANGES.md`

```markdown
# Known Breaking Changes (pg_tviews)

## Current Version: 0.1.0-beta.1

### No Breaking Changes (Beta Period)

This is the initial beta release. No previous versions to break from.

---

## Planned Breaking Changes for v2.0+

See [Phase 4.3: Breaking Changes Roadmap](../phases/phase-4.3-breaking-changes.md)

---

## Deprecation Policy

### Timeline
1. **Current Release**: New deprecation announced in release notes
2. **Next Minor**: Deprecation warnings in code (if applicable)
3. **+6 months**: Minimum before removal in patch release
4. **+12 months**: Preferred before removal in minor version
5. **Major version**: Can remove without notice if properly deprecated

### Removal Example
- v0.2.0: Feature X deprecated (Aug 2025)
- v0.3.0: Deprecation warning added (Sep 2025)
- v0.5.0 (Feb 2026): Can be removed (>6 months)
- v1.0.0 (Apr 2026): Should be removed (>12 months recommended)

### User Communication
- Release notes prominently feature deprecations
- Documentation updated with alternatives
- Error messages point to migration guide
- Forum/discussions alerted to changes
```

---

## Verification Commands

```bash
# 1. Generate API documentation
cargo doc --no-deps --open

# 2. Check for undocumented public items
cargo clippy -- -W missing_docs

# 3. Verify stability tags in code
grep -r "STABLE\|EVOLVING\|EXPERIMENTAL" docs/api/

# 4. Validate JSON registry
python3 -m json.tool docs/api/STABILITY_REGISTRY.json > /dev/null && echo "✅ Valid JSON"

# 5. Check all public functions are classified
python3 scripts/verify_api_coverage.py

# 6. Test API examples in documentation
cargo test --doc

# 7. Verify no breaking changes introduced (for patch releases)
./scripts/api_compat_check.sh v0.1.0-beta.1
```

---

## Acceptance Criteria

- [ ] All public SQL functions documented with stability level
- [ ] All public Rust types/functions documented with stability level
- [ ] Stability registry (JSON) created and valid
- [ ] API stability matrix generated (README section)
- [ ] Breaking changes register created (empty for beta)
- [ ] Rust docs compile with no warnings
- [ ] Examples in docs tested and working
- [ ] Deprecation policy documented and clear
- [ ] Migration paths defined for any breaking changes
- [ ] CHANGELOG.md includes stability notes

---

## DO NOT

- ❌ Mark production APIs as EXPERIMENTAL without strong justification
- ❌ Change STABLE classifications without major version bump
- ❌ Remove APIs without 6+ month deprecation period
- ❌ Introduce breaking changes in minor versions (for STABLE APIs)
- ❌ Document stability without examples of impact
- ❌ Forget to update stability registry when APIs change

---

## Common Classification Mistakes

**❌ WRONG**: "Query function has STABLE classification because it's being used"
- Usage doesn't determine stability, design intent does
- Check: Is the function's contract well-defined and unlikely to change?

**✅ RIGHT**: "Query function has STABLE because contract is well-defined"
- Input/output documented
- Error cases enumerated
- Performance characteristics defined
- Internal implementation can change safely

---

## Related Documentation

- [API Stability Guide](./docs/api/SQL_FUNCTIONS.md) - Detailed function contracts
- [RUST_FUNCTIONS.md](./docs/api/RUST_FUNCTIONS.md) - Rust module documentation
- [Phase 4.2: Versioning Strategy](./phase-4.2-versioning-strategy.md) - Semver policy
- [Phase 4.3: Breaking Changes](./phase-4.3-breaking-changes.md) - 2.0 roadmap

---

## Next Steps

After completion:
- Commit with message: `docs(api): Add stability classifications and registry [PHASE4.1]`
- Review with maintainers before Phase 4.2
- Proceed to **Phase 4.2: Versioning Strategy & Deprecation**
