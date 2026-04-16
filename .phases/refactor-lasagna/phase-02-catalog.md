# Phase 2: Catalog Loader Consolidation

## Objective

Eliminate the three-way repetition of the SPI SELECT in `src/catalog.rs`
across `load_for_source`, `load_by_entity`, and `load_for_tview`.

## Background

All three methods:
1. Open `Spi::connect`
2. Build a single-element `args` vec
3. Execute the **same 10-column SELECT** from `pg_tview_meta` with only the
   WHERE clause differing
4. Call `from_spi_row` on the first result
5. Return `spi::Result<Option<Self>>`

The repeated SELECT is:
```sql
SELECT table_oid AS tview_oid, view_oid, entity,
       fk_columns, uuid_fk_columns,
       dependency_types, dependency_paths, array_match_keys,
       distinct_on_keys, is_union
FROM pg_tview_meta
WHERE <condition>
```

If the column list ever changes (e.g. a new metadata field is added), all
three sites must be updated in sync.

## Success Criteria

- [ ] A single private helper `load_one` contains the SPI boilerplate
- [ ] Each public `load_*` method is reduced to argument construction + a call to `load_one`
- [ ] No change to any public method signatures
- [ ] All tests pass
- [ ] Clippy clean

## TDD Cycles

### Cycle 1: Introduce `load_one` helper (GREEN → REFACTOR)

No new behaviour, so no RED phase needed — existing integration tests cover
correctness.

**GREEN**

Add a private associated function:

```rust
/// Internal loader: executes `SELECT … FROM pg_tview_meta WHERE <where_clause>`
/// with the provided arguments and returns the first row, if any.
///
/// `where_clause` must be a static string (caller controls it — never user input).
fn load_one(where_clause: &'static str, args: Vec<DatumWithOid>) -> spi::Result<Option<Self>> {
    Spi::connect(|client| {
        let mut rows = client.select(
            &format!(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys, \
                        distinct_on_keys, is_union \
                 FROM pg_tview_meta \
                 WHERE {where_clause}"
            ),
            None,
            &args,
        )?;
        match rows.next() {
            Some(row) => Ok(Some(Self::from_spi_row(&row)?)),
            None => Ok(None),
        }
    })
}
```

Note: `where_clause` is `&'static str` — all call sites use a string literal,
so there is no SQL injection risk. The format call only concatenates a
compile-time constant.

**REFACTOR**

Rewrite the three public methods as:

```rust
pub fn load_for_source(source_oid: Oid) -> spi::Result<Option<Self>> {
    let args = vec![unsafe { DatumWithOid::new(source_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
    Self::load_one("view_oid = $1 OR table_oid = $1", args)
}

pub fn load_by_entity(entity_name: &str) -> spi::Result<Option<Self>> {
    let args = vec![unsafe { DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
    Self::load_one("entity = $1", args)
}

pub fn load_for_tview(tview_oid: Oid) -> spi::Result<Option<Self>> {
    let args = vec![unsafe { DatumWithOid::new(tview_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
    Self::load_one("table_oid = $1", args)
}
```

**CLEANUP**

- Remove the verbose doc comment block from `load_for_tview` (it explains the
  function via its signature; a one-liner suffices now)
- Run `cargo clippy` and `cargo test`
- Commit: `refactor(catalog): consolidate three load_* methods into load_one helper`

## Files Touched

- `src/catalog.rs` — only file changed

## Risk

Low. Purely mechanical extraction of repeated SPI boilerplate. The SQL
strings are unchanged; only their location moves. All callers remain
identical.
