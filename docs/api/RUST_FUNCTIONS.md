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

## Module: error (STABLE)

### TViewError
**Status**: STABLE
**Description**: Error type for all pg_tviews operations
**Variants** (as of v0.1.0-beta.1):
- MetadataNotFound
- RefreshFailed
- CacheMiss
- SerializationFailed

---

## Module: queue (EVOLVING)

### RefreshKey
**Status**: EVOLVING
**Description**: Key for identifying rows to refresh
**Warning**: Structure may change as queue optimization evolves

---

## Module: catalog (EVOLVING)

**Status**: EVOLVING
**Warning**: Internal catalog representation may change
**Known Future Changes**:
- Cache invalidation strategy
- Metadata storage format
- Query performance optimizations

---

## Module: schema (EVOLVING)

**Status**: EVOLVING
**Description**: Schema analysis and validation
**Warning**: API may change as schema understanding evolves

---

## Module: config (EXPERIMENTAL)

**Status**: EXPERIMENTAL
**Description**: Configuration management
**Warning**: Configuration options may change frequently

---

## Function: check_jsonb_ivm_available() -> bool

**Status**: STABLE
**Description**: Check if jsonb_ivm extension is available
**Returns**: true if jsonb_ivm can be used for optimization

---

## Constant: VERSION

**Status**: STABLE
**Description**: Extension version string
**Value**: Current Cargo.toml version