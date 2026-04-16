# pg_tviews 0.1.0 — Performance Audit Report

**Date:** 2026-04-16T21:56:31Z  
**Auditor:** Claude Sonnet 4.6 (claude-sonnet-4-6)  
**Codebase:** `/home/lionel/code/pg_tviews` @ commit `709d517`  
**Companion:** Security audit `20260416T214944Z-security-audit.md`

---

## Scope

Full `src/` tree, with emphasis on the hot paths:

- Row-level trigger → enqueue (`src/trigger.rs`)
- Commit-time flush (`src/queue/xact.rs`, `src/queue/ops.rs`)
- Refresh engine (`src/refresh/main.rs`, `src/refresh/bulk.rs`)
- Propagation (`src/propagate.rs`)
- Catalog / caches (`src/catalog.rs`, `src/queue/cache.rs`, `src/queue/graph.rs`)
- Identifier quoting (`src/refresh/bulk.rs:134`, `src/refresh/cache.rs:115`)

---

## Finding P-01 · HIGH · Hot Path: Per-row Trigger

**Uncached `TviewMeta::load_by_entity` on every trigger row for DISTINCT ON check**

**Location:** `src/trigger.rs:50`

**Description:**
Every row fired by `pg_tview_trigger_handler` executes this sequence:

```rust
// trigger.rs:47-84
match entity_for_table(table_oid) {       // cache hit after first call
    Ok(Some(entity)) => {
        match TviewMeta::load_by_entity(&entity) {  // ← FULL SPI ROUNDTRIP EVERY ROW
            Ok(Some(meta)) if meta.is_distinct_on() => { … }
```

`TviewMeta::load_by_entity` issues a full `SELECT … FROM pg_tview_meta WHERE entity = $1` on every single row trigger invocation. For a `UPDATE tb_post … WHERE TRUE` on a table with 50,000 rows, this generates **50,000 identical SPI roundtrips** just to answer the question "is this entity DISTINCT ON?".

The `table_cache` already caches the `table_oid → entity_name` mapping. The `is_distinct_on` flag and `distinct_on_keys[0]` column are equally stable — they do not change unless a TVIEW is dropped and recreated.

**Fix:** Extend `TABLE_ENTITY_CACHE` to store a small struct instead of a bare `String`:

```rust
struct EntityInfo {
    name: String,
    distinct_on_key: Option<String>, // None → standard PK-based
}
```

On cache miss, load both the entity name and the distinct-on key in a single query:

```sql
SELECT entity, distinct_on_keys[1] AS dedup_col
FROM pg_tview_meta WHERE entity = $1
```

The trigger hot path then needs zero SPI calls after the first row (both the entity name and the dispatch mode are in the cache).

---

## Finding P-02 · HIGH · Hot Path: Per-row Refresh

**`check_jsonb_delta_available()` issues an uncached `pg_extension` query on every `apply_patch` call**

**Location:** `src/refresh/main.rs:378`

**Description:**
`apply_patch` is called once per refreshed row. For every invocation it calls:

```rust
if !check_jsonb_delta_available()? {
    return apply_full_replacement(row);
}
```

`check_jsonb_delta_available()` opens an SPI connection and runs:

```sql
SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')
```

This is a fixed property of the database installation. It cannot change without a `CREATE/DROP EXTENSION` DDL statement, which is rare and would need to trigger cache invalidation. For a cascade that refreshes 200 rows, this adds 200 unnecessary catalog queries.

**Fix:** Cache the result in a session-local `OnceLock<bool>`. Invalidate on DDL (via the existing `invalidate_all_caches()` hook, or by re-checking when a `CREATE/DROP EXTENSION` is detected in `tview_process_utility_hook`):

```rust
static JSONB_DELTA_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn check_jsonb_delta_available() -> spi::Result<bool> {
    if let Some(&cached) = JSONB_DELTA_AVAILABLE.get() {
        return Ok(cached);
    }
    // slow path: single query on first call
    let result = Spi::connect(|client| { … })?;
    let _ = JSONB_DELTA_AVAILABLE.set(result); // ignore if already set
    Ok(result)
}
```

---

## Finding P-03 · HIGH · Hot Path: Per-row Refresh

**Double metadata load per refresh: `load_for_source` in `refresh_pk` + `load_for_tview` in `apply_patch`**

**Location:** `src/refresh/main.rs:102` and `src/refresh/main.rs:369`

**Description:**
`refresh_pk(source_oid, pk)` follows this call chain:

```
refresh_pk
  └─ TviewMeta::load_for_source(source_oid)   → SPI query #1
  └─ recompute_view_row(&meta, pk)
  └─ apply_patch(&view_row)
        └─ TviewMeta::load_for_tview(row.tview_oid)  → SPI query #2 (same row!)
```

`apply_patch` receives a `ViewRow` which already carries `tview_oid`. The metadata loaded in `refresh_pk` is discarded before passing to `apply_patch`. For every single-row refresh, two identical SPI roundtrips to `pg_tview_meta` are issued.

For a cascade of 100 rows this costs 200 metadata queries instead of 100.

**Fix:** Thread `meta` through `ViewRow` or pass it as a separate parameter to `apply_patch`:

```rust
// Option A: embed in ViewRow
pub struct ViewRow {
    pub entity_name: String,
    pub pk: i64,
    pub tview_oid: Oid,
    pub data: JsonB,
    pub meta: TviewMeta,    // ← add
}

fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    // Use row.meta directly — no reload
    let deps = row.meta.parse_dependencies();
    …
}
```

---

## Finding P-04 · HIGH · Hot Path: Identifier Quoting

**`quote_identifier` issues an SPI call on every invocation; pure-Rust fallback is correct and already present**

**Location:** `src/refresh/bulk.rs:134–143`, `src/refresh/cache.rs:115–123`

**Description:**
`quote_identifier` is defined in two places (duplication is a separate quality issue) and both call `SELECT quote_ident($1)` via SPI to quote an identifier:

```rust
pub fn quote_identifier(name: &str) -> String {
    let quote_args = vec![unsafe { DatumWithOid::new(name, …TEXTOID…) }];
    match Spi::get_one_with_args::<String>("SELECT quote_ident($1)", &quote_args) {
        Ok(Some(quoted)) => quoted,
        _ => format!("\"{}\"", name.replace('"', "\"\"")),  // ← pure-Rust fallback
    }
}
```

The pure-Rust fallback is semantically correct for every identifier that can appear in this codebase (entity names, column names, table names — all `\w+`-constrained). It matches the output of PostgreSQL's `quote_ident` for all ASCII input containing no null bytes. The SPI route wrapping it adds network/SPI overhead for zero benefit.

`quote_identifier` is called in:
- `refresh_bulk`: 4 calls per bulk refresh (pk col × 2, data col, table name, update table)
- `extract_pks_from_transition_table` (via `trigger.rs:235`): 1 call per statement trigger
- `get_or_prepare_statement` in `refresh/cache.rs`: 2 calls per cache miss

Each of these is in a hot path; eliminating the SPI call removes one of the cheapest but most-called bottlenecks.

**Fix:** Delete the SPI call; use the fallback unconditionally:

```rust
pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
```

Consolidate the two duplicate definitions into `src/utils.rs` and import from there.

---

## Finding P-05 · MEDIUM · Hot Path: OID Resolution

**`relname_from_oid` issues an uncached `pg_class` query on every call**

**Location:** `src/utils.rs:167`, called from `apply_patch:365`, `apply_full_replacement:566`, `refresh_by_dedup_key:147`, `refresh/bulk.rs:148`

**Description:**
`relname_from_oid(oid)` runs `SELECT relname FROM pg_class WHERE oid = $1` on every call. It is not cached. In `apply_patch` alone it is called once for `tv_name` (line 365), and then again inside `apply_full_replacement` if the smart-patch path is unavailable (line 566). `refresh_by_dedup_key` calls it twice (lines 147–148: `view_name` and `tv_name`).

OID→relname mappings are stable within a session; they only change when DDL runs (which is already tracked via the ProcessUtility hook). For a cascade refreshing 200 rows, up to 200 × 2 = 400 catalog queries are issued just to resolve the same two OIDs over and over.

**Fix:** Add an OID-keyed relname cache parallel to `TABLE_ENTITY_CACHE`:

```rust
static OID_RELNAME_CACHE: LazyLock<Mutex<HashMap<Oid, String>>> = …;
```

Invalidate in `invalidate_all_caches()`. The lookup becomes a hash table read on the hot path.

---

## Finding P-06 · MEDIUM · Flush Phase: Propagation

**`find_parent_entities` bypasses the already-cached `EntityDepGraph`; raw SPI query per processed key**

**Location:** `src/propagate.rs:65–88`

**Description:**
`flush_refresh_queue` loads the dependency graph once:

```rust
// queue/xact.rs:217
let graph = super::cache::graph_cache::load_cached()?;
```

`graph.parents` is a `HashMap<String, Vec<String>>` that maps each entity to the entities that depend on it — exactly the data that `find_parent_entities` queries via SPI:

```rust
// propagate.rs:71
let query = format!(
    "SELECT entity FROM public.pg_tview_meta WHERE '{fk_col}' = ANY(fk_columns)"
);
Spi::connect(|client| { … })
```

But `find_parent_entities` does not use the cached graph. It is called once per processed key in `refresh_and_get_parents` (and once per entity-group key in the bulk path). For a flush of 50 distinct keys across 3 entity types, this adds 50 avoidable SPI roundtrips.

Note also: the format-string interpolation of `fk_col` (line 71) is not parameterized, though the value is metadata-derived and not injection-unsafe (see security audit F-15 analogous reasoning). A parameterized query would be cleaner regardless.

**Fix:** Thread `graph` down to `refresh_and_get_parents` and `find_parents_for`:

```rust
fn refresh_and_get_parents(
    key: &RefreshKey,
    graph: &EntityDepGraph,
) -> TViewResult<Vec<RefreshKey>> {
    …
    let parent_keys = find_parents_for(key, graph)?;
    …
}

pub fn find_parents_for(
    key: &RefreshKey,
    graph: &EntityDepGraph,
) -> TViewResult<Vec<RefreshKey>> {
    // O(1) HashMap lookup — no SPI
    let parent_entities = graph.parents.get(&key.entity).cloned().unwrap_or_default();
    …
}
```

---

## Finding P-07 · MEDIUM · Flush Phase: Propagation

**`find_affected_pks` issues one query per changed key; not batched across keys of the same entity**

**Location:** `src/propagate.rs:94–123`

**Description:**
For each changed key, `find_parents_for` → `find_affected_pks(parent_entity, child_entity, child_pk)` issues:

```sql
SELECT pk_post FROM tv_post WHERE fk_user = $1
```

If 30 users are changed in one transaction (e.g., `UPDATE tb_user SET name = … WHERE region = 'EU'`), the flush loop calls `find_affected_pks("post", "user", pk)` **30 times**, issuing 30 sequential queries to the same table with a different `$1` each time.

A single batched query with `= ANY($1)` would retrieve all affected post PKs in one round trip:

```sql
SELECT DISTINCT pk_post FROM tv_post WHERE fk_user = ANY($1)
```

This is already the pattern used in `refresh_bulk` (line 61–66 of `bulk.rs`). Propagation should use the same approach.

**Fix:** Add a batch variant:

```rust
fn find_affected_pks_batch(
    parent_entity: &str,
    child_entity: &str,
    child_pks: &[i64],
) -> spi::Result<HashMap<i64, Vec<i64>>> {
    // Returns Map<child_pk → Vec<parent_pk>>
    let fk_col = format!("fk_{child_entity}");
    let parent_table = format!("tv_{parent_entity}");
    let parent_pk_col = format!("pk_{parent_entity}");
    let query = format!(
        "SELECT {fk_col}, {parent_pk_col} FROM {parent_table} WHERE {fk_col} = ANY($1)"
    );
    …
}
```

In `flush_refresh_queue`, after grouping by entity, collect all child PKs for each entity pair and call the batch variant once.

---

## Finding P-08 · MEDIUM · Refresh Path: Column Discovery

**`apply_full_replacement` queries `pg_attribute` on every invocation to get column names**

**Location:** `src/refresh/main.rs:576–596`

**Description:**
The full-replacement path (used when `jsonb_delta` is unavailable or no deps exist) fetches column names on every call:

```rust
let col_names: Vec<String> = Spi::connect(|client| {
    let rows = client.select(
        "SELECT a.attname::text FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         WHERE c.relname = $1 AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY a.attnum",
        …
    )?;
    …
})?;
```

`get_view_columns` in `refresh_by_dedup_key` (line 206–228) is the same pattern for the DISTINCT ON path. Column sets only change if a DDL `ALTER TABLE … ADD/DROP COLUMN` runs (interceptable via the ProcessUtility hook) or if the extension is upgraded.

For N rows refreshed via the full-replacement path, this generates N identical `pg_attribute` queries.

**Fix:** Add a `static LazyLock<Mutex<HashMap<String, Vec<String>>>>` column-name cache keyed by view name (or view OID). Invalidate in `invalidate_all_caches()`. The cache miss path is the current `pg_attribute` query; hits are a hash lookup.

---

## Finding P-09 · LOW · Queue Sort: Dedup Keys Silently Corrupted

**`sort_keys` converts dedup `RefreshKey`s into `pk=0` keys, corrupting the dedup-key identity**

**Location:** `src/queue/graph.rs:98–118`

**Description:**
`sort_keys` groups keys by entity and reconstructs them:

```rust
for key in keys {
    groups.entry(key.entity.clone())
        .or_default()
        .push(key.pk);   // ← key.pk == 0 for dedup keys
}

for entity in &self.topo_order {
    if let Some(pks) = groups.get(entity) {
        for pk in pks {
            sorted_keys.push(super::key::RefreshKey::pk(entity, *pk)); // ← always pk(…, 0)
        }
    }
}
```

`RefreshKey::dedup` sets `pk = 0`. After sorting, the output contains `RefreshKey::pk(entity, 0)` instead of the original `RefreshKey::dedup(entity, dedup_val)`. The `dedup_key` field is lost. When `refresh_and_get_parents` later receives `pk=0`, it calls `refresh_pk(view_oid, 0)` which will fail or produce incorrect results.

This is simultaneously a correctness bug and a performance concern (unnecessary work done before the error or silent wrong output).

**Fix:** Preserve the full `RefreshKey` through the sort, not just the `pk`:

```rust
pub fn sort_keys(&self, keys: Vec<RefreshKey>) -> Vec<RefreshKey> {
    let mut groups: HashMap<String, Vec<RefreshKey>> = HashMap::new();
    for key in keys {
        groups.entry(key.entity.clone()).or_default().push(key);
    }
    let mut sorted_keys = Vec::new();
    for entity in &self.topo_order {
        if let Some(entity_keys) = groups.remove(entity) {
            sorted_keys.extend(entity_keys);
        }
    }
    sorted_keys
}
```

---

## Finding P-10 · LOW · Flush Loop: Redundant Allocations

**`keys_by_entity` `HashMap` and `processed` `HashSet` growth are unguided; `with_capacity` missing**

**Location:** `src/queue/xact.rs:234, 220`

**Description:**
Inside the inner flush loop, a new `HashMap<String, Vec<RefreshKey>>` is created on every iteration:

```rust
let mut keys_by_entity: std::collections::HashMap<String, Vec<RefreshKey>> =
    std::collections::HashMap::new();   // ← default capacity (1)
```

`sorted_keys` is typically O(queue_size) entries. The `HashMap` will reallocate multiple times as entries are inserted. Similarly, `processed` is initialized with `HashSet::new()` (capacity 0) outside the loop but grows by double on each reallocation.

For typical cascades (tens to hundreds of keys) this is minor. For the DoS scenario in security finding F-09 (large queues), allocation churn amplifies the problem.

**Fix:**

```rust
// Before the outer loop:
let mut processed = std::collections::HashSet::with_capacity(pending.len() * 2);

// Inside inner loop:
let initial_len = sorted_keys.len();
let mut keys_by_entity: HashMap<String, Vec<RefreshKey>> =
    HashMap::with_capacity(initial_len.min(32));
```

---

## Finding P-11 · LOW · Dedup-key Refresh: Two Round Trips Where One Suffices

**`refresh_by_dedup_key` issues a `COUNT(*)` then a separate UPSERT/DELETE; redundant round trip**

**Location:** `src/refresh/main.rs:153–200`

**Description:**
`refresh_by_dedup_key` fetches a row count first, then conditionally executes a DELETE or UPSERT:

```rust
// Round trip 1: count rows
let row_count: i64 = Spi::connect(|client| {
    client.select("SELECT COUNT(*) FROM {view_name} WHERE {key_col}::text = $1", …)
})?;

// Round trip 2 (conditional): DELETE or UPSERT
if row_count == 0 {
    Spi::run_with_args("DELETE FROM {tv_name} WHERE {key_col}::text = $1", …)?;
} else {
    Spi::run_with_args("INSERT INTO {tv_name} … ON CONFLICT … DO UPDATE …", …)?;
}
```

The DELETE + UPSERT can be collapsed into a single statement using a CTE or by relying on the UPSERT's own empty-result semantics. Alternatively, skip the COUNT and use:

```sql
-- Single statement: UPSERT from view (inserts/updates if rows exist, no-ops if empty)
INSERT INTO tv_entity (…)
SELECT … FROM v_entity WHERE key_col::text = $1 LIMIT 1
ON CONFLICT (key_col) DO UPDATE SET …;

-- Then DELETE rows that no longer have a backing-view match:
DELETE FROM tv_entity t
WHERE key_col::text = $1
  AND NOT EXISTS (SELECT 1 FROM v_entity v WHERE v.key_col::text = $1);
```

Or combine into one CTE:

```sql
WITH src AS (
    SELECT … FROM v_entity WHERE key_col::text = $1 LIMIT 1
)
INSERT INTO tv_entity (…) SELECT … FROM src
ON CONFLICT (key_col) DO UPDATE SET …
```

And issue a separate `DELETE WHERE NOT EXISTS` when the view is empty. This reduces the path to one or two queries regardless of existence, compared to the current guaranteed two queries.

---

## Finding P-12 · LOW · Prepared Statement Cache: Correctness-Defeating Validation Query

**`refresh/cache.rs:get_or_prepare_statement` validates cache entries with a `pg_prepared_statements` query, defeating its own fast path**

**Location:** `src/refresh/cache.rs:62–74`

**Description:**
`get_or_prepare_statement` stores statement names in a `LazyLock<Mutex<HashMap>>`. On cache hit:

```rust
if let Some(stmt_name) = cache.get(entity) {
    let exists = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_prepared_statements WHERE name = $1)",
        &exists_args,
    )?.unwrap_or(false);
    if exists { return Ok(stmt_name.clone()); }
    cache.remove(entity);
}
```

Every cache hit causes a `pg_prepared_statements` lookup to verify the statement still exists. This negates most of the caching benefit. Prepared statements are session-scoped; they are only invalidated when `DEALLOCATE`, `DISCARD PREPARED`, or `DISCARD ALL` runs. A simpler approach is to rely on the PostgreSQL error that fires when `EXECUTE` of a missing statement is called, catch that error, and re-prepare.

Note: this module (`refresh/cache.rs`) is currently entirely dead code (security audit F-12). These findings are recorded for when it is activated.

**Fix (if/when module is activated):** Remove the per-hit `pg_prepared_statements` check. Trust that the prepared statement exists once registered; re-prepare only on `EXECUTE` error.

---

## Summary Table

| # | Severity | Hot Path | Location | One-line description |
|---|----------|----------|----------|----------------------|
| P-01 | **HIGH** | Trigger | `trigger.rs:50` | `TviewMeta::load_by_entity` called per row for DISTINCT ON check; should be cached in `table_cache` |
| P-02 | **HIGH** | Refresh | `refresh/main.rs:378` | `check_jsonb_delta_available()` queries `pg_extension` per refreshed row; needs `OnceLock<bool>` |
| P-03 | **HIGH** | Refresh | `refresh/main.rs:102,369` | Double metadata load: `load_for_source` + `load_for_tview` per refresh; thread `meta` through |
| P-04 | **HIGH** | Quoting | `refresh/bulk.rs:134`, `refresh/cache.rs:115` | `quote_identifier` invokes SPI every call; pure-Rust fallback is correct and already present |
| P-05 | MEDIUM | Refresh | `utils.rs:167` | `relname_from_oid` uncached; `pg_class` query per refresh per OID; add OID→relname cache |
| P-06 | MEDIUM | Flush | `propagate.rs:65` | `find_parent_entities` bypasses cached `EntityDepGraph`; raw SPI per processed key |
| P-07 | MEDIUM | Flush | `propagate.rs:94` | `find_affected_pks` unparameterized per key; batch with `= ANY($1)` across same-entity keys |
| P-08 | MEDIUM | Refresh | `refresh/main.rs:576` | `apply_full_replacement` queries `pg_attribute` per invocation; column list should be cached |
| P-09 | LOW | Sort | `queue/graph.rs:98` | `sort_keys` reconstructs keys as `pk(…, 0)`, destroying dedup-key identity — correctness + perf |
| P-10 | LOW | Flush | `queue/xact.rs:234` | `HashMap`/`HashSet` allocated with capacity 0 in inner flush loop; use `with_capacity` |
| P-11 | LOW | Refresh | `refresh/main.rs:153` | `refresh_by_dedup_key` does COUNT then DELETE/UPSERT (2 round trips); fold into 1 |
| P-12 | LOW | Cache | `refresh/cache.rs:62` | Prepared-stmt cache validates each hit with `pg_prepared_statements` query, defeating the cache |

---

## Highest-Impact Fix Order

For fastest wall-clock improvement on common workloads:

1. **P-04** — zero-cost fix (remove SPI call from `quote_identifier`); affects every bulk refresh and every statement trigger
2. **P-02** — one-line fix (`OnceLock`); eliminates one SPI per refreshed row
3. **P-03** — thread `meta` through the call stack; halves metadata SPI queries
4. **P-01** — extend `table_cache` entry to hold DISTINCT ON info; eliminates N metadata queries from bulk-DML trigger hot path
5. **P-06 + P-07** — pass `graph` to propagation, batch `find_affected_pks`; reduces flush-time SPI from O(N×parents) to O(entities)
6. **P-05 + P-08** — OID and column-name caches; eliminates `pg_class` / `pg_attribute` queries across session
7. **P-09** — correctness fix that also removes erroneous zero-PK refresh attempts
