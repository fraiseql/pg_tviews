# Phase: Array Column Handling (Issue #24)

## Objective

Enable end-to-end incremental refresh for TVIEWs that contain JSONB array
aggregations (e.g. `tv_post.data->'comments'` built from `jsonb_agg()`),
so that INSERT, UPDATE, and DELETE on child tables propagate correctly to
parent TVIEW rows.

## Success Criteria

- [ ] `test/sql/50_array_columns.sql` passes — array-typed columns materialize
- [ ] `test/sql/51_jsonb_array_update.sql` passes — child UPDATE propagates to parent array element
- [ ] `test/sql/52_array_insert_delete.sql` passes — child INSERT/DELETE grows/shrinks parent array
- [ ] `cargo check` and `cargo clippy -- -D warnings` pass clean
- [ ] No regressions on existing scalar/nested-object refresh tests

## Architecture Overview

### Current state

```
tb_comment INSERT/UPDATE/DELETE
  → pg_tview_trigger_handler()
    → entity_for_table(tb_comment_oid)  →  None  ← BUG: tb_comment is not
    → return Ok(None)  ← silent no-op       a "source" in pg_tview_meta
```

The trigger fires but does nothing because `entity_for_table` only checks
`view_oid` and `table_oid` in `pg_tview_meta`, not the `dependencies OID[]`
array. The child table is a dependency of a parent TVIEW but not a TVIEW source
itself.

### Target state

```
tb_comment INSERT/UPDATE/DELETE
  → pg_tview_trigger_handler()
    → entities_for_base_table(tb_comment_oid)  →  ["post"]
    → for each parent entity:
        → find affected parent PKs (via FK join)
        → enqueue_refresh("post", affected_pk)
  → PRE_COMMIT
    → refresh_pk(tv_post_oid, pk)
    → apply_patch() dispatches on DependencyType::Array
      → UPDATE: jsonb_smart_patch_array (existing)
      → INSERT/DELETE: apply_full_replacement (re-aggregate)
```

### Key design decisions

1. **No operation-type tracking in the queue.** Instead of adding INSERT/UPDATE/DELETE
   to `RefreshKey`, we use `apply_full_replacement` for array-typed refreshes.
   `jsonb_smart_patch_array` handles element-level UPDATE diffs already; for
   INSERT/DELETE the parent row is fully re-aggregated from the view query.
   This is simpler and correct — the `insert_array_element`/`delete_array_element`
   functions in `array_ops.rs` are premature optimization and stay dead code for now.

2. **Base table → parent entity mapping via `dependencies OID[]`.** No new table.
   Query `pg_tview_meta WHERE $1 = ANY(dependencies)` to find all entities whose
   TVIEW depends on a given base table.

3. **FK-based parent PK discovery.** When `tb_comment` changes, we need to find
   which `tv_post` rows are affected. The child row has `fk_post` → use that to
   find the parent PK. This requires knowing the FK column name, which we derive
   from the parent entity: `fk_{parent_entity}` convention.

## Gaps to Close

| # | Gap | File(s) | Severity |
|---|-----|---------|----------|
| 1 | `entity_for_table` returns `None` for indirect base tables | `catalog.rs`, `trigger.rs` | **Critical** |
| 2 | No reverse lookup: base table → parent entities | `catalog.rs`, `trigger.rs` | **Critical** |
| 3 | No FK-based parent PK discovery from child row | `trigger.rs` or `propagate.rs` | **Critical** |
| 4 | Analyzer regex requires `v_entity.data` pattern, misses inline `jsonb_build_object` | `schema/analyzer.rs` | **High** |
| 5 | `parse_dependencies()` iterates `fk_columns` only, drops array deps | `catalog.rs` | **High** |
| 6 | `jsonb_delta` stubs missing `jsonb_array_insert_where` / `jsonb_array_delete_where` | test stubs | **Medium** (only if we use surgical ops) |
| 7 | `apply_patch` does not fall back to full replacement for array INSERT/DELETE | `refresh/main.rs` | **High** |

## Files to modify

| File | Change |
|------|--------|
| `src/catalog.rs` | Add `parent_entities_for_base_table()` — queries `WHERE $1 = ANY(dependencies)` |
| `src/trigger.rs` | Update `pg_tview_trigger_handler` to use new lookup and enqueue parent refreshes |
| `src/schema/analyzer.rs` | Broaden regex to match inline `jsonb_agg(jsonb_build_object(...))` |
| `src/catalog.rs` | Fix `parse_dependencies()` to iterate `dependency_types` not just `fk_columns` |
| `src/refresh/main.rs` | Ensure `refresh_pk` handles the case where the TVIEW row needs full re-aggregation |
| `src/propagate.rs` | Update `find_parent_entities` to also check `dependencies` array |

## TDD Cycles

### Cycle 1: Base table → parent entity lookup

**RED**: Write a `#[pg_test]` that:
1. Creates `tb_user`, `tb_post`, `tb_comment` tables
2. Calls `pg_tviews_create` for a post TVIEW that JOINs `tb_comment`
3. Calls `parent_entities_for_base_table(tb_comment_oid)` and asserts it returns `["post"]`

**GREEN**: In `src/catalog.rs`, add:
```rust
/// Find all TVIEW entities whose dependency list includes the given base table OID.
/// Returns entity names (e.g. ["post", "comment"]) for all TVIEWs that depend on this table.
pub fn parent_entities_for_base_table(table_oid: pg_sys::Oid) -> TViewResult<Vec<String>> {
    Spi::connect(|client| {
        let query = "SELECT entity FROM pg_tview_meta WHERE $1 = ANY(dependencies)";
        let args = vec![unsafe {
            DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value())
        }];
        let rows = client.select(query, None, &args)?;
        let mut entities = Vec::new();
        for row in rows {
            if let Some(entity) = row["entity"].value::<String>()? {
                entities.push(entity);
            }
        }
        Ok(entities)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "parent_entities_for_base_table".to_string(),
        pg_error: format!("{e:?}"),
    })
}
```

**REFACTOR**: Ensure consistent error handling with existing catalog functions.

**CLEANUP**: Lint, format.

### Cycle 2: Update trigger handler to route indirect base tables

**RED**: Write a `#[pg_test]` that:
1. Sets up `tb_user`, `tb_post`, `tb_comment` with a post TVIEW
2. INSERTs a comment
3. Asserts that `get_queue_contents()` contains a refresh entry for entity `"post"`

**GREEN**: Modify `pg_tview_trigger_handler` in `src/trigger.rs`:

```rust
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => { warning!("..."); return Ok(None); }
    };

    // 1. Direct entity: this table IS a TVIEW source (e.g. tb_user → entity "user")
    match entity_for_table(table_oid) {
        Ok(Some(entity)) => {
            let pk_value = match crate::utils::extract_pk(trigger) {
                Ok(pk) => pk,
                Err(e) => { warning!("..."); return Ok(None); }
            };
            enqueue_refresh(&entity, pk_value);
            return Ok(None);
        }
        Ok(None) => { /* fall through to indirect lookup */ }
        Err(e) => { warning!("..."); return Ok(None); }
    }

    // 2. Indirect: this table is a dependency of one or more TVIEWs
    //    Find parent entities, extract their FK from the child row, enqueue parent refresh
    let parent_entities = match crate::catalog::parent_entities_for_base_table(table_oid) {
        Ok(entities) => entities,
        Err(e) => { warning!("..."); return Ok(None); }
    };

    if parent_entities.is_empty() {
        return Ok(None);  // table not managed by pg_tviews at all
    }

    let tuple = trigger.new().or_else(|| trigger.old())
        .expect("Row must exist for AFTER trigger");

    for parent_entity in parent_entities {
        // Convention: child row has fk_{parent_entity} column
        let fk_col = format!("fk_{parent_entity}");
        match tuple.get_by_name::<i64>(&fk_col) {
            Ok(Some(parent_pk)) => {
                enqueue_refresh(&parent_entity, parent_pk);
            }
            Ok(None) => {
                warning!("FK column {} is NULL, skipping", fk_col);
            }
            Err(_) => {
                // FK column doesn't exist — try full table scan approach
                // or skip this parent
                warning!("No FK column {} on child row", fk_col);
            }
        }
    }

    Ok(None)
}
```

**Important**: This calls `parent_entities_for_base_table` which uses `Spi::connect`.
The Rust `#[pg_trigger]` handler runs outside PL/pgSQL, so nested SPI is safe here
(no PL/pgSQL SPI context wrapping us — this is a C-language trigger function). Verify
this does not SIGABRT. If it does, we need to defer the lookup to PRE_COMMIT.

**REFACTOR**: Extract the indirect-lookup logic into a helper function.

**CLEANUP**: Lint, format.

### Cycle 3: Analyzer regex for inline `jsonb_agg(jsonb_build_object(...))`

**RED**: Write a unit test in `schema/analyzer.rs`:
```rust
#[test]
fn test_detect_inline_jsonb_agg() {
    let sql = "SELECT pk_post, jsonb_build_object(
        'title', p.title,
        'comments', COALESCE(jsonb_agg(
            jsonb_build_object('id', c.pk_comment, 'text', c.text)
            ORDER BY c.pk_comment
        ) FILTER (WHERE c.pk_comment IS NOT NULL), '[]'::jsonb)
    ) AS data FROM tb_post p LEFT JOIN tb_comment c ON c.fk_post = p.pk_post";
    let deps = analyze_dependencies(sql, &["fk_user".to_string()]);
    // Should detect an Array dependency for 'comments'
    assert!(deps.iter().any(|d| d.dep_type == DependencyType::Array));
}
```

**GREEN**: Update `ARRAY_PATTERN_TEMPLATE` or add a second pattern that matches
`'key',\s*(?:coalesce\s*\()?\s*jsonb_agg\s*\(` without requiring `v_entity.data`:
```rust
const INLINE_ARRAY_PATTERN: &str =
    r"'(\w+)',\s*(?:coalesce\s*\()?\s*jsonb_agg\s*\(";
```

Then in `detect_array_dependencies`, try both patterns.

**REFACTOR**: Consolidate pattern matching logic.

**CLEANUP**: Lint, format.

### Cycle 4: Fix `parse_dependencies()` length mismatch

**RED**: Write a unit test constructing a `TviewMeta` where `dependency_types` has
more entries than `fk_columns`, and assert that `parse_dependencies()` returns all of
them.

**GREEN**: Change `parse_dependencies()` in `catalog.rs` to iterate over the longest
array (`dependency_types`) rather than `fk_columns`:
```rust
pub fn parse_dependencies(&self) -> Vec<DependencyDetail> {
    let len = self.dependency_types.len()
        .max(self.fk_columns.len());
    let mut details = Vec::new();
    for i in 0..len {
        let dep_type = self.dependency_types.get(i).cloned()
            .unwrap_or(DependencyType::Scalar);
        let path = self.dependency_paths.get(i).cloned().flatten();
        let match_key = self.array_match_keys.get(i).cloned().flatten();
        details.push(DependencyDetail { dep_type, path, match_key });
    }
    details
}
```

**REFACTOR**: Clean.

**CLEANUP**: Lint, format.

### Cycle 5: Refresh pipeline — full replacement for array parents

**RED**: Write a `#[pg_test]` that:
1. Creates post + comment TVIEWs with array aggregation
2. Inserts a post with 2 comments
3. INSERTs a 3rd comment
4. Triggers PRE_COMMIT processing
5. Asserts `tv_post.data->'comments'` has 3 elements

**GREEN**: In `refresh/main.rs`, when `apply_patch` encounters a `DependencyType::Array`
dependency and the smart patch function is not available or the operation is INSERT/DELETE
(detected by the new row having a different element count than the old), fall back to
`apply_full_replacement`. This re-runs the full view query and overwrites the TVIEW row.

Actually, the simpler approach: for parent entities refreshed due to indirect base table
changes, always use `apply_full_replacement`. The `jsonb_smart_patch_array` path is for
when the child TVIEW row itself is updated and the parent's array element needs surgical
update — that already works. For INSERT/DELETE, full replacement is correct and simple.

Ensure `refresh_pk` handles the case where the view row exists (re-aggregate) or doesn't
exist yet (INSERT).

**REFACTOR**: Clean.

**CLEANUP**: Lint, format.

### Cycle 6: Integration — end-to-end test validation

**RED**: Run the three SQL test files:
```bash
# These should now pass
psql -f test/sql/50_array_columns.sql
psql -f test/sql/51_jsonb_array_update.sql
psql -f test/sql/52_array_insert_delete.sql
```

**GREEN**: Fix any remaining integration issues discovered by the tests.

**REFACTOR**: Clean up any workarounds.

**CLEANUP**: Final lint pass, commit.

## SPI Safety Concern

**Critical question**: Can `pg_tview_trigger_handler` (a C-language trigger via `#[pg_trigger]`)
safely call `Spi::connect` for the `parent_entities_for_base_table` query?

The answer should be **yes** — the old SIGABRT (#31/#32) was caused by PL/pgSQL holding
an SPI connection and then calling a pgrx `#[pg_extern]` that opened *another* `Spi::connect`.
A `#[pg_trigger]` runs as a C function — PostgreSQL calls it directly without an intervening
PL/pgSQL SPI context. So `Spi::connect` inside it should be safe.

**However**, if this assumption is wrong and we get SIGABRTs, the fallback is:
- Store a raw `(table_oid, pk_value)` in the queue (no SPI at trigger time)
- Resolve parent entities during `handle_pre_commit` (clean SPI context)
- This matches the pattern already used for the direct-entity path

Test this early in Cycle 2 before building on it.

## Verification

```bash
# Compile
cargo check

# Lint
cargo clippy -- -D warnings

# Unit tests
cargo test

# Integration (requires running PG instance)
psql -f test/sql/50_array_columns.sql
psql -f test/sql/51_jsonb_array_update.sql
psql -f test/sql/52_array_insert_delete.sql
```

## Risk Assessment

**Medium-high risk.** This touches the trigger handler hot path and the SPI safety
boundary. The main risks:

1. **SPI in trigger context** — could SIGABRT if the `#[pg_trigger]` → `Spi::connect`
   assumption is wrong. Mitigated by early testing and a documented fallback.
2. **Performance** — `parent_entities_for_base_table` runs a query per trigger fire.
   Could be expensive on bulk operations. Mitigated by the existing table cache
   infrastructure (`queue/cache.rs`) — extend it to cache this mapping too.
3. **FK naming convention** — assumes child rows have `fk_{parent_entity}`. If a
   TVIEW uses a non-standard FK column name, the lookup fails silently. This is
   consistent with the existing convention used throughout pg_tviews.

## Dependencies

- None — but benefits from #27 (GUC for cache enable/disable)
- Should be implemented after #27

## Status
[x] Complete (2026-03-19)

## Implementation Notes

Cycles 1-4 were already implemented in prior work. The remaining gaps were:

1. **pgrx 0.16.1 `Spi::get_one_with_args` bug**: Returns `SpiTupleTable positioned`
   error when query returns zero rows. Fixed `entity_for_table_uncached` to use
   `Spi::connect` + `client.select` instead. This was the critical bug preventing
   indirect (child→parent) trigger propagation.

2. **INTEGER vs BIGINT type mismatch**: Trigger tuple extraction used `get_by_name::<i64>`
   which fails for INTEGER columns. Added `tuple_get_i64()` helper that tries BIGINT
   first, falls back to INTEGER with promotion. Used in both `extract_pk` and
   `enqueue_indirect_parents`.

3. **Full replacement path works correctly** for all array operations (INSERT/UPDATE/DELETE).
   Smart patching via `jsonb_delta` is an optional optimization; without it, the
   `apply_full_replacement` UPSERT re-aggregates from the backing view correctly.
