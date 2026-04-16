# Phase 2: `DISTINCT ON` Support — Issue #40

## Objective

Allow TVIEW SELECT statements to use `DISTINCT ON (expr_list)` for
deduplication, enabling patterns like versioned rows where only the latest
version materializes into the TVIEW.

## Success Criteria

- [ ] `CREATE TABLE tv_contract AS SELECT DISTINCT ON (c.id) ...` succeeds
- [ ] Column extraction skips past `DISTINCT ON (...)` to find real columns
- [ ] Schema inference works normally on the extracted columns
- [ ] Backing view stores the full SQL including DISTINCT ON and ORDER BY
- [ ] Metadata stores the DISTINCT ON key(s) for refresh scoping
- [ ] Incremental refresh scopes by DISTINCT ON key, not just PK
- [ ] When the "winning" row changes, the TVIEW updates correctly
- [ ] When all rows for a DISTINCT ON key are deleted, the TVIEW row is removed
- [ ] Existing non-DISTINCT-ON TVIEWs are unaffected

## Analysis

### What already works

- **Backing view creation**: `CREATE VIEW v_entity AS SELECT DISTINCT ON (...)
  ...` is valid PostgreSQL — works as-is.
- **Dependency discovery**: `pg_rewrite`/`pg_depend` traversal is
  SQL-agnostic.
- **Trigger installation**: Installs on base table OIDs — no change needed.
- **Dependency type analysis** (`analyzer.rs`): Regex patterns scan full SQL
  text — unaffected by DISTINCT ON clause.

### What breaks

1. **`parser.rs:extract_columns_regex`**: After finding `SELECT`, it takes
   everything up to `FROM` as the column list. With `SELECT DISTINCT ON (c.id)
   c.pk_contract, ...`, the `DISTINCT ON (c.id)` portion pollutes the column
   list. `extract_column_name("DISTINCT ON (c.id)")` would return garbage.

2. **Refresh scoping**: The current refresh model assumes each base-table row
   maps 1:1 to a TVIEW row via `pk_entity`. With DISTINCT ON, multiple
   base-table rows map to one TVIEW row (the "winner"). When any of those rows
   changes, we must:
   - Identify the DISTINCT ON key value (e.g., `c.id = 42`)
   - Re-query `SELECT * FROM v_entity WHERE id = 42` to find the new winner
   - UPSERT the winner into the TVIEW (or DELETE if no rows remain)

   The current trigger extracts `pk_entity` from the changed row and refreshes
   by PK. For DISTINCT ON TVIEWs, the trigger must instead extract the
   **DISTINCT ON key** column value and refresh by that key.

3. **TVIEW primary key semantics**: For a DISTINCT ON TVIEW, the TVIEW's PK
   should be the dedup key (e.g., `id`), NOT the base table's `pk_contract`.
   The `pk_contract` column is still useful for the backing view's ORDER BY
   but is NOT materialized as the TVIEW PK. This means `pk_entity` may be
   absent from the TVIEW schema — the schema inference must handle this.

### Fix strategy

**Parser**: After detecting `SELECT`, check for `DISTINCT ON (...)` and skip
past the closing parenthesis before extracting columns. Store the DISTINCT ON
expression list in metadata for refresh scoping.

**Metadata**: Add a `distinct_on_keys` field to `pg_tview_meta` — a TEXT[]
storing the column names/expressions used in DISTINCT ON. When NULL, the TVIEW
uses standard PK-based refresh.

**Refresh**: When `distinct_on_keys` is non-NULL:
- The trigger extracts the DISTINCT ON key value from the changed row
  (e.g., `NEW.id` or `OLD.id`)
- Refresh queries `SELECT * FROM v_entity WHERE <dedup_key> = $1`
- If the query returns a row, UPSERT into TVIEW
- If the query returns no rows, DELETE from TVIEW where `<dedup_key> = $1`

**Table schema**: For DISTINCT ON TVIEWs, the materialized table uses the
DISTINCT ON key as its effective unique constraint. The `pk_entity` column
can still exist (from the backing view) but the UPSERT conflict target is
the dedup key column.

## TDD Cycles

### Cycle 1: Parser — skip DISTINCT ON clause

- **RED**: Unit test:
  ```rust
  #[test]
  fn test_parse_distinct_on_columns() {
      let sql = "SELECT DISTINCT ON (c.id) c.pk_contract, c.id, c.name, \
                  jsonb_build_object('id', c.id, 'name', c.name) AS data \
                  FROM tenant.tb_contract c ORDER BY c.id, c.version DESC";
      let cols = parse_select_columns(sql).unwrap();
      assert_eq!(cols, vec!["pk_contract", "id", "name", "data"]);
  }
  ```
- **GREEN**: In `extract_columns_regex`, after finding `SELECT` at position
  `select_start`, detect `DISTINCT ON` and advance past the parenthesized
  expression list using the paren-depth walker from Phase 1.
  ```
  select_start + 6 → trim → check starts_with("distinct on")
    → find '(' → walk to matching ')' → new column_start
  ```
  Also handle plain `SELECT DISTINCT` (no `ON`) — just skip the word.
- **REFACTOR**: Extract DISTINCT ON detection into a helper
  `skip_distinct_clause(sql, offset) -> (usize, Option<Vec<String>>)` that
  returns the new offset and optionally the DISTINCT ON key expressions.
- **CLEANUP**: Lint, format, commit.

### Cycle 2: Extract DISTINCT ON keys

- **RED**: Test a function `extract_distinct_on_keys(sql)` that returns the
  expression list:
  ```rust
  #[test]
  fn test_extract_distinct_on_keys() {
      let sql = "SELECT DISTINCT ON (c.id) ...";
      let keys = extract_distinct_on_keys(sql).unwrap();
      assert_eq!(keys, vec!["c.id"]);
  }

  #[test]
  fn test_extract_composite_distinct_on() {
      let sql = "SELECT DISTINCT ON (c.tenant_id, c.id) ...";
      let keys = extract_distinct_on_keys(sql).unwrap();
      assert_eq!(keys, vec!["c.tenant_id", "c.id"]);
  }
  ```
- **GREEN**: Parse the parenthesized expression list, split by top-level
  commas, strip table qualifiers for storage.
- **REFACTOR**: Reuse `split_by_top_level_comma` from parser.rs.
- **CLEANUP**: Lint, format, commit.

### Cycle 3: Metadata, RefreshKey typing, and UPSERT conflict target

This cycle handles three coupled concerns that must be designed together before
refresh logic is written in Cycle 4.

**3a — Metadata schema**

- **RED**: Test that `pg_tview_meta` stores `distinct_on_keys` for a DISTINCT
  ON TVIEW, and NULL for a normal TVIEW.
- **GREEN**: Add `distinct_on_keys TEXT[]` column to `pg_tview_meta` (via
  extension upgrade SQL). Populate during `register_metadata` in
  `ddl/create.rs`. Update `TviewMeta` struct in `catalog.rs`.
- **REFACTOR**: Ensure `TviewMeta::load()` reads the new column with a
  fallback to NULL for existing TVIEWs.

**3b — RefreshKey variant type**

`refresh_pk(source_oid: Oid, pk: i64)` takes a BIGINT. For DISTINCT ON TVIEWs
whose dedup key is `id` (UUID), the trigger must enqueue a UUID — not a BIGINT.
Changing the key type ripples through `queue/key.rs`, `queue/ops.rs`,
`trigger.rs`, and `refresh_pk` itself. Define the variant type now so Cycles
4 and 5 can be written cleanly:

```rust
// queue/key.rs (new)
pub enum RefreshKey {
    Pk(i64),       // standard TVIEW — pk_entity BIGINT
    Dedup(String), // DISTINCT ON TVIEW — dedup key value, cast to TEXT for queue storage
}
```

- **RED**: Test that the queue accepts both `RefreshKey::Pk` and
  `RefreshKey::Dedup` and round-trips them through enqueue/dequeue.
- **GREEN**: Add `RefreshKey` enum. Update `queue/ops.rs` enqueue/dequeue to
  carry the variant. Update `trigger.rs` to enqueue `RefreshKey::Dedup` when
  `TviewMeta.distinct_on_keys` is `Some`. Existing `RefreshKey::Pk` path is
  unchanged.
- **REFACTOR**: `refresh_pk` becomes `refresh_by_key(meta, key: RefreshKey)` or
  keeps the name with a richer signature — decide based on callsite clarity.

**3c — UPSERT conflict target in `create_materialized_table`**

For DISTINCT ON TVIEWs, the unique constraint must be on the dedup key column(s)
(e.g., `id`), not on `pk_contract`. The `pk_contract` column can still be
materialized (useful for debugging and ORDER BY), but must not be the table PK.
`infer_schema` currently promotes a `pk_<entity>` column to the primary key.
When `distinct_on_keys` is non-empty:
- The materialized table's primary key is on the dedup key column(s)
- `pk_<entity>` gets a regular index instead
- The UPSERT in `apply_patch` / `apply_full_replacement` uses the dedup key
  as the `ON CONFLICT` target

- **RED**: Test that `create_materialized_table` with `distinct_on_keys = ["id"]`
  creates the PK on `id`, not on `pk_contract`.
- **GREEN**: Pass `distinct_on_keys` into `create_materialized_table` and
  branch the PK/constraint DDL generation accordingly.
- **REFACTOR**: Extract a `primary_key_columns(schema, distinct_on_keys)` helper.

- **CLEANUP**: Lint, format, commit all three sub-cycles together.

### Cycle 4: Refresh scoping by DISTINCT ON key (uses RefreshKey from Cycle 3)

- **RED**: Test scenario:
  ```
  Base table: tb_contract (pk_contract, id, version, name)
  Rows: (1, 100, 3, 'v3'), (2, 100, 2, 'v2'), (3, 100, 1, 'v1')
  TVIEW: tv_contract with DISTINCT ON (c.id) ORDER BY c.id, c.version DESC
  Expected: tv_contract has 1 row: id=100, name='v3'

  Delete row pk_contract=1 (version 3)
  Expected: tv_contract updates to id=100, name='v2' (version 2 wins)
  ```
- **GREEN**: Modify `refresh/main.rs` to dispatch on `RefreshKey`:
  - `RefreshKey::Pk(pk)` → existing path, query `WHERE pk_entity = $1`
  - `RefreshKey::Dedup(key)` → new path:
    - Query backing view: `SELECT * FROM v_entity WHERE id = $1::uuid` (or
      the appropriate dedup key column and cast)
    - If row returned → UPSERT into TVIEW (conflict on dedup key per Cycle 3c)
    - If no row → DELETE from TVIEW where dedup key = $1
- **REFACTOR**: Extract the "refresh by dedup key" path into a separate
  function `refresh_by_distinct_key()`.
- **CLEANUP**: Lint, format, commit.

### Cycle 5: Trigger — extract correct key for enqueue (wires RefreshKey from Cycle 3)

- **RED**: Test that when a trigger fires on `tb_contract`, the refresh queue
  contains the DISTINCT ON key value (the `id`), not the `pk_contract`.
- **GREEN**: `trigger.rs:pg_tview_trigger_handler()` already conditionally
  enqueues `RefreshKey::Dedup` when `TviewMeta.distinct_on_keys` is `Some`
  (wired in Cycle 3b). This cycle verifies the trigger handler extracts the
  correct column value from the `NEW`/`OLD` Datum and that the enqueued key
  survives a round-trip through the queue to `refresh_by_distinct_key()`.
- **REFACTOR**: Ensure that for DELETE triggers, `OLD` is used (not `NEW`),
  and for INSERT/UPDATE, `NEW` is used.
- **CLEANUP**: Lint, format, commit.

### Cycle 6: Integration test — full DISTINCT ON TVIEW lifecycle

- **RED**: SQL integration test covering:
  - Create TVIEW with DISTINCT ON
  - Verify initial population (only winners)
  - Insert new winning row → old winner replaced
  - Delete current winner → next-best promoted
  - Delete all rows for a key → TVIEW row removed
  - Update non-winning row → no TVIEW change
  - Update row to become new winner → TVIEW updated
- **GREEN**: End-to-end should work with all previous cycles.
- **REFACTOR**: N/A.
- **CLEANUP**: Lint, format, commit.

## Dependencies

- Requires: Phase 1 (reuses CTE paren-depth walker utility)
- Blocks: Phase 3 (UNION + DISTINCT ON combinations)

## Status

[ ] Not Started
