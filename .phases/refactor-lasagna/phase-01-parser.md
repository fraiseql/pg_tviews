# Phase 1: Parser Unification

## Objective

Eliminate the near-total duplication between `extract_columns_regex` and
`extract_columns_with_expressions_regex` in `src/schema/parser.rs`.

## Background

`extract_columns_regex` (lines 26–88) and
`extract_columns_with_expressions_regex` (lines 529–588) are byte-for-byte
identical except for their final push:

```rust
// extract_columns_regex
columns.push(col_name);

// extract_columns_with_expressions_regex
columns.push((col_name, trimmed.to_string()));
```

All other logic — CTE skipping, SELECT/FROM detection, UNION bounding,
DISTINCT ON skipping, top-level comma splitting, `extract_column_name` call —
is duplicated across ~60 lines.

## Success Criteria

- [ ] A single private function `extract_select_columns_inner` handles the
      shared logic and returns `Vec<(String, String)>` (name, full expression)
- [ ] `extract_columns_regex` calls it and discards the expression
- [ ] `extract_columns_with_expressions_regex` calls it unchanged
- [ ] All existing parser tests pass
- [ ] No public API changes

## TDD Cycles

### Cycle 1: Introduce inner function (RED → GREEN)

**RED**

Add a new test that calls both public functions on the same SQL and asserts
that the names returned by `parse_select_columns` equal the `.0` fields
returned by `parse_select_columns_with_expressions`.  This test is trivially
green today — make it explicit so any regression is caught immediately.

```rust
#[test]
fn test_column_names_consistent_between_parsers() {
    let sql = "SELECT u.id, u.name AS full_name, ... FROM tb_user u";
    let names = parse_select_columns(sql).unwrap();
    let with_expr = parse_select_columns_with_expressions(sql).unwrap();
    let names_from_expr: Vec<String> = with_expr.into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, names_from_expr);
}
```

Verify it passes before touching any implementation code.

**GREEN**

Extract the shared body into:

```rust
/// Core SELECT-column extraction.  Returns (alias_or_name, full_expression) pairs.
/// Both public entry points delegate here.
fn extract_select_columns_inner(sql: &str) -> Result<Vec<(String, String)>, String> {
    // ... everything from extract_columns_regex, but push (col_name, trimmed.to_string())
}
```

Rewrite the two existing private functions as one-liners:

```rust
fn extract_columns_regex(sql: &str) -> Result<Vec<String>, String> {
    extract_select_columns_inner(sql).map(|v| v.into_iter().map(|(n, _)| n).collect())
}

fn extract_columns_with_expressions_regex(sql: &str) -> Result<Vec<(String, String)>, String> {
    extract_select_columns_inner(sql)
}
```

**REFACTOR**

Since `extract_columns_with_expressions_regex` is now a direct pass-through,
inline it into its only caller `parse_select_columns_with_expressions` and
delete the intermediate function.

Final call graph:
```
parse_select_columns               → extract_select_columns_inner → (drop expressions)
parse_select_columns_with_expressions → extract_select_columns_inner
```

**CLEANUP**

- Delete `extract_columns_with_expressions_regex` (now inlined)
- Run `cargo clippy --no-default-features --features pg18 -- -D warnings`
- Run `cargo test`
- Commit: `refactor(parser): unify column extraction into single inner function`

## Files Touched

- `src/schema/parser.rs` — only file changed

## Risk

Low. The parser has strong unit test coverage including CTE, DISTINCT ON,
UNION, nested subqueries, and alias edge cases. The inner function body is a
copy-then-tweak, so correctness is maintained automatically.
