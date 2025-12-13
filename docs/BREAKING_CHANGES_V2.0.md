# Breaking Changes Planned for v2.0

## Summary

pg_tviews v2.0 (planned April 2028) will include the following breaking changes to improve API clarity, reduce complexity, and enhance maintainability:

| # | Category | Change | Impact | Migration | Effort |
|----|----------|--------|--------|-----------|--------|
| 1 | API | Simplify entity naming (refresh_pk → refresh_tview_row) | HIGH | Function rename, parameter changes | Low |
| 2 | API | Unify error handling (reduce error variants) | HIGH | Error handling code changes | Medium |
| 3 | API | Merge refresh functions (multiple functions → one) | MEDIUM | Function calls updated | Low |
| 4 | API | Remove queue debugging functions | LOW | Debug code removal | Very Low |
| 5 | Schema | Reorganize internal schema (tables → pg_tviews schema) | NONE | Transparent migration | Low |
| 6 | Schema | Enhance metadata structure | NONE | Transparent migration | Low |
| 7 | Behavior | Configurable refresh policies | MEDIUM | Optional feature, backward compat available | Medium |

## Timeline

- **Announcement**: December 2025 (this document)
- **Deprecation Period**: April 2026 - April 2028 (24 months)
- **v2.0 Release**: April 2028
- **v1.x End of Life**: April 2029 (12 months after v2.0)

Users have **24 months** to migrate from 1.x to 2.0, with deprecation warnings starting in v1.5 (October 2026).

## User Action Required

By upgrade to v2.0, users should:
- [ ] Update all `refresh_pk()` calls to `refresh_tview_row()`
- [ ] Update error handling code for new error structure
- [ ] Replace multiple `refresh_*()` calls with unified `pg_tviews_refresh()`
- [ ] Remove any queue debugging code (was never recommended for production)
- [ ] Review migration guides for other changes
- [ ] Test in staging environment before production upgrade

## Support Timeline

| Version | Release | End of Life | Support Level |
|---------|---------|-------------|---------------|
| 1.x | Apr 2026 | Apr 2029 | Full support + security patches |
| 2.0 | Apr 2028 | Apr 2030 | Full support + security patches |

---

## Detailed Breaking Changes

### 1. API Simplification: Entity Naming (HIGH PRIORITY)

#### Current Problem
Functions accept both `entity_name` and `table_name` with inconsistent parameter types:
```rust
pub fn refresh_pk(source_oid: Oid, pk: i64) -> spi::Result<()>  // Opaque Oid
pub fn refresh_batch(entity: &str, pk_values: &[i64]) -> TViewResult<usize>  // String name
```

#### Proposed Solution (v2.0)
Standardize on clear naming and types:
```rust
pub fn refresh_tview_row(tview_name: &str, primary_key: i64) -> TViewResult<()>
pub fn refresh_tview_rows(tview_name: &str, primary_keys: &[i64]) -> TViewResult<usize>
```

#### Benefits
- Clear what each parameter means
- No need to understand Oid representation
- More discoverable API (better IDE support)
- Consistent naming: "tview" always refers to materialized view

#### Migration Path
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

#### User Migration Example
```rust
// OLD CODE (v1.x)
let oid = get_table_oid("public", "sales");
refresh_pk(oid, 42)?;

// NEW CODE (v2.0)
refresh_tview_row("public.sales", 42)?;
```

#### Impact Assessment
- **User Impact**: HIGH (affects all refresh callers)
- **Migration Effort**: Low (clear 1:1 mapping)
- **Business Value**: High (much clearer API)

---

### 2. API Simplification: Error Handling (HIGH PRIORITY)

#### Current Problem
Multiple error types with inconsistent error reporting:
```rust
pub enum TViewError {
    MetadataNotFound { entity: String },
    RefreshFailed { reason: String },
    CacheMiss { key: String },
    // ... 10+ more variants, some redundant
}
```

Problems:
- Too many variants (>15)
- Some do same thing with different names
- Hard for users to handle comprehensively

#### Proposed Solution (v2.0)
Rationalized error hierarchy:
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
```

#### Benefits
- Simpler pattern matching
- Clearer error categories
- Advanced features optional
- Easier to handle generically

#### Migration Path
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

#### User Migration Example
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

#### Impact Assessment
- **User Impact**: HIGH (any error handling code affected)
- **Migration Effort**: Medium (affects error handling in all callers)
- **Business Value**: High (much simpler error patterns)

---

### 3. Feature Consolidation: Refresh Functions (MEDIUM PRIORITY)

#### Current Problem
Multiple refresh functions with overlapping functionality:
```sql
SELECT pg_tviews_refresh_one(entity, pk);          -- One row
SELECT pg_tviews_refresh_batch(entity, pk_array);  -- Multiple rows
SELECT pg_tviews_refresh_all(schema_pattern);      -- All matching
SELECT pg_tviews_refresh_cascade(entity);          -- With cascade
```

Problems:
- Too many functions to remember
- Similar implementations
- Unclear which to use for performance

#### Proposed Solution (v2.0)
Single unified refresh function:
```sql
-- Single function, flexible parameters
SELECT pg_tviews_refresh(
    tview_name => 'public.sales',
    primary_keys => ARRAY[1, 2, 3],  -- Optional, omit for all
    cascade => true,                  -- Optional
    priority => 'high'                -- Optional
);
```

#### Migration Path
```sql
-- v1.x: Keep all functions
SELECT pg_tviews_refresh_one(entity, pk);     -- DEPRECATED in 1.5
SELECT pg_tviews_refresh_batch(entity, pks);  -- DEPRECATED in 1.5

-- v2.0: Single function
SELECT pg_tviews_refresh(tview_name => 'public.sales', primary_keys => ARRAY[1,2,3]);
```

#### User Migration Example
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

#### Impact Assessment
- **User Impact**: MEDIUM (affects refresh function calls)
- **Migration Effort**: Low (straightforward parameter mapping)
- **Business Value**: Medium (simpler API surface)

---

### 4. Feature Removal: Queue Debugging Functions (LOW PRIORITY)

#### Current Problem
Queue debugging functions were added for development but never intended for production:
```sql
SELECT pg_tviews_debug_queue_status();
SELECT pg_tviews_debug_queue_items();
SELECT pg_tviews_debug_worker_status();
```

#### Proposed Solution (v2.0)
Remove these functions entirely. Debug information available through proper monitoring interfaces.

#### Migration Path
```sql
-- v1.5: Add deprecation warnings
SELECT pg_tviews_debug_queue_status(); -- WARNING: This function is deprecated

-- v2.0: Functions removed
-- Use proper monitoring/logging instead
```

#### Impact Assessment
- **User Impact**: LOW (debug functions, not production use)
- **Migration Effort**: Very Low (remove debug code)
- **Business Value**: Low (cleanup experimental features)

---

### 5. Schema Reorganization: Internal Schema (TRANSPARENT)

#### Current Problem
Internal tables scattered across public schema with generic names.

#### Proposed Solution (v2.0)
Move to dedicated `pg_tviews` schema with clear naming.

#### Migration Path
Automatic migration during upgrade. No user action required.

#### Impact Assessment
- **User Impact**: NONE (transparent)
- **Migration Effort**: Low (automatic)
- **Business Value**: Medium (better organization)

---

### 6. Schema Enhancement: Metadata Structure (TRANSPARENT)

#### Current Problem
Metadata structure optimized for current features.

#### Proposed Solution (v2.0)
Enhanced metadata to support future features.

#### Migration Path
Automatic migration during upgrade. No user action required.

#### Impact Assessment
- **User Impact**: NONE (transparent)
- **Migration Effort**: Low (automatic)
- **Business Value**: Medium (future-proofing)

---

### 7. Behavior Enhancement: Configurable Refresh Policies (OPTIONAL)

#### Current Problem
Fixed refresh behavior.

#### Proposed Solution (v2.0)
Configurable refresh policies with backward compatibility.

#### Migration Path
New optional parameters. Existing code continues to work.

#### Impact Assessment
- **User Impact**: MEDIUM (optional enhancement)
- **Migration Effort**: Medium (opt-in feature)
- **Business Value**: High (flexibility)

---

## Rollback Procedures

### For v2.0 Upgrade Issues

1. **Immediate Rollback** (within 1 hour):
   ```bash
   # Downgrade binary
   pg_ctl stop
   # Replace with v1.x binary
   pg_ctl start
   ```

2. **Data Migration Rollback** (within 24 hours):
   ```sql
   -- Restore from backup taken before upgrade
   -- Schema changes are backward compatible for 24 hours
   ```

3. **Extended Support** (up to 30 days):
   - v1.x remains available for download
   - Community support for rollback issues
   - Enterprise support contracts include rollback assistance

### Testing Recommendations

- Test upgrade in staging environment first
- Have recent backup available
- Plan rollback time windows
- Monitor error logs during upgrade

---

## User Communication Plan

### Phase 1: Announcement (December 2025)
- [ ] Release this breaking changes document
- [ ] Blog post: "Planning for pg_tviews v2.0"
- [ ] GitHub announcement
- [ ] Community forum post

### Phase 2: Deprecation Warnings (v1.5, October 2026)
- [ ] Add deprecation warnings to affected functions
- [ ] Update documentation with migration guides
- [ ] Release notes highlight upcoming changes

### Phase 3: Migration Support (2027)
- [ ] Migration tooling release
- [ ] Webinar: "Migrating to pg_tviews v2.0"
- [ ] Office hours for migration questions

### Phase 4: v2.0 Release (April 2028)
- [ ] Comprehensive release notes
- [ ] Migration success stories
- [ ] Enterprise migration support

---

## FAQ

### General Questions

**Q: Why breaking changes in v2.0?**
A: v1.0 commits to API stability. v2.0 allows us to fix fundamental design issues and simplify the API for better long-term maintainability.

**Q: How long do I have to migrate?**
A: 24 months from announcement (December 2025) to v2.0 release (April 2028).

**Q: What if I can't migrate by April 2028?**
A: v1.x will receive security patches until April 2029, giving you an additional 12 months.

### Technical Questions

**Q: Will my existing code break immediately?**
A: No. Deprecation warnings start in v1.5 (October 2026). Breaking changes only occur in v2.0.

**Q: Can I upgrade to v2.0 without changing code?**
A: No. The API changes are significant and require code updates. However, migration is straightforward.

**Q: What if I have enterprise change control processes?**
A: The 24-month timeline is designed to accommodate enterprise planning cycles. Contact enterprise support for extended timelines if needed.

**Q: Are there tools to help with migration?**
A: Yes, we'll provide migration tools and detailed guides. Community support available for complex cases.

### Support Questions

**Q: Where can I get help with migration?**
A: Documentation, GitHub Discussions, and enterprise support contracts.

**Q: What if I encounter issues during migration?**
A: Community forums for general help, GitHub Issues for bugs, enterprise support for priority assistance.

**Q: Can I get professional migration services?**
A: Enterprise support includes migration assistance and custom tooling.

---

## Getting Help

For migration questions:
- **Documentation**: [docs/migration/v2.0-upgrade-guide.md](docs/migration/v2.0-upgrade-guide.md)
- **Community forums**: [GitHub Discussions](https://github.com/your-org/pg_tviews/discussions)
- **Issues**: [GitHub Issues](https://github.com/your-org/pg_tviews/issues)
- **Enterprise support**: Contact sales for migration assistance

## Decision Rationale (ADR)

See [docs/adr/2025-v2-breaking-changes.md](docs/adr/2025-v2-breaking-changes.md) for detailed decision rationale and feasibility assessment.</content>
<parameter name="filePath">docs/BREAKING_CHANGES_V2.0.md