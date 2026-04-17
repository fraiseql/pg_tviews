# Phase 3: Audit Logging & Cache Improvements

## Objective

1. Make audit logging conditional and batch-friendly to remove SPI overhead from
   the per-row refresh hot path.
2. Cache negative lookups in `entity_info_cached` to avoid repeated SPI queries
   for non-TVIEW tables.

## Why This Matters

**Audit logging**: Every `refresh_pk` call currently issues 2 SPI queries for audit
(one for `session_user()`, one for `INSERT INTO pg_tview_audit_log`). In a transaction
touching 500 TVIEW rows, that's 1000 extra SPI calls — potentially doubling refresh
latency. The `refresh_bulk` path doesn't log at all, creating inconsistency.

**Negative cache**: Triggers fire on every DML statement on every table that has a
trigger installed. Tables like `tb_comment` (which has triggers but no direct TVIEW)
hit `entity_for_table_uncached` on every trigger invocation. The query returns `None`
every time, but the result is never cached.

## Success Criteria

- [ ] Audit logging gated behind `pg_tviews.audit_enabled` GUC (default: true for
  backward compatibility)
- [ ] Audit logging batched: `flush_refresh_queue` logs once per entity with total
  row count, not once per row
- [ ] Negative lookups cached in `TABLE_ENTITY_CACHE`
- [ ] `cargo clippy --no-default-features --features pg18` clean
- [ ] `cargo test` passes

## Files To Change

### Part A: Conditional & Batched Audit Logging

#### 1. `src/config/mod.rs` — Add `audit_enabled` GUC

```rust
static AUDIT_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);

// In register_gucs():
GucRegistry::define_bool_guc(
    c"pg_tviews.audit_enabled",
    c"Enable audit logging of TVIEW operations to pg_tview_audit_log.",
    c"When false, refresh/create/drop operations are not logged.",
    &AUDIT_ENABLED_GUC,
    GucContext::Userset,
    GucFlags::default(),
);

pub fn audit_enabled() -> bool {
    AUDIT_ENABLED_GUC.get()
}
```

#### 2. `src/audit.rs` — Guard all logging functions

Wrap the body of `log_create`, `log_drop`, and `log_refresh` with an early return:

```rust
pub fn log_refresh(entity: &str, rows_affected: i64) -> spi::Result<()> {
    if !crate::config::audit_enabled() {
        return Ok(());
    }
    // ... existing implementation
}
```

#### 3. `src/refresh/main.rs` — Remove per-row audit calls

**Line 115** — `refresh_pk`: Remove `crate::audit::log_refresh(...)` call.
**Line 225** — `refresh_by_dedup_key`: Remove `crate::audit::log_refresh(...)` call.

#### 4. `src/queue/xact.rs` — Add batched audit at flush time

In `flush_refresh_queue`, after the outer drain loop completes successfully,
log a single audit entry per entity with the aggregated row count.

The variable `processed` is a `HashSet<RefreshKey>` (defined at line 220).
`RefreshKey` has a `pub entity: String` field (defined in `src/queue/key.rs:11`).

Insert this block **before** the "Record metrics" block (line 313), after the
drain loop's closing brace:

```rust
// Batched audit: one log entry per entity instead of per-row
if crate::config::audit_enabled() {
    let mut entity_counts: std::collections::HashMap<&str, i64> =
        std::collections::HashMap::new();
    for key in &processed {
        *entity_counts.entry(&key.entity).or_insert(0) += 1;
    }
    for (entity, count) in entity_counts {
        if let Err(e) = crate::audit::log_refresh(entity, count) {
            warning!("Failed to log refresh audit for '{}': {}", entity, e);
        }
    }
}
```

This reduces audit SPI calls from `N` (one per row) to `E` (one per entity,
typically 3-10).

### Part B: Negative Cache for `entity_info_cached`

#### 5. `src/queue/cache.rs` — Cache `None` results

The current code only caches `Some(info)`. Change the cache type to store
`Option<CachedEntityInfo>` and cache `None` as "known-absent":

```rust
// Change the cache type:
static TABLE_ENTITY_CACHE: LazyLock<Mutex<HashMap<pg_sys::Oid, Option<CachedEntityInfo>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn entity_info_cached(
    table_oid: pg_sys::Oid,
) -> crate::TViewResult<Option<CachedEntityInfo>> {
    if !crate::config::table_cache_enabled() {
        return load_entity_info_uncached(table_oid);
    }

    // Fast path: check cache (now distinguishes "cached None" from "not in cache")
    {
        let cache = TABLE_ENTITY_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cached_value) = cache.get(&table_oid) {
            crate::metrics::metrics_api::record_table_cache_hit();
            return Ok(cached_value.clone());
        }
    }

    // Slow path: query and cache (including None)
    crate::metrics::metrics_api::record_table_cache_miss();
    let info = load_entity_info_uncached(table_oid)?;

    {
        let mut cache = TABLE_ENTITY_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        cache.insert(table_oid, info.clone());
    }

    Ok(info)
}
```

**Update `entity_for_table_cached`** — the return mapping stays the same since
`info.map(|i| i.name)` works on both `Some` and `None`.

**Update `invalidate`** — stays the same (`cache.clear()`).

**Update test** — `test_table_cache_invalidation` needs to insert `Some(CachedEntityInfo{...})`
instead of bare `CachedEntityInfo{...}`.

## TDD Cycles

### Cycle 1: Add `audit_enabled` GUC

- **RED**: Write a unit test in `config/mod.rs::tests` that calls `audit_enabled()`
  and verifies it returns `true` by default.
- **GREEN**: Add the GUC static, register it, add the accessor.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 2: Guard audit functions + remove per-row calls

- **RED**: `test_refresh_logging` pg_test expects audit entries — it will still pass
  because `audit_enabled` defaults to true. Verify `cargo test` passes.
- **GREEN**: Add early-return guards to `log_create`, `log_drop`, `log_refresh`.
  Remove the per-row `log_refresh` calls from `refresh_pk` and `refresh_by_dedup_key`.
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 3: Add batched audit in `flush_refresh_queue`

- **RED**: The `test_refresh_logging` pg_test now won't find audit entries because
  per-row logging was removed and batched logging isn't added yet. This test runs
  outside the queue flush path (calls `refresh_pk` directly), so we need to either:
  (a) update the test to go through the queue, or (b) keep the test as-is and accept
  it now tests the "no audit when called outside flush" behavior. Option (b) is simpler.
- **GREEN**: Add the batched audit block in `flush_refresh_queue`. Update the
  `test_refresh_logging` test to reflect new behavior (audit entries come from
  queue flush, not from direct `refresh_pk` calls).
- **REFACTOR**: None
- **CLEANUP**: `cargo clippy`, `cargo test`

### Cycle 4: Negative cache

- **RED**: Write a unit test in `cache.rs::tests`:
  ```rust
  #[test]
  fn test_negative_cache_entry() {
      table_cache::invalidate();
      // Insert a None entry
      TABLE_ENTITY_CACHE.lock().unwrap().insert(pg_sys::Oid::from(999), None);
      // Verify it's cached as None (not a cache miss)
      let cache = TABLE_ENTITY_CACHE.lock().unwrap();
      assert!(cache.get(&pg_sys::Oid::from(999)).is_some()); // key exists
      assert!(cache.get(&pg_sys::Oid::from(999)).unwrap().is_none()); // value is None
  }
  ```
- **GREEN**: Change `TABLE_ENTITY_CACHE` type to `HashMap<Oid, Option<CachedEntityInfo>>`.
  Update `entity_info_cached` to cache `None` results. Fix compile errors in
  existing code that reads from the cache.
- **REFACTOR**: Update `test_table_cache_invalidation` to match new type.
- **CLEANUP**: `cargo clippy`, `cargo test`

## Verification

```bash
cargo clippy --no-default-features --features pg18 -- -D warnings
cargo test

# Verify no per-row audit calls remain in refresh:
grep -n 'audit::log_refresh' src/refresh/ --include='*.rs'
# Should return zero results

# Verify batched audit is in flush:
grep -n 'audit::log_refresh' src/queue/xact.rs
# Should return the batched block
```

## Dependencies

- Requires: Phase 2 (uses consolidated helpers)
- Blocks: Phase 5 (finalize)

## Status
[ ] Not Started
