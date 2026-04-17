# Phase 1: SQL Parameterization

## Objective

Eliminate all string-interpolated SQL values and replace them with parameterized
queries (`$N` placeholders + `DatumWithOid` args), bringing every SPI call into
alignment with the secure pattern used in the rest of the codebase.

## Why This Matters

While the interpolated values (OIDs, i64 PKs) are not user-controlled and not
exploitable today, the inconsistency makes it harder to audit for real injection
bugs. A single pattern everywhere means a reviewer can grep for `format!` in SQL
strings and flag any remaining instances as suspicious.

## Success Criteria

- [ ] Zero instances of integer/OID interpolation in SPI query strings
- [ ] `cargo clippy --no-default-features --features pg18` clean
- [ ] `cargo test` passes
- [ ] All existing `#[pg_test]` functions still compile

## Required Imports

All parameterized SPI calls use the same pattern already established in the codebase
(see `src/ddl/convert.rs` for reference). The key imports are:

```rust
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;  // includes PgOid, PgBuiltInOids

// OID constant construction:
PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()   // for pg_sys::Oid values
PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()   // for i64 values

// SPI call patterns:
Spi::get_one_with_args::<T>(sql, &[unsafe { DatumWithOid::new(val, oid) }])
Spi::run_with_args(sql, &[unsafe { DatumWithOid::new(val, oid) }])
```

## Files To Change

> **Note**: `src/cascade.rs` also has interpolated SQL values but is being deleted
> entirely in Phase 4. No point parameterizing code that will be removed.

### 1. `src/catalog.rs`

**Line 411-413** — `entity_for_table_uncached`:
```rust
// BEFORE:
crate::utils::spi_get_string(&format!(
    "SELECT relname::text FROM pg_class WHERE oid = {table_oid:?}"
))

// AFTER: Replace spi_get_string with Spi::get_one_with_args (inline, no helper needed):
let table_name: String = Spi::get_one_with_args(
    "SELECT relname::text FROM pg_class WHERE oid = $1",
    &[unsafe {
        DatumWithOid::new(
            table_oid,
            PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
        )
    }],
)?
.ok_or_else(|| /* existing error handling pattern */)?;
```

**Line 429-431** — Same function, second query already uses `Spi::get_one_with_args`
with `$1` — this one is fine, no change needed.

### 2. `src/admin.rs`

**Line 100-106** — `get_view_columns_by_oid`:
```rust
// BEFORE:
let rows = client.select(
    &format!(
        "SELECT attname::text FROM pg_attribute \
         WHERE attrelid = {rel_oid:?} AND attnum > 0 AND NOT attisdropped \
         ORDER BY attnum"
    ),
    None,
    &[],
)?;

// AFTER: Use $1 parameter with OIDOID:
let args = vec![unsafe {
    DatumWithOid::new(
        rel_oid,
        PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
    )
}];
let rows = client.select(
    "SELECT attname::text FROM pg_attribute \
     WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped \
     ORDER BY attnum",
    None,
    &args,
)?;
```

### 3. `src/refresh/bulk.rs`

**Line 146-148** — `relname_from_oid` (local duplicate):
```rust
// BEFORE:
crate::utils::spi_get_string(&format!(
    "SELECT relname::text FROM pg_class WHERE oid = {oid:?}"
))

// AFTER: Delete this function entirely. Replace the single call site (line 80)
// with crate::utils::relname_from_oid(meta.tview_oid)?
// This also addresses the Phase 2 duplication issue early.
```

### 4. `src/refresh/main.rs`

**Line 668** — `apply_full_replacement`:
```rust
// BEFORE (line 668):
format!("SELECT {col_list} FROM {view_name} WHERE {pk_col} = {}", row.pk)
// called with Spi::run(&sql)

// AFTER:
let sql = format!("SELECT {col_list} FROM {view_name} WHERE {pk_col} = $1");
Spi::run_with_args(
    &sql,
    &[unsafe {
        DatumWithOid::new(
            row.pk,
            PgOid::BuiltIn(PgBuiltInOids::INT8OID).value(),
        )
    }],
)?;
```

Add `use pgrx::datum::DatumWithOid;` to the file's imports if not already present.

## TDD Cycles

### Cycle 1: catalog.rs + admin.rs parameterization

- **RED**: Existing unit tests in `catalog.rs::tests` pass
- **GREEN**: Parameterize `entity_for_table_uncached` OID query and
  `get_view_columns_by_oid` in admin.rs
- **REFACTOR**: No helper needed — use `Spi::get_one_with_args` inline for catalog.rs
  and `client.select` with args for admin.rs. Both are one-off call sites.
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 2: refresh/bulk.rs + refresh/main.rs parameterization

- **RED**: `test_refresh_bulk_empty` passes
- **GREEN**: Delete `bulk.rs::relname_from_oid`, redirect to `utils::relname_from_oid`.
  Parameterize `apply_full_replacement` PK value.
- **REFACTOR**: None needed
- **CLEANUP**: `cargo clippy`, `cargo test`

## Verification

After all cycles:
```bash
cargo clippy --no-default-features --features pg18 -- -D warnings
cargo test
# Grep for remaining interpolated OIDs/PKs in SQL strings:
grep -rn 'format!.*oid:?\|format!.*base_pk\|format!.*row\.pk' src/ --include='*.rs' | grep -v cascade.rs
# Should return zero results (cascade.rs excluded — deleted in Phase 4)
```

## Dependencies

- Requires: nothing (first phase)
- Blocks: Phase 2 (bulk.rs duplication partially resolved here)

## Status
[ ] Not Started
