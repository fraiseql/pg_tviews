# Phase 4: Trigger Helper Extraction

## Objective

Remove the repeated per-table preamble shared between `install_triggers` and
`remove_triggers` in `src/dependency/triggers.rs`.

## Background

Both functions iterate over `table_oids` and open each iteration with
identical setup:

```rust
let table_name = get_table_name(table_oid)?;
let qi_table = quote_identifier(&table_name);
let trigger_name = format!("trg_tview_{entity}_on_{table_name}");
let flush_trigger_name = format!("trg_tview_flush_{entity}_on_{table_name}");
```

These four lines are verbatim duplicates. If the naming convention for
triggers ever changes, both functions must be updated in sync.

## Success Criteria

- [ ] A private struct or tuple returned by a helper encapsulates the four
      computed names/identifiers for a single table
- [ ] Both `install_triggers` and `remove_triggers` call the helper instead
      of repeating the construction
- [ ] No behaviour change; all trigger names remain identical
- [ ] Clippy clean; tests pass

## TDD Cycles

### Cycle 1: Extract `TriggerTarget` (GREEN → REFACTOR)

No new behaviour; no RED phase.

**GREEN**

Introduce a small private struct and constructor:

```rust
/// Computed names for the two triggers managed per base table.
struct TriggerTarget {
    /// Quoted table identifier: `"tb_foo"`
    qi_table: String,
    /// Row-level trigger name (unquoted): `trg_tview_{entity}_on_{table}`
    row_trigger: String,
    /// Statement-level flush trigger name: `trg_tview_flush_{entity}_on_{table}`
    flush_trigger: String,
}

impl TriggerTarget {
    fn for_table(table_oid: pg_sys::Oid, entity: &str) -> TViewResult<Self> {
        let table_name = get_table_name(table_oid)?;
        Ok(Self {
            qi_table: quote_identifier(&table_name),
            row_trigger: format!("trg_tview_{entity}_on_{table_name}"),
            flush_trigger: format!("trg_tview_flush_{entity}_on_{table_name}"),
        })
    }
}
```

**REFACTOR**

Rewrite the loop bodies:

```rust
// install_triggers
for &table_oid in table_oids {
    let t = TriggerTarget::for_table(table_oid, tview_entity)?;
    if trigger_exists(&t.qi_table, &t.row_trigger)? { ... continue; }
    // CREATE TRIGGER {quote_identifier(&t.row_trigger)} ... ON {t.qi_table} ...
    if !trigger_exists(&t.qi_table, &t.flush_trigger)? {
        // CREATE TRIGGER {quote_identifier(&t.flush_trigger)} ...
    }
}

// remove_triggers
for &table_oid in table_oids {
    let t = TriggerTarget::for_table(table_oid, tview_entity)?;
    // DROP TRIGGER IF EXISTS {quote_identifier(&t.row_trigger)} ON {t.qi_table}
    // DROP TRIGGER IF EXISTS {quote_identifier(&t.flush_trigger)} ON {t.qi_table}
}
```

Note: `trigger_exists` currently takes `(&table_name, &trigger_name)` — check
its signature and adjust the call site to use `t` fields consistently.

**CLEANUP**

- Confirm no spurious warnings about `qi_table` in `install_triggers` (it was
  used in trigger SQL; make sure the field reference is correct after refactor)
- Run `cargo clippy --no-default-features --features pg18 -- -D warnings`
- Run `cargo test`
- Commit: `refactor(triggers): extract TriggerTarget to deduplicate name construction`

## Files Touched

- `src/dependency/triggers.rs` — only file changed

## Risk

Low. Pure name/string construction with no logic change. Trigger names are
determined at compile time by the format strings, which are unchanged.
