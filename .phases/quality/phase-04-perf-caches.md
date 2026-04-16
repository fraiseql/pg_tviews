# Phase 4: Performance — Caching & Batching

## Objective

Eliminate the remaining MEDIUM and LOW performance bottlenecks:
OID→relname uncached; propagation bypasses the dependency graph cache; affected-PK lookup
unbatched; column-name discovery uncached; flush-loop allocations unguided; dedup-key
refresh wastes a round trip.

## Success Criteria

- [ ] `relname_from_oid` makes at most one `pg_class` query per OID per session
- [ ] `find_parent_entities` uses the cached `EntityDepGraph` (zero SPI per flush)
- [ ] `find_affected_pks` issues one query per entity-pair per flush (not one per key)
- [ ] `apply_full_replacement` makes at most one `pg_attribute` query per view per session
- [ ] Flush-loop `HashMap`/`HashSet` initialized with `with_capacity`
- [ ] `refresh_by_dedup_key` issues at most two queries per refresh (down from guaranteed two)
- [ ] `cargo clippy --no-default-features --features pg18 -- -D warnings` clean
- [ ] All existing tests pass

## TDD Cycles

### Cycle 1: P-05 — relname_from_oid uncached (utils.rs:167)

**ROOT CAUSE:**
`relname_from_oid(oid)` runs `SELECT relname FROM pg_class WHERE oid = $1` on every call.
Called in `apply_patch`, `apply_full_replacement`, `refresh_by_dedup_key` (twice), and
`refresh/bulk.rs`. OID→relname mappings are stable within a session — they only change
on DDL, which is already tracked via ProcessUtility hook.

- **RED**: Write a test: call `relname_from_oid` twice with the same OID; assert the
  second call hits the cache (pg_class query count = 1).
- **GREEN**:
  ```rust
  static OID_RELNAME_CACHE: LazyLock<Mutex<HashMap<pg_sys::Oid, String>>> =
      LazyLock::new(|| Mutex::new(HashMap::new()));

  pub fn relname_from_oid(oid: pg_sys::Oid) -> spi::Result<Option<String>> {
      {
          let cache = OID_RELNAME_CACHE.lock().unwrap();
          if let Some(name) = cache.get(&oid) {
              return Ok(Some(name.clone()));
          }
      }
      // slow path
      let name = Spi::get_one_with_args::<String>(
          "SELECT relname::text FROM pg_class WHERE oid = $1",
          &[DatumWithOid::new(oid.as_u32(), OIDOID)],
      )?;
      if let Some(ref n) = name {
          OID_RELNAME_CACHE.lock().unwrap().insert(oid, n.clone());
      }
      Ok(name)
  }
  ```
  Add `OID_RELNAME_CACHE.lock().unwrap().clear()` to `invalidate_all_caches()`.
- **REFACTOR**: Ensure all call sites use the updated signature if return type changes.
- **CLEANUP**: Clippy clean.

### Cycle 2: P-06 — find_parent_entities bypasses cached graph (propagate.rs:65-88)

**ROOT CAUSE:**
`flush_refresh_queue` loads `graph` once via `graph_cache::load_cached()`.
`find_parent_entities` is called once per processed key in `refresh_and_get_parents`,
but it issues a raw SPI query instead of reading from `graph.parents`. For 50 distinct
keys across 3 entity types: 50 avoidable SPI roundtrips.

- **RED**: Write a test: call `find_parent_entities` (or its caller) twice for the same
  entity and assert only one SPI query is issued (the one that loads the graph).
- **GREEN**: Thread `graph: &EntityDepGraph` parameter through the call chain:
  ```rust
  fn refresh_and_get_parents(
      key: &RefreshKey,
      graph: &EntityDepGraph,
      // ...
  ) -> TViewResult<Vec<RefreshKey>>

  fn find_parents_for(key: &RefreshKey, graph: &EntityDepGraph) -> TViewResult<Vec<RefreshKey>> {
      let parent_entities = graph.parents.get(&key.entity).cloned().unwrap_or_default();
      // Build RefreshKeys from parent_entities (still needs SPI to find affected PKs)
  }
  ```
  Delete the raw SPI query from `find_parent_entities`.
- **REFACTOR**: Ensure `graph` is passed from `flush_refresh_queue` all the way down
  through `refresh_and_get_parents` → `find_parents_for` without re-loading.
- **CLEANUP**: Delete the now-unused SPI query in propagate.rs; clippy clean.

### Cycle 3: P-07 — find_affected_pks unbatched (propagate.rs:94-123)

**ROOT CAUSE:**
For each changed key, `find_affected_pks(parent_entity, child_entity, child_pk)` issues
one query per child PK. 30 changed users → 30 queries of the form
`SELECT pk_post FROM tv_post WHERE fk_user = $1`.
A single `= ANY($1)` query retrieves all affected post PKs in one round trip.

- **RED**: Write a pgtest: UPDATE 30 rows in tb_user. Check query count in pg_stat_activity
  or use a counter; assert find_affected_pks issues 1 query (not 30) for the user→post
  edge.
- **GREEN**: Add a batch variant called once per entity-pair in `flush_refresh_queue`.
  **Important:** quote all identifiers with `quote_identifier` (from Phase 3 P-04) to
  avoid the same unquoted-identifier bug fixed in F-11:
  ```rust
  fn find_affected_pks_batch(
      parent_entity: &str,
      child_entity: &str,
      child_pks: &[i64],
  ) -> spi::Result<Vec<(i64, i64)>> { // (child_pk, parent_pk)
      let fk_col = quote_identifier(&format!("fk_{child_entity}"));
      let parent_table = quote_identifier(&format!("tv_{parent_entity}"));
      let pk_col = quote_identifier(&format!("pk_{parent_entity}"));
      Spi::connect(|client| {
          client.select(
              &format!("SELECT {fk_col}, {pk_col} FROM {parent_table} WHERE {fk_col} = ANY($1)"),
              Some(1),
              Some(vec![child_pks_array_datum]),
          )
      })
  }
  ```
  In `flush_refresh_queue`, group all child PKs by entity-pair and call the batch variant
  once per pair.
- **REFACTOR**: Delete the single-PK `find_affected_pks`; update callers.
- **CLEANUP**: Clippy clean; confirm correctness against single-key edge case.

### Cycle 4: P-08 — apply_full_replacement queries pg_attribute per invocation (refresh/main.rs:576-596)

**ROOT CAUSE:**
`apply_full_replacement` and `get_view_columns` (for the DISTINCT ON path) each query
`pg_attribute` on every invocation to get column names. Column sets only change on
`ALTER TABLE … ADD/DROP COLUMN` (interceptable) or extension upgrade.
N refreshed rows → N identical catalog queries.

- **RED**: Write a test: call `apply_full_replacement` twice for the same view; assert
  `pg_attribute` is queried exactly once.
- **GREEN**:
  ```rust
  static COLUMN_CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
      LazyLock::new(|| Mutex::new(HashMap::new()));

  fn get_view_columns_cached(view_name: &str) -> spi::Result<Vec<String>> {
      {
          let cache = COLUMN_CACHE.lock().unwrap();
          if let Some(cols) = cache.get(view_name) {
              return Ok(cols.clone());
          }
      }
      let cols = get_view_columns(view_name)?; // existing slow path
      COLUMN_CACHE.lock().unwrap().insert(view_name.to_string(), cols.clone());
      Ok(cols)
  }
  ```
  Replace direct `get_view_columns` / `pg_attribute` calls in `apply_full_replacement`
  and `refresh_by_dedup_key` with `get_view_columns_cached`.
  Add cache clear to `invalidate_all_caches()`.
- **REFACTOR**: Unify the two places that query `pg_attribute` for column names into a
  single `get_view_columns` implementation.
- **CLEANUP**: Clippy clean.

### Cycle 5: P-10 — flush-loop HashMap/HashSet allocated with capacity 0 (queue/xact.rs:220,234)

**ROOT CAUSE:**
`processed: HashSet` initialized with capacity 0 before the outer loop; `keys_by_entity:
HashMap` initialized with capacity 0 (default) inside the inner loop. Both reallocate
repeatedly for typical flush sizes (tens to hundreds of keys).

- **RED**: Code-level test: confirm that after the fix, the initial capacity of `processed`
  is at least `pending.len()` (assert via debug instrumentation or just review).
- **GREEN**:
  ```rust
  let mut processed = HashSet::with_capacity(pending.len() * 2);
  // inside inner loop:
  let mut keys_by_entity: HashMap<String, Vec<RefreshKey>> =
      HashMap::with_capacity(sorted_keys.len().min(32));
  ```
- **REFACTOR**: No structural change needed.
- **CLEANUP**: Clippy clean.

### Cycle 6: P-11 — refresh_by_dedup_key issues COUNT then separate DML (refresh/main.rs:153-200)

**ROOT CAUSE:**
`refresh_by_dedup_key` runs a `COUNT(*)` query first, then conditionally a DELETE or
UPSERT. Always two round trips; could be one or two with smarter SQL.

- **RED**: Write a test that calls `refresh_by_dedup_key` for an existing row and a
  deleted row and asserts query count is ≤2 in each case (down from guaranteed 2).
- **GREEN**: Restructure to:
  1. Always issue the UPSERT from the view (inserts/updates if view has data, no-ops if empty).
  2. Issue a `DELETE … WHERE NOT EXISTS (SELECT 1 FROM v_entity WHERE key_col = $1)`.
  This is two queries but eliminates the COUNT, and the UPSERT no-ops cheaply when the
  view returns no rows.
  Alternatively, use a single CTE that does both in one statement.
- **REFACTOR**: Simplify the surrounding conditional logic once the COUNT is removed.
- **CLEANUP**: Clippy clean; verify the DELETE no-ops correctly when the UPSERT succeeded.

## Dependencies

- Requires: Phase 3 complete (quote_identifier consolidated; TABLE_ENTITY_CACHE extended)
- Blocks: Phase 5

## Status

[ ] Not Started
