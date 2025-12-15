use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::LazyLock;
use pgrx::prelude::*;

/// Global cache for `EntityDepGraph` to avoid repeated `pg_tview_meta` queries
static ENTITY_GRAPH_CACHE: LazyLock<Mutex<Option<super::graph::EntityDepGraph>>> = LazyLock::new(|| {
    Mutex::new(None)
});

/// Global cache for table OID → entity name mapping
static TABLE_ENTITY_CACHE: LazyLock<Mutex<HashMap<pg_sys::Oid, String>>> = LazyLock::new(|| {
    Mutex::new(HashMap::new())
});

/// Cache operations for `EntityDepGraph`
pub mod graph_cache {
    use super::*;

    /// Get cached `EntityDepGraph`, loading from database if not cached
    pub fn load_cached() -> crate::TViewResult<crate::queue::graph::EntityDepGraph> {
        // Check if caching is enabled
        if !crate::config::graph_cache_enabled() {
            return crate::queue::graph::EntityDepGraph::load();
        }

        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();

        if let Some(graph) = cache.as_ref() {
            // Cache hit
            crate::metrics::metrics_api::record_graph_cache_hit();
            return Ok(graph.clone());
        }

        // Cache miss: load from database
        crate::metrics::metrics_api::record_graph_cache_miss();
        let graph = crate::queue::graph::EntityDepGraph::load()?;
        *cache = Some(graph.clone());
        Ok(graph)
    }

    /// Invalidate the `EntityDepGraph` cache
    /// Should be called when TVIEWs are created or dropped
    pub fn invalidate() {
        let mut cache = ENTITY_GRAPH_CACHE.lock().unwrap();
        *cache = None;
    }
}

/// Cache operations for table OID → entity mapping
pub mod table_cache {
    use super::*;

    /// Get cached entity name for table OID, loading from database if not cached
    pub fn entity_for_table_cached(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<String>> {
        // Check if caching is enabled
        if !crate::config::table_cache_enabled() {
            return crate::catalog::entity_for_table_uncached(table_oid);
        }

        // Fast path: check cache
        {
            let cache = TABLE_ENTITY_CACHE.lock().unwrap();
            if let Some(entity) = cache.get(&table_oid) {
                crate::metrics::metrics_api::record_table_cache_hit();
                return Ok(Some(entity.clone()));
            }
        }

        // Slow path: query and cache
        crate::metrics::metrics_api::record_table_cache_miss();
        let entity = crate::catalog::entity_for_table_uncached(table_oid)?;

        if let Some(ref e) = entity {
            let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
            cache.insert(table_oid, e.clone());
        }

        Ok(entity)
    }

    /// Invalidate the table entity cache
    /// Should be called when TVIEWs are created or dropped
    pub fn invalidate() {
        let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
        cache.clear();
    }
}

/// Combined cache invalidation for all caches
pub fn invalidate_all_caches() {
    graph_cache::invalidate();
    table_cache::invalidate();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_cache_invalidation() {
        // Test that invalidate clears the cache
        graph_cache::invalidate();

        let cache = ENTITY_GRAPH_CACHE.lock().unwrap();
        assert!(cache.is_none());
    }

    #[test]
    fn test_table_cache_invalidation() {
        // Add something to cache
        {
            let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
            cache.insert(pg_sys::Oid::from(123), "test".to_string());
        }

        // Verify it's there
        {
            let cache = TABLE_ENTITY_CACHE.lock().unwrap();
            assert_eq!(cache.get(&pg_sys::Oid::from(123)), Some(&"test".to_string()));
        }

        // Invalidate
        table_cache::invalidate();

        // Verify it's gone
        {
            let cache = TABLE_ENTITY_CACHE.lock().unwrap();
            assert!(cache.is_empty());
        }
    }
}