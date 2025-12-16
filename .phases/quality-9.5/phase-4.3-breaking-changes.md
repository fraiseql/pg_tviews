# Phase 4.3: Breaking Changes Roadmap for 2.0

**Objective**: Identify and plan breaking changes for v2.0, with clear migration paths for all affected users

**Priority**: MEDIUM
**Estimated Time**: 2-3 days
**Blockers**: Phase 4.1-4.2 complete (API audit & versioning)

---

## Context

**Current State**: v0.1.0-beta.1 with potential design improvements identified

**Why This Matters**:
- v1.0 must commit to long-term API stability
- Breaking changes can only happen in major versions (2.0+)
- Early identification allows planning and user communication
- v2.0 timeline: ~18-24 months after v1.0 (Apr 2028)
- Users need minimum 12 months notice before removal

**Deliverable**: Comprehensive breaking changes roadmap with migration strategies

---

## Breaking Changes Evaluation Framework

### Criteria for Considering a Breaking Change

**Consider Breaking** if:
- Significantly improves user experience
- Simplifies API complexity
- Fixes fundamental design issues
- Requires deprecation period to communicate

**Avoid Breaking** if:
- Marginal improvement
- Workaround exists for users
- Can be accommodated in backward-compatible way
- Deprecation cost > benefit

### Impact Assessment

For each breaking change, document:

1. **User Impact**: How many users affected?
2. **Effort**: How hard to migrate?
3. **Benefit**: How valuable is the change?
4. **Migration Path**: Clear upgrade instructions?

---

## Proposed Breaking Changes for v2.0

### Category 1: API Simplification (HIGH PRIORITY)

#### 1.1: Simplify Entity Naming

**Current (Problem)**: Functions accept both `entity_name` and `table_name`
```rust
pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()>  // Opaque Oid
pub fn refresh_batch(entity: &str, pk_values: &[i64]) -> TViewResult<usize>  // String name

// Inconsistent: One takes Oid, one takes string name
```

**Proposed (v2.0)**: Standardize on clear naming and types
```rust
pub fn refresh_tview_row(tview_name: &str, primary_key: i64) -> TViewResult<()>
pub fn refresh_tview_rows(tview_name: &str, primary_keys: &[i64]) -> TViewResult<usize>

// Benefits:
// - Clear what each parameter means
// - No need to understand Oid representation
// - More discoverable API (better IDE support)
// - Consistent naming: "tview" always refers to materialized view
```

**Migration Path**:
```rust
// v1.x (STABLE)
pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()>

// v1.5+ (EVOLVING, prepare for change)
#[deprecated(since = "1.5.0", note = "Use refresh_tview_row instead")]
pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()> {
    // Forward to new function
    let tview = catalog::entity_for_table(source_oid)?;
    refresh_tview_row(&tview, pk)
}

pub fn refresh_tview_row(tview_name: &str, primary_key: i64) -> TViewResult<()> {
    // New implementation
}

// v2.0 (REMOVED)
// refresh_pk removed entirely
// Users MUST use refresh_tview_row
```

**User Migration (Example)**:
```rust
// OLD CODE (v1.x)
let oid = get_table_oid("public", "sales");
refresh_pk(oid, 42)?;

// NEW CODE (v2.0)
refresh_tview_row("public.sales", 42)?;
```

**Effort**: Low (clear 1:1 mapping)
**Benefit**: High (much clearer API)
**User Impact**: Medium (affects all refresh callers)

---

#### 1.2: Unify Error Handling

**Current (Problem)**: Multiple error types, inconsistent error reporting
```rust
pub enum TViewError {
    MetadataNotFound { entity: String },
    RefreshFailed { reason: String },
    CacheMiss { key: String },
    // ... 10+ more variants, some redundant
}

// Problems:
// - Too many variants (>15)
// - Some do same thing with different names
// - Hard for users to handle comprehensively
```

**Proposed (v2.0)**: Rationalized error hierarchy
```rust
pub enum TViewError {
    // Core errors (always exist)
    NotFound { entity: String, reason: String },  // Unified
    Refresh { entity: String, reason: String },   // Unified
    Internal { reason: String },                  // Catch-all

    // Optional detailed errors (for advanced users)
    #[cfg(feature = "detailed-errors")]
    CacheMiss { key: String },
    #[cfg(feature = "detailed-errors")]
    LockPoisoned { resource: String },
}

// Benefits:
// - Simpler pattern matching
// - Clearer error categories
// - Advanced features optional
// - Easier to handle generically: `match e { TViewError::Internal {..} => .. }`
```

**Migration Path**:
```rust
// v1.x: Both representations exist
pub enum TViewError {
    MetadataNotFound { entity: String },  // DEPRECATED
    CacheMiss { key: String },             // DEPRECATED
    NotFound { entity: String, reason: String },  // NEW
}

impl From<TViewError> for TViewError {
    // Automatic conversion for v1.x code
    fn from(old: OldTViewError) -> Self {
        match old {
            OldTViewError::MetadataNotFound { entity } => {
                TViewError::NotFound { entity, reason: "metadata not found".to_string() }
            }
            // ...
        }
    }
}

// v2.0: Old variants removed
pub enum TViewError {
    NotFound { entity: String, reason: String },
    Refresh { entity: String, reason: String },
    Internal { reason: String },
}
```

**User Migration (Example)**:
```rust
// OLD CODE (v1.x)
match refresh_pk(oid, pk) {
    Err(TViewError::MetadataNotFound { entity }) => println!("Not found: {}", entity),
    Err(TViewError::CacheMiss { key }) => println!("Cache miss: {}", key),
    Err(e) => println!("Other error: {:?}", e),
    Ok(_) => println!("Success"),
}

// NEW CODE (v2.0)
match refresh_tview_row(tview, pk) {
    Err(TViewError::NotFound { entity, reason }) => println!("Not found: {} ({})", entity, reason),
    Err(TViewError::Refresh { entity, reason }) => println!("Refresh failed: {} ({})", entity, reason),
    Err(TViewError::Internal { reason }) => println!("Internal error: {}", reason),
    Ok(_) => println!("Success"),
}
```

**Effort**: Medium (affects error handling in all callers)
**Benefit**: High (much simpler error patterns)
**User Impact**: High (any error handling code affected)

---

### Category 2: Feature Consolidation (MEDIUM PRIORITY)

#### 2.1: Merge Refresh Functions

**Current (Problem)**: Multiple refresh functions with overlapping functionality
```sql
SELECT pg_tviews_refresh_one(entity, pk);          -- One row
SELECT pg_tviews_refresh_batch(entity, pk_array);  -- Multiple rows
SELECT pg_tviews_refresh_all(schema_pattern);      -- All matching
SELECT pg_tviews_refresh_cascade(entity);          -- With cascade

-- Problems:
-- - Too many functions to remember
-- - Similar implementations
-- - Unclear which to use for performance
```

**Proposed (v2.0)**: Single unified refresh function
```sql
-- Single function, flexible parameters
SELECT pg_tviews_refresh(
    tview_name => 'public.sales',
    primary_keys => ARRAY[1, 2, 3],  -- Optional, omit for all
    cascade => true,                  -- Optional
    priority => 'high'                -- Optional
);
```

**Migration Path**:
```sql
-- v1.x: Keep all functions
SELECT pg_tviews_refresh_one(entity, pk);     -- DEPRECATED in 1.5
SELECT pg_tviews_refresh_batch(entity, pks);  -- DEPRECATED in 1.5

-- v2.0: Single function
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[1,2,3]);
```

**User Migration (Example)**:
```sql
-- OLD (v1.x)
SELECT pg_tviews_refresh_one('public.sales', 42);
SELECT pg_tviews_refresh_batch('public.sales', ARRAY[1,2,3]);
SELECT pg_tviews_refresh_all('public.%');

-- NEW (v2.0)
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[42]);
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[1,2,3]);
SELECT pg_tviews_refresh(tview_name => 'public.%');
```

**Effort**: Low (additive, old functions still work during v1.x)
**Benefit**: High (much simpler API)
**User Impact**: Low (optional migration)

---

#### 2.2: Remove Experimental Queue Debugging Functions

**Current (Problem)**: Queue debugging functions are experimental and cumbersome
```sql
SELECT * FROM pg_tviews_get_queue();        -- EXPERIMENTAL
SELECT pg_tviews_clear_queue();             -- EXPERIMENTAL, dangerous
SELECT pg_tviews_queue_stats();             -- EXPERIMENTAL
```

**Proposed (v2.0)**: Remove in favor of stable monitoring views
```sql
-- NEW: Stable monitoring interface
SELECT * FROM pg_tviews_queue_status;       -- STABLE view
SELECT * FROM pg_tviews_refresh_statistics; -- STABLE view
SELECT * FROM pg_tviews_cache_info;         -- STABLE view

-- No equivalent for clear_queue (was dangerous, never use)
```

**Rationale**:
- Queue is internal implementation detail
- Users shouldn't manually clear queue (data corruption risk)
- Monitoring via stable views is safer and clearer
- Experimentation period (v0.1-v1.5) enough to gather feedback

**Migration Path**:
```sql
-- v1.0-1.4: EXPERIMENTAL functions exist
SELECT * FROM pg_tviews_get_queue();        -- Works but deprecated in warnings

-- v1.5: Deprecation notices added
SELECT * FROM pg_tviews_get_queue();
-- WARNING: This function is deprecated
-- Use: pg_tviews_queue_status view instead
-- See: docs/migration/1.5-upgrade-guide.md

-- v2.0: Functions removed
SELECT * FROM pg_tviews_get_queue();        -- ERROR: function not found
SELECT * FROM pg_tviews_queue_status;       -- NEW: Use this instead
```

**User Migration (Example)**:
```sql
-- OLD (v1.x, for debugging only)
SELECT * FROM pg_tviews_get_queue()
WHERE entity = 'public.sales';

-- NEW (v2.0, safer alternative)
SELECT * FROM pg_tviews_queue_status
WHERE entity = 'public.sales'
  AND status = 'pending';
```

**Effort**: Very Low (only affects advanced users)
**Benefit**: Medium (safer API, clearer intent)
**User Impact**: Very Low (debugging only, few users)

---

### Category 3: Schema/Storage Changes (LOW PRIORITY)

#### 3.1: Reorganize Internal Schema

**Current (Problem)**: Internal tables scattered, hard to manage
```
pg_tviews_metadata         -- TVIEW config
pg_tviews_cache            -- Cache data
pg_tviews_queue_persistence -- Queue state
pg_tviews_audit_log        -- Audit trail
```

**Proposed (v2.0)**: Organize into schema
```
pg_tviews.metadata
pg_tviews.cache
pg_tviews.queue_persistence
pg_tviews.audit_log
```

**Rationale**:
- Cleaner schema organization
- Easier to manage (drop schema pg_tviews if needed)
- Consistent with other PostgreSQL extensions (e.g., pglogical)

**Impact**: Internal only, hidden from users
**Effort**: Low (transparent migration)
**User Impact**: None (if internal tables not referenced directly)

---

#### 3.2: Change Metadata Table Structure

**Current (Problem)**: Metadata columns may need optimization
```sql
CREATE TABLE pg_tviews_metadata (
    tview_oid OID,              -- Works but opaque
    entity_name TEXT,
    primary_key_column TEXT,
    select_definition TEXT,
    created_at TIMESTAMP,
    -- Missing: last_refresh_time, row_count, ...
);
```

**Proposed (v2.0)**: Enhanced metadata structure
```sql
CREATE TABLE pg_tviews.metadata (
    tview_oid OID,
    tview_name TEXT,                    -- NEW
    schema_name TEXT,                   -- NEW (split from name)
    table_name TEXT,                    -- NEW (split from name)
    primary_key TEXT[],                 -- Array instead of single column
    select_definition TEXT,
    created_at TIMESTAMP,
    updated_at TIMESTAMP,               -- NEW
    last_refresh_at TIMESTAMP,          -- NEW
    row_count BIGINT,                   -- NEW
    status TEXT CHECK (status IN ('active', 'suspended', 'error')),  -- NEW
);
```

**Migration Path**: Automatic migration script during upgrade

**User Impact**: None (users query via stable function interface)

---

### Category 4: Behavioral Changes (MEDIUM PRIORITY)

#### 4.1: Change Default Refresh Mode

**Current (Problem)**: No configurable refresh behavior
```rust
// Only one strategy: immediate refresh on change
// Some users want: batched refresh at specific times
// Some users want: manual refresh only
```

**Proposed (v2.0)**: Configurable refresh policies
```sql
CREATE TABLE pg_tviews.policies (
    tview_name TEXT,
    refresh_mode TEXT,    -- 'immediate', 'batched', 'manual'
    batch_interval INT,   -- milliseconds, if batched
    max_batch_size INT,   -- rows, if batched
    priority INT,         -- refresh priority (1-100)
);

SELECT pg_tviews_set_policy('public.sales',
    refresh_mode => 'batched',
    batch_interval => 5000
);
```

**Rationale**:
- Immediate refresh not optimal for all use cases
- Batching reduces lock contention
- Advanced users need fine-grained control

**Migration Path**: Default to 'immediate' (current behavior) in v1.x

**User Migration**: Optional in v1.5+, becomes configurable in v2.0

---

### Category 5: Removed Features (DO NOT PLAN YET)

**None identified for v2.0**

These will be evaluated when closer to v2.0 release:
- PostgreSQL version drops (e.g., drop PG13 support if obsolete)
- Deprecated dependencies
- Experimental features that didn't gain adoption

---

## Breaking Changes Timeline

### v1.0 (April 2026) - Current Design
- All breaking changes from 0.1.x frozen
- STABLE API contract established
- 12-month notice period begins

### v1.1-1.4 (May 2026 - March 2028)
- New features (backward compatible)
- Deprecation notices added to v1.x code
- Migration guides published
- User feedback collected on breaking changes

### v1.5 (March 2028)
- Deprecation warnings visible (last 1.x version)
- All v2.0 changes documented
- Migration guides finalized

### v2.0 (April 2028) - Breaking Changes Release
- **18+ months** after v1.0 (exceeds 12-month requirement)
- All breaking changes implemented
- Complete migration guide published
- Support for v1.x continues for 1 year (until April 2029)

```
v0.1 (Dec 2025)
    ↓ [Beta period, breaking changes OK]
v1.0 (Apr 2026) [Production stable, freezes API]
    ↓ [Users told: changes coming in v2.0]
v1.5 (Mar 2028) [Last 1.x, last chance to deprecate]
    ↓ [>12 months notice]
v2.0 (Apr 2028) [Breaking changes released]
    ↓ [v1.x supported for 1 more year]
v1.x EOL (Apr 2029) [v1.x support ends]
```

---

## Implementation Steps

### Step 1: Create Breaking Changes Catalogue

**File**: `docs/BREAKING_CHANGES_V2.0.md`

```markdown
# Breaking Changes Planned for v2.0

## Summary

pg_tviews v2.0 (planned April 2028) will include the following breaking changes:

| # | Category | Change | Impact | Migration | Effort |
|----|----------|--------|--------|-----------|--------|
| 1 | API | Simplify entity naming (refresh_pk → refresh_tview_row) | HIGH | Function rename, parameter changes | Low |
| 2 | API | Unify error handling (reduce error variants) | HIGH | Error handling code changes | Medium |
| 3 | API | Merge refresh functions (multiple functions → one) | MEDIUM | Function calls updated | Low |
| 4 | API | Remove queue debugging functions | LOW | Debug code removal | Very Low |
| 5 | Schema | Reorganize internal schema (tables → pg_tviews schema) | NONE | Transparent migration | Low |
| 6 | Schema | Enhance metadata structure | NONE | Transparent migration | Low |
| 7 | Behavior | Configurable refresh policies | MEDIUM | Optional feature, backward compat available | Medium |

## Detailed Sections

[Each category as detailed above]

## User Action Required

By upgrade to v2.0, users should:
- [ ] Update all refresh_pk() calls to refresh_tview_row()
- [ ] Update error handling code for new error structure
- [ ] Replace multiple refresh_*() calls with unified pg_tviews_refresh()
- [ ] Remove any queue debugging code (was never recommended for production)
- [ ] Review migration guides for other changes
- [ ] Test in staging environment before production upgrade

## Support Timeline

| Version | Release | End of Life |
|---------|---------|------------|
| 1.x | Apr 2026 | Apr 2029 |
| 2.0 | Apr 2028 | Apr 2030 |

Users have **24 months** to migrate from 1.x to 2.0.

## Getting Help

For migration questions:
- Documentation: [docs/migration/v2.0-upgrade-guide.md](docs/migration/v2.0-upgrade-guide.md)
- Community forums: [GitHub Discussions](https://github.com/your-org/pg_tviews/discussions)
- Issues: [GitHub Issues](https://github.com/your-org/pg_tviews/issues)
```

### Step 2: Create Migration Guides (Template)

**File**: `docs/migration/v2.0-upgrade-guide.md`

```markdown
# Upgrade Guide: pg_tviews 1.x → 2.0

This guide helps you migrate your code from v1.x to v2.0.

**Estimated time**: 30 minutes to 2 hours (depends on usage)

## Breaking Changes Overview

### 1. Function Renaming (REQUIRED)

**Change**: `refresh_pk()` → `refresh_tview_row()`

**Before (v1.x)**:
```rust
use pg_tviews::refresh::refresh_pk;
use pg_tviews::catalog::entity_for_table;

let oid = get_table_oid("public", "sales");
refresh_pk(oid, 42)?;
```

**After (v2.0)**:
```rust
use pg_tviews::refresh::refresh_tview_row;

refresh_tview_row("public.sales", 42)?;
```

**Why**: Clearer naming, no need to understand Oid representation

---

### 2. Error Handling (REQUIRED)

**Change**: Consolidated error variants

**Before (v1.x)**:
```rust
match result {
    Err(TViewError::MetadataNotFound { entity }) => {...}
    Err(TViewError::CacheMiss { key }) => {...}
    Err(TViewError::RefreshFailed { reason }) => {...}
    _ => {...}
}
```

**After (v2.0)**:
```rust
match result {
    Err(TViewError::NotFound { entity, reason }) => {...}
    Err(TViewError::Refresh { entity, reason }) => {...}
    Err(TViewError::Internal { reason }) => {...}
}
```

**Why**: Fewer error variants, simpler pattern matching

---

### 3. SQL Function Consolidation (OPTIONAL)

**Change**: Multiple refresh functions → unified function

**Before (v1.x)**:
```sql
SELECT pg_tviews_refresh_one('public.sales', 42);
SELECT pg_tviews_refresh_batch('public.sales', ARRAY[1,2,3]);
SELECT pg_tviews_refresh_all('public.%');
```

**After (v2.0)**:
```sql
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[42]);
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[1,2,3]);
SELECT pg_tviews_refresh(tview_name => 'public.%');
```

**Why**: Single function is simpler and more discoverable

---

### 4. Removed Debugging Functions (OPTIONAL)

**Change**: Experimental queue functions removed

**Before (v1.x)**:
```sql
-- For debugging only
SELECT * FROM pg_tviews_get_queue();
SELECT pg_tviews_clear_queue();  -- DANGEROUS
```

**After (v2.0)**:
```sql
-- Use stable views instead
SELECT * FROM pg_tviews_queue_status;
SELECT * FROM pg_tviews_refresh_statistics;
```

**Why**: Safer, clearer intent, stable interface

---

## Migration Checklist

- [ ] Review all refresh_pk() calls and rename to refresh_tview_row()
- [ ] Update error handling code for new error structure
- [ ] Replace multiple refresh_*() SQL calls with unified pg_tviews_refresh()
- [ ] Remove any code that calls pg_tviews_clear_queue() (should never be in production anyway)
- [ ] Test thoroughly in staging environment
- [ ] Plan database migration during maintenance window (data migration may lock tables)
- [ ] Verify backups before upgrading
- [ ] Monitor logs after upgrade for any issues

## Rollback Procedure

If issues occur after upgrade:

```bash
# Downgrade to v1.x (rollback)
cargo install pg_tviews --version "1.5"
cargo pgrx install --release

# Restore from backup if data structure changed
# (See docs/disaster-recovery.md)
```

## Estimated Impact by Usage Pattern

| Pattern | Impact | Effort |
|---------|--------|--------|
| Basic TVIEW queries only | None | 0 hours |
| Custom Rust refresh logic | HIGH | 1-2 hours |
| SQL procedures with refresh | MEDIUM | 30 min - 1 hour |
| Error handling code | HIGH | 30 min - 1 hour |
| Queue debugging code | LOW | 5-15 min |

## FAQ

**Q: Can I upgrade from v0.1.x directly to v2.0?**
A: No, you must upgrade through v1.0 or v1.5 first. See [v0.1 → v1.0 migration](./v1.0-upgrade-guide.md).

**Q: What if I don't want to migrate?**
A: v1.x will be supported through April 2029 (2+ years). You can continue using v1.x safely.

**Q: How long does migration take?**
A: Most users: 30 minutes. Complex integrations: up to 2 hours.

**Q: Is there a tool to automate migration?**
A: Partially. Use `cargo fix` for Rust code, manual review of SQL still needed.

## Getting Help

- Documentation: [docs/](../docs/)
- Community: [GitHub Discussions](...)
- Issues: [GitHub Issues](...)
```

### Step 3: Create Decision Record

**File**: `docs/adr/ADR-001-breaking-changes-v2.0.md`

```markdown
# ADR-001: Breaking Changes for v2.0

## Status
PROPOSED (to be finalized 12+ months before v2.0 release)

## Context

pg_tviews v1.0 will establish long-term API stability commitment. However, some design issues prevent optimal user experience:

1. **API Naming Inconsistency**: `refresh_pk(Oid)` vs `refresh_batch(String)`
2. **Error Verbosity**: 15+ error variants when 3-4 would suffice
3. **Function Explosion**: 4 refresh functions could be 1 with optional parameters

These issues don't have backward-compatible fixes.

## Decision

Accept breaking changes in v2.0 for significant UX improvement.

### Changes Approved

| Change | Rationale | Timeline |
|--------|-----------|----------|
| Rename refresh_pk → refresh_tview_row | Clarity | v1.5 deprecate, v2.0 remove |
| Consolidate error variants | Simpler matching | v1.5 deprecate, v2.0 remove |
| Merge refresh functions | Single API | v1.5 deprecate, v2.0 remove |
| Remove queue debugging functions | Safety | v1.5 deprecate, v2.0 remove |

### Not Approved (Stay Compatible)

- Internal schema changes (transparent)
- Performance optimizations (always backward compat)
- New optional features (backward compat)

## Consequences

**Positive**:
- Much simpler, more usable API
- Fewer error cases to handle
- Better IDE discovery
- Easier to teach/learn

**Negative**:
- Users must migrate code
- 12-18 months of dual-API support needed
- Documentation burden (migration guides)

**Timeline**:
- v1.0 (Apr 2026): Current API frozen
- v1.5 (Mar 2028): Deprecation warnings added
- v2.0 (Apr 2028): Breaking changes released
- Minimum notice: 12 months, actual: 18 months

## Related Decisions

- [Phase 4.1: API Audit](../phases/phase-4.1-api-audit.md)
- [Phase 4.2: Versioning Strategy](../phases/phase-4.2-versioning-strategy.md)

## References

- [Semantic Versioning](https://semver.org/)
- [Deprecation Policy](../VERSIONING.md)
```

### Step 4: Create Feasibility Assessment

**File**: `docs/breaking-changes-assessment.md`

```markdown
# Breaking Changes Feasibility Assessment

## Summary Table

| Change | Complexity | Risk | User Impact | Feasibility |
|--------|-----------|------|-------------|-------------|
| Rename functions | Low | Low | Medium | ✅ GREEN |
| Consolidate errors | Medium | Medium | Medium | ✅ GREEN |
| Merge functions | Low | Low | Low | ✅ GREEN |
| Remove debugging | Very Low | Very Low | Very Low | ✅ GREEN |
| Schema reorganization | Low | Low | None | ✅ GREEN |

## Per-Change Assessment

### 1. Function Renaming

**Complexity**: Low
- Straightforward rename + signature change
- No logic changes needed
- Clear migration path (deprecation wrapper)

**Risk**: Low
- Type safety enforces updates
- Clear compilation errors on old names
- Easy to test migration

**Feasibility**: HIGH (PROCEED)

---

### 2. Error Consolidation

**Complexity**: Medium
- All error handling code must be updated
- Needs careful testing of error paths
- Pattern matching code changes

**Risk**: Medium
- Must ensure no error cases are lost
- Some error context might be reduced
- Users must audit error handling

**Feasibility**: HIGH (PROCEED, with caution)

---

### 3. Function Consolidation

**Complexity**: Low
- Consolidation is additive (keep old functions in v1.x)
- New function easy to implement
- Gradual migration possible

**Risk**: Low
- Can phase in gradually
- Easy to test both old and new
- No hard cutoff needed

**Feasibility**: HIGH (PROCEED)

---

### 4. Remove Debugging Functions

**Complexity**: Very Low
- Only affects experimental, undocumented functions
- Very few users (if any) depend on them
- Stable alternatives exist

**Risk**: Very Low
- Breaking change notice OK
- No production code should use these
- Clear alternative provided

**Feasibility**: HIGH (PROCEED)

---

## Overall Feasibility

**RECOMMENDATION**: Proceed with all planned breaking changes for v2.0

**Confidence Level**: High (80%+)
- All changes have clear migration paths
- No fundamental architectural issues
- User impact is manageable with proper notice
- 18-month notice period exceeds minimum requirement

## Risk Mitigation

1. **Extended deprecation period**: Keep v1.5 available for 12+ months
2. **Comprehensive guides**: Detailed migration guide per change
3. **Automated tools**: cargo-fix and code rewrite helpers where possible
4. **Community feedback**: Gather feedback during v1.x development

---

## Next Steps

1. Finalize exact breaking changes in Phase 4.3
2. Start deprecation notices in v1.5 (March 2028)
3. Publish migration guides with v1.5 release
4. Implement changes in v2.0 (April 2028)
5. Support v1.x for additional 12 months
```

---

## Verification Commands

```bash
# 1. Check which APIs are planned for breaking changes
grep -r "BREAKING CHANGE\|deprecated\|will be removed in v2" docs/

# 2. Verify deprecation markers in code
grep -r "deprecated(since" src/ --include="*.rs"

# 3. Check migration guides exist for each breaking change
ls -la docs/migration/v2.0-*.md

# 4. Validate ADR document exists
test -f docs/adr/ADR-001-breaking-changes-v2.0.md && echo "✅ ADR exists"

# 5. Verify timeline is documented
grep -A 5 "v2.0" docs/BREAKING_CHANGES_V2.0.md | head -20

# 6. Check consistency between all docs
wc -l docs/BREAKING_CHANGES_V2.0.md docs/migration/v2.0-upgrade-guide.md docs/adr/ADR-001-*
```

---

## Acceptance Criteria

- [ ] Breaking changes catalog created with all planned changes
- [ ] Each breaking change has detailed migration path
- [ ] Migration guide template created and filled for each change
- [ ] ADR document created with decision rationale
- [ ] Feasibility assessment completed and approved
- [ ] Timeline documented (minimum 12 months notice)
- [ ] Impact assessment completed for each change
- [ ] Rollback procedures documented
- [ ] User communication plan created
- [ ] FAQ section covers common concerns

---

## DO NOT

- ❌ Plan breaking changes for v1.x (stability commitment)
- ❌ Make breaking changes without 12+ month notice
- ❌ Remove features without deprecated alternative
- ❌ Forget to document migration paths clearly
- ❌ Assume users will find migration guides (must be prominent)
- ❌ Make breaking changes for marginal improvements
- ❌ Remove error handling capability without replacement

---

## Common Pitfalls

**❌ WRONG**: "Users can just update their code"
- Some users have enterprise change control processes
- Requires testing, approval cycles
- 12+ months notice is minimum, not excessive

**✅ RIGHT**: "Provide clear migration guide + tool support"
- Document before/after examples
- Provide code rewrite hints (cargo-fix)
- Offer migration helper scripts

**❌ WRONG**: "Consolidate everything for simplicity"
- Breaks user code unnecessarily
- Not all consolidations are improvements
- Users may have good reasons to use current API

**✅ RIGHT**: "Change only if significant UX improvement"
- Weigh cost to users against benefit
- Get user feedback during v1.x period
- Only break if benefit clearly exceeds cost

---

## Related Documentation

- [Phase 4.1: API Audit](./phase-4.1-api-audit.md) - API classification
- [Phase 4.2: Versioning Strategy](./phase-4.2-versioning-strategy.md) - Deprecation policy
- [CHANGELOG.md](../CHANGELOG.md) - Release notes template
- [VERSIONING.md](../docs/VERSIONING.md) - Semantic versioning policy

---

## Next Steps

After completion:
- Commit with message: `docs(breaking-changes): Add v2.0 roadmap with migration paths [PHASE4.3]`
- Share with community for feedback during v1.x development
- Use as roadmap for v1.x deprecation planning
- Revisit before v1.5 release (March 2028) to finalize
- Publish migration guides with v1.5 release
