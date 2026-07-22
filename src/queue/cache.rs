use pgrx::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::cascade_path::CascadePath;

/// Cached information for a table managed by `pg_tviews`
#[derive(Clone, Debug)]
pub struct CachedEntityInfo {
    pub name: String,
    /// First DISTINCT ON key if this is a DISTINCT ON TVIEW, None otherwise
    pub distinct_on_key: Option<String>,
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

    /// Load entity info from database (name + `distinct_on_key`)
    fn load_entity_info_uncached(
        table_oid: pg_sys::Oid,
    ) -> crate::TViewResult<Option<CachedEntityInfo>> {
        let entity_name = crate::catalog::entity_for_table_uncached(table_oid)?;

        match entity_name {
            Some(name) => {
                // Query distinct_on_keys from pg_tview_meta
                let distinct_on_key = pgrx::spi::Spi::connect(|client| {
                    let args = vec![unsafe {
                        pgrx::datum::DatumWithOid::new(
                            &name,
                            pgrx::pg_sys::PgOid::BuiltIn(pgrx::pg_sys::PgBuiltInOids::TEXTOID)
                                .value(),
                        )
                    }];
                    let mut rows = client.select(
                        "SELECT distinct_on_keys FROM pg_tview_meta WHERE entity = $1",
                        None,
                        &args,
                    )?;

                    let result: Result<Option<String>, pgrx::spi::Error> = match rows.next() {
                        Some(row) => {
                            // Extract first element from distinct_on_keys TEXT[] array
                            let keys: Option<Vec<String>> = row["distinct_on_keys"].value()?;
                            Ok(keys.and_then(|k| k.first().cloned()))
                        }
                        None => Ok(None),
                    };
                    result
                })?;

                Ok(Some(CachedEntityInfo {
                    name,
                    distinct_on_key,
                }))
            }
            None => Ok(None),
        }
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

/// Combined cache invalidation for all caches
pub fn invalidate_all_caches() {
    graph_cache::invalidate();
    table_cache::invalidate();
    crate::lifecycle::invalidate_jsonb_delta_cache();
    crate::utils::invalidate_oid_relname_cache();
    crate::utils::invalidate_view_columns_cache();
    crate::utils::invalidate_dedup_dml_cache();
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)] // Reason: test module prelude import
mod tests {
    use super::*;

    #[test]
    fn test_graph_cache_invalidation() {
        // Test that invalidate clears the cache
        graph_cache::invalidate();

        assert!(ENTITY_GRAPH_CACHE.lock().unwrap().is_none());
    }

    #[test]
    fn test_table_cache_invalidation() {
        // Add something to cache
        {
            let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
            cache.insert(
                pg_sys::Oid::from(123),
                Some(CachedEntityInfo {
                    name: "test".to_string(),
                    distinct_on_key: None,
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
    fn test_negative_cache_entry() {
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
