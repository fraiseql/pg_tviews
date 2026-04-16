use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use pgrx::prelude::*;

/// Cached information for a table managed by pg_tviews
#[derive(Clone, Debug)]
pub struct CachedEntityInfo {
    pub name: String,
    /// First DISTINCT ON key if this is a DISTINCT ON TVIEW, None otherwise
    #[allow(dead_code)] // Reason: Will be used by trigger handler in REFACTOR phase
    pub distinct_on_key: Option<String>,
}

/// Global cache for `EntityDepGraph` to avoid repeated `pg_tview_meta` queries
static ENTITY_GRAPH_CACHE: LazyLock<Mutex<Option<super::graph::EntityDepGraph>>> = LazyLock::new(|| {
    Mutex::new(None)
});

/// Global cache for table OID → entity info (name + distinct_on_key)
static TABLE_ENTITY_CACHE: LazyLock<Mutex<HashMap<pg_sys::Oid, CachedEntityInfo>>> = LazyLock::new(|| {
    Mutex::new(HashMap::new())
});

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

        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap_or_else(PoisonError::into_inner);

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
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        *cache = None;
    }
}

/// Cache operations for table OID → entity mapping
pub mod table_cache {
    #[allow(clippy::wildcard_imports)] // Reason: module-internal prelude import
    use super::*;

    /// Get cached entity info (name + distinct_on_key) for table OID
    /// Loads from database on first miss per session
    pub fn entity_info_cached(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<CachedEntityInfo>> {
        // Check if caching is enabled
        if !crate::config::table_cache_enabled() {
            return load_entity_info_uncached(table_oid);
        }

        // Fast path: check cache
        {
            let cache = TABLE_ENTITY_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(info) = cache.get(&table_oid) {
                crate::metrics::metrics_api::record_table_cache_hit();
                return Ok(Some(info.clone()));
            }
        }

        // Slow path: query and cache
        crate::metrics::metrics_api::record_table_cache_miss();
        let info = load_entity_info_uncached(table_oid)?;

        if let Some(ref i) = info {
            let mut cache = TABLE_ENTITY_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
            cache.insert(table_oid, i.clone());
        }

        Ok(info)
    }

    /// Get cached entity name (backward compatibility)
    pub fn entity_for_table_cached(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<String>> {
        entity_info_cached(table_oid).map(|info| info.map(|i| i.name))
    }

    /// Load entity info from database (name + distinct_on_key)
    fn load_entity_info_uncached(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<CachedEntityInfo>> {
        let entity_name = crate::catalog::entity_for_table_uncached(table_oid)?;

        match entity_name {
            Some(name) => {
                // TODO: P-01 REFACTOR - enhance to query distinct_on_keys[1] from pg_tview_meta
                // For now, trigger handler will call load_by_entity once per session
                Ok(Some(CachedEntityInfo {
                    name,
                    distinct_on_key: None,
                }))
            }
            None => Ok(None),
        }
    }

    /// Invalidate the table entity cache
    /// Should be called when TVIEWs are created or dropped
    pub fn invalidate() {
        let mut cache = TABLE_ENTITY_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        cache.clear();
    }
}

/// Combined cache invalidation for all caches
pub fn invalidate_all_caches() {
    graph_cache::invalidate();
    table_cache::invalidate();
    crate::lifecycle::invalidate_jsonb_delta_cache();
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
                CachedEntityInfo {
                    name: "test".to_string(),
                    distinct_on_key: None,
                },
            );
        }

        // Verify it's there
        assert!(TABLE_ENTITY_CACHE.lock().unwrap().get(&pg_sys::Oid::from(123)).is_some());

        // Invalidate
        table_cache::invalidate();

        // Verify it's gone
        assert!(TABLE_ENTITY_CACHE.lock().unwrap().is_empty());
    }
}