# Phase 2: Consolidate Duplicate Functions

## Objective

Merge the duplicate implementations of `relname_from_oid` and `get_view_columns`
into single, cached, canonical versions. Eliminate code that reimplements what
already exists elsewhere.

## Why This Matters

Three separate implementations of "get column names for a view" and two of
"get relname from OID" means bugs fixed in one copy don't propagate to the others.
The uncached copies also skip the `OID_RELNAME_CACHE` and `VIEW_COLUMNS_CACHE`,
causing unnecessary SPI round-trips on repeated calls.

## Success Criteria

- [ ] Exactly ONE `relname_from_oid` function exists (in `utils.rs`, cached)
- [ ] Exactly ONE `get_view_columns` function exists (cached), callable by OID or name
- [ ] `cargo clippy --no-default-features --features pg18` clean
- [ ] `cargo test` passes

## Current State — What Exists

### `relname_from_oid`

| Location | Cached | Parameterized | Notes |
|----------|--------|---------------|-------|
| `src/utils.rs:213` | Yes (`OID_RELNAME_CACHE`) | Yes (`$1` + OIDOID) | Canonical |
| `src/refresh/bulk.rs:145` | No | No (format interpolation) | Duplicate — should be deleted in Phase 1 |

If Phase 1 already deleted `bulk.rs::relname_from_oid`, this is done. Verify only.

### `get_view_columns` (column name lists)

| Location | Cached | Lookup key | Notes |
|----------|--------|------------|-------|
| `src/refresh/main.rs:232` `get_view_columns(&str)` | Yes (`VIEW_COLUMNS_CACHE`) | view name (text) | Used by dedup key refresh |
| `src/admin.rs:97` `get_view_columns_by_oid(Oid)` | No | relation OID | Used by `pg_tviews_refresh` |
| `src/refresh/main.rs:621` inline in `apply_full_replacement` | No | view name (text) | Hardcoded SPI block |

## Files To Change

### 1. `src/utils.rs` — Add canonical `get_view_columns` function

Move `get_view_columns` from `refresh/main.rs:232-273` into `utils.rs` as a
public function. The `VIEW_COLUMNS_CACHE` static is already defined in `utils.rs`
(line 185), and the function in `refresh/main.rs` accesses it via
`crate::utils::VIEW_COLUMNS_CACHE`. After the move, the function lives next to the
cache it uses, and external access becomes `crate::utils::get_view_columns()`.

```rust
/// Get column names for a view/table by name. Results are cached per session.
pub fn get_view_columns(relation_name: &str) -> spi::Result<Vec<String>> {
    // (move existing implementation from refresh/main.rs)
}
```

Also add a by-OID variant that resolves the name first, then delegates:

```rust
/// Get column names for a relation by OID. Resolves name via relname_from_oid,
/// then delegates to get_view_columns for caching.
pub fn get_view_columns_by_oid(rel_oid: Oid) -> spi::Result<Vec<String>> {
    let name = relname_from_oid(rel_oid)?;
    get_view_columns(&name)
}
```

### 2. `src/refresh/main.rs` — Remove local `get_view_columns`

- Delete the `get_view_columns` function (lines 232-273)
- Replace call at line 193: `let col_names = get_view_columns(&view_name)?;`
  with: `let col_names = crate::utils::get_view_columns(&view_name)?;`
- Replace the inline SPI block in `apply_full_replacement` (lines 621-645)
  with: `let col_names = crate::utils::get_view_columns(&view_name)?;`

### 3. `src/admin.rs` — Remove local `get_view_columns_by_oid`

- Delete `get_view_columns_by_oid` function (lines 97-118)
- Replace call at line 69:
  `let view_columns = get_view_columns_by_oid(meta.view_oid)?;`
  with: `let view_columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;`

### 4. `src/refresh/bulk.rs` — Verify `relname_from_oid` already removed

If Phase 1 deleted the local `relname_from_oid` and redirected to
`crate::utils::relname_from_oid`, just verify. Otherwise do it now.

## TDD Cycles

### Cycle 1: Move `get_view_columns` to utils.rs

- **RED**: Existing unit tests pass, `get_view_columns` is private in refresh/main.rs
- **GREEN**: Copy function to `utils.rs` as `pub fn get_view_columns`. Add
  `pub fn get_view_columns_by_oid`. Update call sites.
- **REFACTOR**: Remove the old definitions from `refresh/main.rs` and `admin.rs`.
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 2: Eliminate inline SPI in `apply_full_replacement`

- **RED**: The `#[pg_test] test_fallback_without_jsonb_delta` exercises this path
- **GREEN**: Replace the inline pg_attribute query in `apply_full_replacement`
  (lines 621-645) with `crate::utils::get_view_columns(&view_name)?`
- **REFACTOR**: The `apply_full_replacement` function is now much shorter.
  Consider if `apply_patch` can also be simplified (it calls `relname_from_oid`
  then passes to `apply_full_replacement` which calls it again — eliminate the
  double lookup by passing `tv_name` as a parameter).
- **CLEANUP**: `cargo clippy`, `cargo test`

## Verification

```bash
cargo clippy --no-default-features --features pg18 -- -D warnings
cargo test
# Verify no duplicates remain:
grep -rn 'fn relname_from_oid\|fn get_view_columns' src/ --include='*.rs'
# Should show exactly:
#   src/utils.rs: pub fn relname_from_oid
#   src/utils.rs: pub fn get_view_columns
#   src/utils.rs: pub fn get_view_columns_by_oid
```

## Dependencies

- Requires: Phase 1 (bulk.rs relname_from_oid deletion)
- Blocks: Phase 3 (audit batching will use the canonical helpers)

## Status
[ ] Not Started
