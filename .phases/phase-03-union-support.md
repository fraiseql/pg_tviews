# Phase 3: `UNION ALL` / `UNION` Support — Issue #42

## Objective

Allow TVIEW SELECT statements to combine multiple query branches with
`UNION ALL` (and optionally `UNION`), enabling patterns like locale fallback
chains, multi-source aggregations, and catalog unions.

## Success Criteria

- [ ] `CREATE TABLE tv_item AS SELECT ... UNION ALL SELECT ...` succeeds
- [ ] Column extraction works on the first branch (UNION requires compatible columns)
- [ ] Schema inference works normally on the extracted columns
- [ ] Backing view stores the full UNION SQL
- [ ] Dependency discovery finds base tables across all branches
- [ ] Triggers fire correctly for tables in any branch
- [ ] Incremental refresh re-evaluates the full UNION for the changed key
- [ ] Duplicate-row semantics are handled (see below)
- [ ] CTE + UNION ALL combinations work (Phase 1 + Phase 3)
- [ ] DISTINCT ON + UNION ALL combinations work (Phase 2 + Phase 3)
- [ ] Existing TVIEWs are unaffected

## Analysis

### What already works

- **Backing view creation**: `CREATE VIEW v_entity AS SELECT ... UNION ALL
  SELECT ...` is valid PostgreSQL.
- **Dependency discovery**: `pg_rewrite`/`pg_depend` traverses all branches of
  the UNION and finds every referenced table.
- **Trigger installation**: Installs on all base table OIDs.
- **Dependency type analysis** (`analyzer.rs`): Regex-scans the full SQL, so
  patterns in any branch are detected. However, if the same FK appears in
  multiple branches, duplicates must be deduplicated.

### What breaks

1. **`parser.rs:extract_columns_regex`**: Finds the first `SELECT` and the
   first outer `FROM`. For `SELECT a, b FROM t1 UNION ALL SELECT c, d FROM t2`,
   this correctly parses the first branch's columns. **But**: if the SQL starts
   with a CTE (Phase 1), the CTE-skip logic must fire first. And if any branch
   uses `DISTINCT ON` (Phase 2), the skip logic must handle that too.

   Actually, the first branch parse *should* work because:
   - `find("select")` finds the first SELECT (correct — first branch)
   - `find_outer_from()` finds the first outer FROM (correct — first branch)
   - UNION ALL appears after the first FROM clause, so it's ignored

   **Potential issue**: If the first branch has a subquery (e.g.,
   `SELECT ... FROM (SELECT ...) sub`), `find_outer_from` must skip inner
   FROMs — this is already implemented via paren-depth tracking.

   **Real issue**: `find_outer_from` scans to end of SQL and returns the
   *outermost* FROM. With UNION ALL, the second branch has its own FROM at
   the same paren depth as the first. The function might return the wrong
   FROM if it scans past the UNION boundary.

2. **Refresh scoping**: When a row changes in a table that appears in one
   branch, the refresh query `SELECT * FROM v_entity WHERE id = $1` re-
   evaluates the full UNION for that id. PostgreSQL executes all branches and
   unions the result. This is correct but has a subtlety:

   - With `UNION ALL`: if both branches match for the same `id`, you get
     **duplicate rows**. The UPSERT into TVIEW (which has a PK/unique
     constraint on `pk_entity` or `id`) would fail on the second row.
   - With `UNION` (deduplicated): no duplicates, but slightly slower.

3. **Duplicate-row contract**: The issue's example uses mutually exclusive
   branches (`WHERE locale = X` vs `WHERE NOT EXISTS (...)`). This is the
   expected pattern. We should:
   - Document that `UNION ALL` branches must produce non-overlapping rows for
     the same key (the TVIEW's unique constraint enforces this at UPSERT time)
   - Provide a clear error message if the UPSERT fails due to duplicates
   - Optionally support `UNION` (deduplicated) for cases where overlap exists

### Fix strategy

**Parser**: The main concern is `find_outer_from()` returning a FROM from the
wrong UNION branch. Fix: add a `find_outer_union` scan that detects `UNION`
at paren-depth 0, and limit the FROM search to before the first UNION. If no
UNION exists, behavior is unchanged.

**Metadata**: Add `is_union BOOLEAN DEFAULT FALSE` to `pg_tview_meta` to flag
UNION TVIEWs. This allows refresh logic to know the backing view is a UNION
and handle edge cases (like duplicate-row errors).

**Refresh**: No fundamental change — `SELECT * FROM v_entity WHERE id = $1`
already re-evaluates the full UNION. The UPSERT path handles the result.
Add explicit duplicate detection: if the backing view returns multiple rows
for the same key, log a warning and take the first row (matching DISTINCT ON
behavior), or raise an error.

**Validation**: The `validate_tview_select` function in `hooks.rs` should
accept UNION SQL without warnings.

## TDD Cycles

### Cycle 1: Parser — find_outer_from bounded by UNION

- **RED**: Unit test:
  ```rust
  #[test]
  fn test_parse_union_all_columns() {
      let sql = "SELECT i.pk_item, i.id, l.label AS name, \
                  jsonb_build_object('id', i.id, 'name', l.label) AS data \
                  FROM catalog.tb_item i \
                  JOIN catalog.tb_item_i18n l ON l.item_id = i.pk_item \
                  WHERE l.locale = 'en' \
                  UNION ALL \
                  SELECT i.pk_item, i.id, i.name, \
                  jsonb_build_object('id', i.id, 'name', i.name) AS data \
                  FROM catalog.tb_item i \
                  WHERE NOT EXISTS (SELECT 1 FROM catalog.tb_item_i18n l \
                  WHERE l.item_id = i.pk_item AND l.locale = 'en')";
      let cols = parse_select_columns(sql).unwrap();
      assert_eq!(cols, vec!["pk_item", "id", "name", "data"]);
  }
  ```
- **GREEN**: Implement `find_outer_union(sql: &str, start: usize) -> Option<usize>`
  that scans for `UNION` at **paren-depth 0, outside string literals** — the
  same depth-aware, quote-aware byte walker already used in `find_outer_from`
  and `split_by_top_level_comma`. A `UNION` inside a subquery (depth > 0) or
  inside a string literal must be ignored. Use this to bound the
  `find_outer_from` search: only scan between `select_start` and
  `union_pos.unwrap_or(sql.len())`.
- **REFACTOR**: Ensure `find_outer_union` handles:
  - `UNION ALL` vs `UNION` vs `UNION DISTINCT`
  - `UNION` inside string literals (e.g. `WHERE label = 'UNION ALL'`) — must
    not match
- **CLEANUP**: Lint, format, commit.

### Cycle 2: CTE + UNION ALL combination

- **RED**: Test CTE wrapping a UNION:
  ```rust
  #[test]
  fn test_parse_cte_with_union_all() {
      let sql = "WITH fallback AS (SELECT ...) \
                  SELECT ... FROM tb_item \
                  UNION ALL \
                  SELECT ... FROM fallback";
      let cols = parse_select_columns(sql).unwrap();
      // Should return main query's first-branch columns
  }
  ```
- **GREEN**: Phase 1's CTE-skip runs first, advancing to the main SELECT.
  Then UNION-bounded FROM search runs on the post-CTE SQL. Verify the
  composition works.
- **REFACTOR**: Offset composition strategy — both `skip_cte_preamble` and
  `find_outer_union` accept and return **absolute byte offsets into the original
  lowercased SQL string**. The parser maintains one `cursor: usize` variable:
  ```
  let cursor = skip_cte_preamble(sql_lower, 0)?;   // advances past WITH
  let select_start = find_keyword(sql_lower, cursor, "select")?;
  let union_bound = find_outer_union(sql_lower, select_start)
      .unwrap_or(sql_lower.len());
  let from_pos = find_outer_from(sql_lower, select_start, union_bound)?;
  ```
  All three functions operate on the same string with absolute positions —
  no substring slicing that would shift offsets.
- **CLEANUP**: Lint, format, commit.

### Cycle 3: DISTINCT ON + UNION ALL combination

- **RED**: Test DISTINCT ON inside a UNION branch:
  ```rust
  #[test]
  fn test_parse_distinct_on_union_all() {
      let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id, c.name, \
                  jsonb_build_object('id', c.id) AS data \
                  FROM tb_contract c ORDER BY c.id, c.version DESC \
                  UNION ALL \
                  SELECT ...";
      let cols = parse_select_columns(sql).unwrap();
      assert_eq!(cols[0], "pk_contract");
  }
  ```
- **GREEN**: Phase 2's DISTINCT ON skip runs after SELECT detection, before
  column extraction. UNION bound limits FROM search. All three phases compose.
- **REFACTOR**: Document the parser's skip chain:
  `CTE skip → SELECT → DISTINCT ON skip → columns → FROM (bounded by UNION)`
- **CLEANUP**: Lint, format, commit.

### Cycle 4: Schema inference and metadata for UNION TVIEWs

- **RED**: Test `infer_schema()` with UNION SQL — verify columns from first
  branch are used. Test metadata registration stores `is_union = true`.
- **GREEN**: Schema inference calls the parser, which now handles UNION. Add
  `is_union` field to `TviewMeta` and set it during `register_metadata` based
  on detecting UNION in the SQL.
- **REFACTOR**: Detection of UNION for the metadata flag should reuse
  `find_outer_union()` from Cycle 1.
- **CLEANUP**: Lint, format, commit.

### Cycle 5: Refresh — handle UNION duplicate-row edge case

- **RED**: Test scenario:
  ```
  Backing view with UNION ALL returns 2 rows for id=42 (overlapping branches).
  Refresh for id=42 should handle this gracefully — either take first row
  or raise a clear error.
  ```
- **GREEN**: In `refresh/main.rs`, after querying the backing view:
  - If exactly 1 row → UPSERT (normal path)
  - If 0 rows → DELETE from TVIEW (key no longer exists)
  - If >1 rows → log warning, take first row (UNION ALL with overlapping
    branches), UPSERT that row. Alternatively, error with a message like:
    "UNION ALL branches produce duplicate rows for key=42; ensure branches
    are mutually exclusive or use UNION instead of UNION ALL"
- **REFACTOR**: Make the behavior configurable via a GUC
  (`pg_tviews.union_duplicate_policy = 'first' | 'error'`), defaulting to
  `'error'` to surface misconfigurations early.
- **CLEANUP**: Lint, format, commit.

### Cycle 6: Integration test — locale fallback UNION ALL

- **RED**: SQL integration test from the issue's example:
  ```sql
  CREATE TABLE catalog.tb_item (pk_item BIGSERIAL PRIMARY KEY, id UUID, tenant_id UUID, name TEXT);
  CREATE TABLE catalog.tb_item_i18n (pk_i18n BIGSERIAL PRIMARY KEY, item_id BIGINT, label TEXT, locale TEXT);

  CREATE TABLE public.tv_item AS
  SELECT i.pk_item, i.id, i.tenant_id, l.label AS name,
         jsonb_build_object('id', i.id, 'name', l.label) AS data
  FROM catalog.tb_item i
  JOIN catalog.tb_item_i18n l ON l.item_id = i.pk_item
  WHERE l.locale = 'en'
  UNION ALL
  SELECT i.pk_item, i.id, i.tenant_id, i.name,
         jsonb_build_object('id', i.id, 'name', i.name) AS data
  FROM catalog.tb_item i
  WHERE NOT EXISTS (
      SELECT 1 FROM catalog.tb_item_i18n l
      WHERE l.item_id = i.pk_item AND l.locale = 'en'
  );

  -- Insert item without translation → fallback branch wins
  INSERT INTO catalog.tb_item VALUES (1, gen_random_uuid(), gen_random_uuid(), 'Default Name');
  SELECT name FROM tv_item;  -- 'Default Name'

  -- Add translation → localized branch wins, fallback disappears
  INSERT INTO catalog.tb_item_i18n VALUES (1, 1, 'Translated', 'en');
  SELECT name FROM tv_item;  -- 'Translated'

  -- Remove translation → fallback returns
  DELETE FROM catalog.tb_item_i18n WHERE pk_i18n = 1;
  SELECT name FROM tv_item;  -- 'Default Name'

  -- Verify triggers are installed on BOTH branches' base tables
  SELECT tgrelid::regclass::text FROM pg_trigger
  WHERE tgname LIKE '%tv_item%'
  ORDER BY 1;
  -- Expected: catalog.tb_item AND catalog.tb_item_i18n both appear
  -- (dependency discovery must traverse all UNION branches, not just the first)
  ```
- **GREEN**: End-to-end should work with all previous cycles. If the trigger
  count assertion fails, `pg_rewrite`/`pg_depend` traversal is not reaching
  the second branch — investigate `dependency/graph.rs:find_base_tables`.
- **REFACTOR**: N/A.
- **CLEANUP**: Lint, format, commit.

### Cycle 7: UNION (deduplicated) support

- **RED**: Test with `UNION` (no `ALL`) — verify deduplication semantics.
  This should work the same as UNION ALL from the parser's perspective, but
  the backing view applies deduplication.
- **GREEN**: Parser already handles this (UNION keyword detected regardless of
  ALL). Metadata flag `is_union` applies to both variants. Refresh path is
  identical (the view does the deduplication).
- **REFACTOR**: Consider storing `union_type` (`'union_all'` | `'union'`)
  instead of boolean `is_union` for richer metadata.
- **CLEANUP**: Lint, format, commit.

## Dependencies

- Requires: Phase 1 (CTE skip) and Phase 2 (DISTINCT ON skip) for composition
- Blocks: None (final phase)

## Status

[ ] Not Started
