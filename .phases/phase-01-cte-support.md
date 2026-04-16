# Phase 1: CTE (`WITH` clause) Support — Issue #41

## Objective

Allow TVIEW SELECT statements to use Common Table Expressions so that
multi-step queries (locale resolution, intermediate aggregations, filtered
joins) can be expressed idiomatically.

## Success Criteria

- [ ] `CREATE TABLE tv_item AS WITH ... SELECT ...` completes successfully
- [ ] Column extraction works on the **main** SELECT (not CTE body SELECTs)
- [ ] Schema inference (pk_, id, data, fk_*, *_id) works unchanged
- [ ] Dependency analysis detects base tables inside CTE bodies via pg_depend
- [ ] Backing view `v_item` stores the full SQL including WITH clause
- [ ] Incremental refresh works: change in CTE-referenced table triggers refresh
- [ ] `WITH RECURSIVE` is rejected with a clear error message
- [ ] Existing non-CTE TVIEWs are unaffected (regression suite green)

## Analysis

### What already works

- **Backing view creation** (`ddl/create.rs:create_backing_view`): executes
  `CREATE VIEW v_entity AS {select_sql}` — PostgreSQL handles CTEs natively.
- **Dependency discovery** (`dependency/graph.rs:find_base_tables`): traverses
  `pg_rewrite` + `pg_depend` on the view OID — finds tables in CTE bodies
  automatically.
- **Trigger installation** (`dependency/triggers.rs`): installs on OIDs from
  dependency graph — no SQL parsing involved.
- **Refresh logic** (`refresh/main.rs`): re-executes `SELECT * FROM v_entity
  WHERE pk_entity = $1` — the CTE is embedded in the view definition.
- **Dependency type analysis** (`schema/analyzer.rs`): regex-scans the full
  SQL text for patterns like `'key', v_view.data` — works even if the pattern
  is in the main SELECT after the CTE.

### What breaks

1. **`parser.rs:extract_columns_regex`** (line 26): calls
   `sql_lower.find("select")` which matches the **first** SELECT — inside the
   first CTE body, not the main query. Column extraction fails or returns CTE
   columns instead of TVIEW columns.

2. **`parser.rs:extract_columns_with_expressions_regex`** (line 85): same
   problem — finds wrong SELECT/FROM pair.

3. **`hooks.rs:validate_tview_select`** (line 94): scans `sql_lower` for `id`
   and `data` patterns. This is a loose text scan — likely still passes for
   CTE SQL, but should be verified.

### Fix strategy

Add a **CTE-stripping preamble** to the parser entry points. Before looking
for SELECT/FROM, detect a leading `WITH` clause and advance past the entire
CTE list to the main SELECT. The full SQL is preserved for view creation and
metadata; only the parser's column extraction window shifts.

Algorithm to skip CTEs:
```
1. Trim + lowercase, check starts_with("with")
2. If followed by "recursive" → reject with error
3. Walk forward, tracking paren depth:
   - Each CTE: identifier + optional column list + "AS" + "(" body ")"
   - After closing ")", expect "," (another CTE) or main SELECT
4. Return byte offset of main SELECT in original SQL
```

This is robust because CTE bodies are always wrapped in parentheses — we just
need to balance parens, respecting string literals and quoted identifiers.

## TDD Cycles

### Cycle 1: CTE detection and main-SELECT offset

- **RED**: Unit tests in `parser.rs` — both `parse_select_columns()` and
  `parse_select_columns_with_expressions()` on a CTE query return columns from
  the main SELECT, not the CTE body. Currently both fail (same `find("select")`
  bug at lines 33 and 113 respectively).
  ```rust
  #[test]
  fn test_parse_cte_columns() {
      let sql = "WITH labels AS (SELECT item_id, label FROM tb_i18n) \
                  SELECT i.pk_item, i.id, i.name, l.label AS data \
                  FROM tb_item i LEFT JOIN labels l ON l.item_id = i.pk_item";
      let cols = parse_select_columns(sql).unwrap();
      assert!(cols.contains(&"pk_item".to_string()));
      assert!(cols.contains(&"id".to_string()));
      assert!(!cols.contains(&"item_id".to_string())); // CTE column
  }

  #[test]
  fn test_parse_cte_columns_with_expressions() {
      let sql = "WITH labels AS (SELECT item_id, label FROM tb_i18n) \
                  SELECT i.pk_item, i.id, i.name, l.label AS data \
                  FROM tb_item i LEFT JOIN labels l ON l.item_id = i.pk_item";
      let cols = parse_select_columns_with_expressions(sql).unwrap();
      let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
      assert!(names.contains(&"pk_item"));
      assert!(names.contains(&"data"));
      assert!(!names.contains(&"item_id")); // CTE column
  }
  ```
- **GREEN**: Implement `skip_cte_preamble(sql) -> usize` that returns the byte
  offset of the main SELECT. Call it at the top of `extract_columns_regex` and
  `extract_columns_with_expressions_regex` to adjust the search window.
- **REFACTOR**: Extract the paren-depth walker into a shared utility (it's
  already partially implemented in `split_by_top_level_comma`).
- **CLEANUP**: Lint, format, commit.

### Cycle 2: Multi-CTE and edge cases

- **RED**: Tests for:
  - Multiple CTEs: `WITH a AS (...), b AS (... FROM a ...) SELECT ...`
  - CTE with explicit column list: `WITH a(x, y) AS (...) SELECT ...`
  - CTE containing string literals with `)` inside: `WHERE name = 'a)'`
  - CTE containing nested subqueries: `WITH a AS (SELECT (SELECT 1)) ...`
- **GREEN**: Harden `skip_cte_preamble` to handle these cases.
- **REFACTOR**: Ensure the paren-depth walker handles single-quoted and
  double-quoted strings (skip content inside quotes).
- **CLEANUP**: Lint, format, commit.

### Cycle 3: WITH RECURSIVE rejection

- **RED**: Test that `WITH RECURSIVE` returns a clear error:
  ```rust
  #[test]
  fn test_recursive_cte_rejected() {
      let sql = "WITH RECURSIVE tree AS (...) SELECT ...";
      let result = parse_select_columns(sql);
      assert!(result.is_err());
      assert!(result.unwrap_err().contains("RECURSIVE"));
  }
  ```
- **GREEN**: In `skip_cte_preamble`, after detecting `WITH`, check for
  `RECURSIVE` keyword and return error.
- **REFACTOR**: Error message should suggest alternatives.
- **CLEANUP**: Lint, format, commit.

### Cycle 4: Schema inference with CTE SQL

- **RED**: Test `infer_schema()` with a CTE query — verify entity name,
  column categorization, type inference all work on main SELECT columns.
- **GREEN**: Should pass if Cycles 1-2 are correct (inference calls parser).
  If not, trace the failure.
- **REFACTOR**: N/A (likely passes without changes).
- **CLEANUP**: Lint, format, commit.

### Cycle 5: Integration test — full TVIEW creation with CTE

- **RED**: SQL integration test:
  ```sql
  CREATE TABLE tenant.tb_item (pk_item BIGSERIAL PRIMARY KEY, id UUID, name TEXT);
  CREATE TABLE tenant.tb_item_i18n (pk_i18n BIGSERIAL PRIMARY KEY, item_id BIGINT, label TEXT, locale TEXT);

  CREATE TABLE public.tv_item AS
  WITH locale_labels AS (
      SELECT item_id, label
      FROM tenant.tb_item_i18n
      WHERE locale = 'en'
  )
  SELECT
      i.pk_item,
      i.id,
      i.name,
      COALESCE(l.label, i.name) AS identifier,
      jsonb_build_object('id', i.id, 'name', COALESCE(l.label, i.name)) AS data
  FROM tenant.tb_item i
  LEFT JOIN locale_labels l ON l.item_id = i.pk_item;

  -- Verify TVIEW created
  SELECT count(*) FROM pg_tview_meta WHERE entity = 'item';

  -- Insert base data and verify refresh
  INSERT INTO tenant.tb_item VALUES (1, gen_random_uuid(), 'Widget');
  -- (trigger fires, refresh executes the CTE-containing view)
  SELECT count(*) FROM tv_item;
  ```
- **GREEN**: End-to-end should work if parser fix propagates correctly.
- **REFACTOR**: N/A.
- **CLEANUP**: Lint, format, commit.

## Dependencies

- Requires: None (first phase)
- Blocks: Phase 3 (UNION ALL often used inside CTEs)

## Status

[ ] Not Started
