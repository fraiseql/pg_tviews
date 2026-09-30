use pgrx::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::cascade_path::CascadePath;

/// Cached information for a table managed by `pg_tviews`.
///
/// Beyond the entity name and DISTINCT ON key, this carries everything the issue
/// #56 direct-patch eligibility check needs, so the row trigger can decide the fast
/// path from a single cached lookup with **no SPI in the hot path** (populated once
/// per session on cache miss, invalidated on DDL).
#[derive(Clone, Debug)]
pub struct CachedEntityInfo {
    pub name: String,
    /// First DISTINCT ON key if this is a DISTINCT ON TVIEW, None otherwise
    pub distinct_on_key: Option<String>,

    /// Direct-patch column→key map (issue #56): base column name → JSONB key it
    /// feeds in the entity's own `data`. Empty ⇒ the fast path never engages.
    pub direct_map: HashMap<String, String>,

    /// Integer FK columns of this entity's base table. A changed FK is a
    /// membership change ⇒ the fast path must decline (issue #56 eligibility).
    pub fk_columns: Vec<String>,

    /// UUID FK columns of this entity's base table (same membership rule).
    pub uuid_fk_columns: Vec<String>,

    /// Output columns the `tv_<entity>` table materialises (`pk_<entity>`, `id`,
    /// `data`, and any column projected outside `data`). A changed base column that
    /// is also a projected output column would go stale under a data-only patch ⇒
    /// the fast path declines (issue #56 eligibility).
    pub output_columns: Vec<String>,

    /// `true` when the backing view is a UNION / UNION ALL — different refresh
    /// machinery, so the fast path declines (issue #56 eligibility).
    pub is_union: bool,
}

/// Global cache for `EntityDepGraph` to avoid repeated `pg_tview_meta` queries
static ENTITY_GRAPH_CACHE: LazyLock<Mutex<Option<super::graph::EntityDepGraph>>> =
    LazyLock::new(|| Mutex::new(None));

/// Global cache for table OID → entity info (name + `distinct_on_key`)
/// Stores Option<CachedEntityInfo> to cache negative lookups (None results)
static TABLE_ENTITY_CACHE: LazyLock<Mutex<HashMap<pg_sys::Oid, Option<CachedEntityInfo>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Transaction-scoped cache for table OID → cascade paths.
// Cleared on transaction end to avoid stale data.
thread_local! {
    static CASCADE_PATH_CACHE: std::cell::RefCell<HashMap<pg_sys::Oid, Vec<CascadePath>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Cache operations for `EntityDepGraph`
pub mod graph_cache {
    #[allow(clippy::wildcard_imports)] // Reason: module-internal prelude import
    use super::*;

    /// Get cached `EntityDepGraph`, loading from database if not cached
    pub fn load_cached() -> crate::TViewResult<crate::queue::graph::EntityDepGraph> {
        // Check if caching is enabled
        if !crate::config::graph_cache_enabled() {
            return crate::queue::graph::EntityDepGraph::load();
        }

        let mut cache = ENTITY_GRAPH_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        if let Some(graph) = cache.as_ref() {
            // Cache hit
            crate::metrics::metrics_api::record_graph_cache_hit();
            return Ok(graph.clone());
        }

        // Cache miss: load from database
        crate::metrics::metrics_api::record_graph_cache_miss();
        let graph = crate::queue::graph::EntityDepGraph::load()?;
        *cache = Some(graph.clone());
        drop(cache);
        Ok(graph)
    }

    /// Invalidate the `EntityDepGraph` cache
    /// Should be called when TVIEWs are created or dropped
    pub fn invalidate() {
        let mut cache = ENTITY_GRAPH_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *cache = None;
    }
}

/// Cache operations for table OID → entity mapping
pub mod table_cache {
    #[allow(clippy::wildcard_imports)] // Reason: module-internal prelude import
    use super::*;

    /// Get cached entity info (name + `distinct_on_key`) for table OID
    /// Loads from database on first miss per session, caches negative results
    pub fn entity_info_cached(
        table_oid: pg_sys::Oid,
    ) -> crate::TViewResult<Option<CachedEntityInfo>> {
        // Check if caching is enabled
        if !crate::config::table_cache_enabled() {
            return load_entity_info_uncached(table_oid);
        }

        // Fast path: check cache (distinguishes cached None from not-in-cache)
        {
            let cache = TABLE_ENTITY_CACHE
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(cached_value) = cache.get(&table_oid) {
                crate::metrics::metrics_api::record_table_cache_hit();
                return Ok(cached_value.clone());
            }
        }

        // Slow path: query and cache (including None)
        crate::metrics::metrics_api::record_table_cache_miss();
        let info = load_entity_info_uncached(table_oid)?;

        {
            let mut cache = TABLE_ENTITY_CACHE
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            crate::utils::bound_cache(&mut cache);
            cache.insert(table_oid, info.clone());
        }

        Ok(info)
    }

    /// Get cached entity name (backward compatibility)
    pub fn entity_for_table_cached(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<String>> {
        entity_info_cached(table_oid).map(|info| info.map(|i| i.name))
    }

    /// Load entity info from the database on a cache miss.
    ///
    /// Loads the full `TviewMeta` for the entity plus the `tv_<entity>` output
    /// columns, so a single cached record answers every issue #56 eligibility
    /// question without further SPI in the trigger hot path.
    fn load_entity_info_uncached(
        table_oid: pg_sys::Oid,
    ) -> crate::TViewResult<Option<CachedEntityInfo>> {
        let Some(name) = crate::catalog::entity_for_table_uncached(table_oid)? else {
            return Ok(None);
        };

        // Name resolved but no meta row (shouldn't happen) → minimal safe info
        // with the fast path disabled (empty direct_map).
        let Some(meta) = crate::catalog::TviewMeta::load_by_entity(&name)? else {
            return Ok(Some(CachedEntityInfo {
                name,
                distinct_on_key: None,
                direct_map: HashMap::new(),
                fk_columns: Vec::new(),
                uuid_fk_columns: Vec::new(),
                output_columns: Vec::new(),
                is_union: false,
            }));
        };

        let mut direct_map: HashMap<String, String> = meta
            .direct_map_columns
            .iter()
            .cloned()
            .zip(meta.direct_map_keys.iter().cloned())
            .collect();

        // Output columns the tv_<entity> table materialises. If they can't be
        // determined we can't verify the "projected column" eligibility rule, so
        // disable the fast path for this entity rather than risk a stale column.
        let output_columns = if let Ok(cols) = crate::utils::get_view_columns_by_oid(meta.tview_oid)
        {
            cols
        } else {
            direct_map.clear();
            Vec::new()
        };

        // A base column that also feeds a projected column outside `data` must
        // recompute: a data-only patch would leave that column stale (#98).
        // SAFETY: DatumWithOid wraps the entity name as a TEXT parameter.
        let args = [unsafe {
            pgrx::datum::DatumWithOid::new(
                name.as_str(),
                PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
            )
        }];
        let definition: Option<String> = Spi::get_one_with_args(
            "SELECT definition FROM pg_tview_meta WHERE entity = $1",
            &args,
        )
        .unwrap_or(None);
        match definition
            .as_deref()
            .and_then(crate::schema::direct_map::columns_referenced_outside_data)
        {
            Some(projected) => direct_map.retain(|col, _| !projected.contains(&col.to_lowercase())),
            None => direct_map.clear(),
        }

        Ok(Some(CachedEntityInfo {
            name,
            distinct_on_key: meta.distinct_on_keys.first().cloned(),
            direct_map,
            fk_columns: meta.fk_columns,
            uuid_fk_columns: meta.uuid_fk_columns,
            output_columns,
            is_union: meta.is_union,
        }))
    }

    /// Invalidate the table entity cache
    /// Should be called when TVIEWs are created or dropped
    pub fn invalidate() {
        let mut cache = TABLE_ENTITY_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.clear();
    }
}

/// Cache operations for cascade paths (transaction-scoped)
pub mod cascade_cache {
    use super::{CASCADE_PATH_CACHE, CascadePath, pg_sys};

    /// Get cached cascade paths for a source table OID.
    /// Returns all `CascadePath` entries across all entities where `source_oid` matches.
    pub fn cascade_paths_for_table(table_oid: pg_sys::Oid) -> crate::TViewResult<Vec<CascadePath>> {
        CASCADE_PATH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();

            if let Some(paths) = cache.get(&table_oid) {
                return Ok(paths.clone());
            }

            let paths = load_cascade_paths_for_table(table_oid)?;
            cache.insert(table_oid, paths.clone());
            Ok(paths)
        })
    }

    /// Load cascade paths matching a source table OID from `pg_tview_meta`
    fn load_cascade_paths_for_table(
        table_oid: pg_sys::Oid,
    ) -> crate::TViewResult<Vec<CascadePath>> {
        let meta_list = crate::catalog::TviewMeta::load_all()?;

        let mut relevant_paths = Vec::new();
        for meta in meta_list {
            for path in meta.cascade_paths {
                if path.source_oid == table_oid {
                    relevant_paths.push(path);
                }
            }
        }

        Ok(relevant_paths)
    }

    /// Clear the cascade path cache (called on transaction end)
    pub fn clear_cache() {
        CASCADE_PATH_CACHE.with(|cache| {
            cache.borrow_mut().clear();
        });
    }
}

// ── Cross-backend invalidation ───────────────────────────────────────────
//
// Every cache here is per backend. A relcache invalidation (DDL on a watched
// relation, or the `pg_tview_meta` statement trigger that invalidates the catalog
// itself) bumps the generation; the next `sync_generation()` then clears them all.
// The callback only touches a `Cell`: it can run in the middle of a catalog access,
// so it must neither take a lock nor call SPI.

thread_local! {
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SEEN_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static WATCHED: std::cell::RefCell<std::collections::HashSet<pg_sys::Oid>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

thread_local! {
    static CATALOG_WATCHED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Watch `pg_tview_meta` itself: its statement trigger invalidates it on every
/// write, which is how other backends learn about a created, changed or dropped
/// TVIEW. Resolved once per backend.
fn watch_catalog() {
    if CATALOG_WATCHED.with(std::cell::Cell::get) {
        return;
    }
    crate::metrics::metrics_api::record_catalog_lookup();
    if let Ok(Some(oid)) = Spi::connect(|client| {
        client
            .select("SELECT to_regclass('pg_tview_meta')::oid", None, &[])?
            .first()
            .get_one::<pg_sys::Oid>()
    }) {
        watch(&[oid]);
        CATALOG_WATCHED.with(|w| w.set(true));
    }
}

/// Watch relations whose DDL must invalidate the caches (TVIEW tables and views,
/// `pg_tview_meta`).
pub fn watch(oids: &[pg_sys::Oid]) {
    WATCHED.with(|w| w.borrow_mut().extend(oids.iter().copied()));
}

/// Clear every cache if a relevant invalidation arrived since the last call.
pub fn sync_generation() {
    watch_catalog();
    let current = GENERATION.with(std::cell::Cell::get);
    if SEEN_GENERATION.with(std::cell::Cell::get) != current {
        SEEN_GENERATION.with(|s| s.set(current));
        invalidate_all_caches();
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn relcache_callback(_arg: pg_sys::Datum, relid: pg_sys::Oid) {
    let relevant = relid == pg_sys::InvalidOid
        || WATCHED.with(|w| w.try_borrow().map_or(true, |w| w.contains(&relid)));
    if relevant {
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

/// Register the relcache callback. Called once from `_PG_init`.
pub fn register_relcache_callback() {
    // SAFETY: registers a static callback with no argument; valid in `_PG_init`.
    unsafe {
        pg_sys::CacheRegisterRelcacheCallback(Some(relcache_callback), pg_sys::Datum::from(0));
    }
}

/// Invalidate the `pg_tviews` caches of every backend (and this one) once the
/// current transaction commits, by invalidating `relid`'s relcache entry.
#[pg_extern]
fn pg_tviews_invalidate_caches(relid: pg_sys::Oid) {
    // SAFETY: the caller passes an existing relation (the trigger's TG_RELID).
    unsafe { pg_sys::CacheInvalidateRelcacheByRelid(relid) };
}

/// Combined cache invalidation for all caches
pub fn invalidate_all_caches() {
    graph_cache::invalidate();
    table_cache::invalidate();
    cascade_cache::clear_cache();
    crate::catalog::clear_meta_cache();
    super::ops::clear_crash_recovery_cache();
    crate::lifecycle::invalidate_jsonb_delta_cache();
    crate::utils::invalidate_oid_relname_cache();
    crate::utils::invalidate_view_columns_cache();
    crate::utils::invalidate_dedup_dml_cache();
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)] // Reason: test module prelude import
mod tests {
    use super::*;

    /// Serialises the tests that mutate the process-global `TABLE_ENTITY_CACHE`;
    /// the test harness runs tests in parallel threads.
    static TABLE_CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_graph_cache_invalidation() {
        // Test that invalidate clears the cache
        graph_cache::invalidate();

        assert!(ENTITY_GRAPH_CACHE.lock().unwrap().is_none());
    }

    #[test]
    fn test_table_cache_invalidation() {
        let _guard = TABLE_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        // Add something to cache
        {
            let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
            cache.insert(
                pg_sys::Oid::from(123),
                Some(CachedEntityInfo {
                    name: "test".to_string(),
                    distinct_on_key: None,
                    direct_map: HashMap::new(),
                    fk_columns: Vec::new(),
                    uuid_fk_columns: Vec::new(),
                    output_columns: Vec::new(),
                    is_union: false,
                }),
            );
        }

        // Verify it's there
        assert!(
            TABLE_ENTITY_CACHE
                .lock()
                .unwrap()
                .get(&pg_sys::Oid::from(123))
                .is_some()
        );

        // Invalidate
        table_cache::invalidate();

        // Verify it's gone
        assert!(TABLE_ENTITY_CACHE.lock().unwrap().is_empty());
    }

    #[test]
    fn test_cached_entity_info_carries_direct_patch_fields() {
        // The cached record exposes everything the issue #56 eligibility check
        // needs, so the trigger hot path never re-queries pg_tview_meta.
        let mut direct_map = HashMap::new();
        direct_map.insert("bio".to_string(), "bio".to_string());
        direct_map.insert("name".to_string(), "display_name".to_string());

        let info = CachedEntityInfo {
            name: "user".to_string(),
            distinct_on_key: None,
            direct_map,
            fk_columns: vec!["fk_org".to_string()],
            uuid_fk_columns: vec![],
            output_columns: vec!["pk_user".to_string(), "id".to_string(), "data".to_string()],
            is_union: false,
        };

        assert_eq!(info.direct_map.get("bio").map(String::as_str), Some("bio"));
        assert_eq!(
            info.direct_map.get("name").map(String::as_str),
            Some("display_name")
        );
        assert!(info.fk_columns.contains(&"fk_org".to_string()));
        assert!(info.output_columns.contains(&"data".to_string()));
        assert!(!info.is_union);
    }

    #[test]
    fn test_negative_cache_entry() {
        let _guard = TABLE_CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        table_cache::invalidate();
        // Insert a None entry
        TABLE_ENTITY_CACHE
            .lock()
            .unwrap()
            .insert(pg_sys::Oid::from(999), None);
        // Verify it's cached as None (not a cache miss)
        let cache = TABLE_ENTITY_CACHE.lock().unwrap();
        assert!(cache.get(&pg_sys::Oid::from(999)).is_some()); // key exists
        assert!(cache.get(&pg_sys::Oid::from(999)).unwrap().is_none()); // value is None
    }
}
