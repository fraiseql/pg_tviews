# Phase: Security Hardening

## Objective

Eliminate all remaining SQL injection vectors by replacing quote-doubling with parameterized queries, and add input validation at system boundaries.

## Current State (2026-03-19)

Commit 72ef7db (2026-03-03) parameterized several hot spots in `catalog.rs` and `audit.rs`, but **9+ locations** in other modules still use `format!()` with `replace('\'', "''")`  — a weaker defense that breaks under edge cases (backslash escaping, alternate encodings).

### Remaining Vulnerable Locations

| File | Function | Line(s) | Input |
|------|----------|---------|-------|
| `src/ddl/create.rs` | `tview_exists()` | ~131 | entity_name |
| `src/ddl/create.rs` | metadata INSERT | ~400 | entity_name, definition |
| `src/ddl/convert.rs` | `get_table_columns()` | ~127 | table_name |
| `src/ddl/convert.rs` | `table_exists()` | ~291 | table_name |
| `src/ddl/convert.rs` | `get_base_table_hints()` | ~301 | table_name |
| `src/ddl/convert.rs` | metadata INSERT | ~400 | entity_name, definition |
| `src/dependency/triggers.rs` | `trigger_exists()` | ~203 | table_name, trigger_name |
| `src/dependency/graph.rs` | `get_view_oid()` | varies | view_name |
| `src/refresh/main.rs` | column enumeration | varies | view_name |
| `src/propagate.rs` | parent PK lookup | ~99 | child_pk (integer, but still interpolated) |

---

## TDD Cycles

### Cycle 1: Parameterize DDL module queries

**RED**: Write tests that call `tview_exists()`, `get_table_columns()`, `table_exists()`, `get_base_table_hints()` with adversarial entity/table names containing `'`, `''`, `\`, `--`, and `;`. Verify no SQL error or unexpected behavior.

**GREEN**: Replace all `format!()` + `replace('\'', "''")` patterns in `src/ddl/create.rs` and `src/ddl/convert.rs` with `Spi::get_one_with_args()` or `client.select()` using `$1`, `$2` placeholders.

Example transformation:
```rust
// BEFORE
Spi::get_one::<bool>(&format!(
    "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = '{}'",
    entity_name.replace('\'', "''")
))

// AFTER
Spi::get_one_with_args::<bool>(
    "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = $1",
    vec![(PgBuiltInOids::TEXTOID.oid(), entity_name.into_datum())],
)
```

**REFACTOR**: Extract a helper for common `EXISTS`-check pattern if used in 3+ places.
**CLEANUP**: Remove all `replace('\'', "''")` calls from DDL modules. Verify clippy clean.

### Cycle 2: Parameterize dependency and graph modules

**RED**: Test `trigger_exists()` and `get_view_oid()` with adversarial trigger/view names.

**GREEN**: Convert `src/dependency/triggers.rs` and `src/dependency/graph.rs` to parameterized queries. For `regclass` casts, use `$1::regclass` with the name as a parameter.

**REFACTOR**: Consolidate OID lookup patterns (`pg_class` queries) into a shared catalog helper.
**CLEANUP**: Verify clippy clean.

### Cycle 3: Parameterize propagation and refresh queries

**RED**: Test `find_parents_for()` and column enumeration with edge-case inputs.

**GREEN**:
- `src/propagate.rs:99` — parameterize `child_pk` as `$1` (it's an integer, but still should not be interpolated)
- `src/refresh/main.rs` — parameterize view_name in `pg_attribute` queries

**REFACTOR**: For identifier parameters (table/column names which can't be `$1`), ensure all use `quote_identifier()` from pgrx.
**CLEANUP**: Grep entire codebase for remaining `replace('\'', "''")` — target: zero occurrences.

### Cycle 4: Input validation at system boundaries

**RED**: Test that `pg_tviews_create()`, `pg_tviews_drop()`, and other SQL-callable functions reject:
- Empty entity names
- Names with null bytes
- Names exceeding 63 chars (PostgreSQL NAMEDATALEN)
- Names with SQL metacharacters

**GREEN**: Add a `validate_entity_name()` function (similar to existing `validate_gid()`) that enforces `^[a-z_][a-z0-9_]*$` pattern and max length. Call it at entry points.

**REFACTOR**: Unify `validate_gid()` and `validate_entity_name()` under a common validation module if patterns overlap.
**CLEANUP**: Verify all public SQL-callable functions have validation.

### Cycle 5: Verify with automated scan

**GREEN**: Add a `grep`-based CI check that fails if `replace('\'', "''")` appears anywhere in `src/`:
```yaml
- name: Check for quote-doubling anti-pattern
  run: |
    if grep -rn "replace.*'\\\\''.*''" src/; then
      echo "::error::Found quote-doubling pattern. Use parameterized queries."
      exit 1
    fi
```

**CLEANUP**: Add this check to `clippy.yml` or as a standalone workflow step.

---

## Dependencies

- Requires: Phase CI Health Cycle 1 (pgrx pin) for CI to validate changes
- Blocks: Production release

## Status
[x] Cycle 1: Parameterize DDL module queries (create.rs, convert.rs)
[x] Cycle 2: Parameterize dependency and graph modules (graph.rs)
[x] Cycle 3: Parameterize propagation and refresh queries (propagate.rs, refresh/main.rs)
[x] Cycle 4: Input validation at system boundaries (ddl/mod.rs entry points + validation.rs fixed)
[x] Cycle 5: CI check for quote-doubling anti-pattern (clippy.yml)
