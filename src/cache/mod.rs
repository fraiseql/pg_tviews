//! Every per-backend cache, and the one path that invalidates them.
//!
//! A cache here only memoises catalog reads: clearing it costs a re-read, never
//! correctness. DDL on a watched relation (a TVIEW's table or backing view,
//! `pg_tview_meta`, a base table a mapping query reads) invalidates its relcache
//! entry in every backend; the relcache callback bumps this backend's generation,
//! and the next lookup in any cache here sees the bump and clears them all. The
//! callback only touches a `Cell`: it can run in the middle of a catalog access, so
//! it must neither take a lock nor call SPI.

use crate::catalog::EntityDepGraph;
use crate::catalog::plan::LocalPath;
use crate::catalog::{KeyType, TviewMeta};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// A memo of catalog reads, keyed by `K`, bounded by `pg_tviews.cache_size`.
pub struct Memo<K, V>(RefCell<HashMap<K, V>>);

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    fn new() -> Self {
        Self(RefCell::new(HashMap::new()))
    }

    /// The value for `key`, after any pending invalidation.
    pub fn get(&self, key: &K) -> Option<V> {
        sync_generation();
        self.0.borrow().get(key).cloned()
    }

    /// Remember `value` for `key`.
    pub fn insert(&self, key: K, value: V) {
        let mut map = self.0.borrow_mut();
        if map.len() >= crate::config::cache_size() {
            map.clear();
        }
        map.insert(key, value);
    }

    /// The value for `key`, loaded by `load` on a miss.
    ///
    /// # Errors
    /// What `load` returns.
    pub fn get_or_load(
        &self,
        key: K,
        load: impl FnOnce() -> crate::TViewResult<V>,
    ) -> crate::TViewResult<V> {
        if let Some(value) = self.get(&key) {
            return Ok(value);
        }
        let value = load()?;
        self.insert(key, value.clone());
        Ok(value)
    }

    fn clear(&self) {
        self.0.borrow_mut().clear();
    }
}

thread_local! {
    /// The entity dependency graph (one entry, keyed by `()`).
    pub static GRAPH: Memo<(), EntityDepGraph> = Memo::new();
    /// Table → the local paths starting at it. Also cleared at transaction end.
    pub static CASCADE_PATHS: Memo<Oid, Vec<LocalPath>> = Memo::new();
    /// Every `TviewMeta` read so far (keyed by entity).
    pub static METAS: Memo<String, TviewMeta> = Memo::new();
    /// TVIEW table → the type of its identity column.
    pub static KEY_TYPES: Memo<Oid, KeyType> = Memo::new();
    /// Relation → its column names, in order.
    pub static COLUMNS: Memo<Oid, Vec<String>> = Memo::new();
    /// `(entity, relid)` → the rendered mapping query (`None`: a relation is gone).
    pub static MAPPINGS: Memo<(String, u32), Option<String>> = Memo::new();
    /// `(entity, relid, attnum)` → the rendered read-set query (`None`: a
    /// relation is gone).
    pub static READ_SETS: Memo<(String, u32, i16), Option<String>> = Memo::new();
    /// `(relid, attnum)` → how a value of a column writers lock values of is locked.
    pub static LOCK_VALUES: Memo<(u32, i16), Option<crate::concurrency::LockValue>> = Memo::new();
    /// SQL → its kept plan, for the queries every refresh runs (PostgreSQL
    /// revalidates a kept plan when what it reads changes).
    pub static PLANS: Memo<String, std::rc::Rc<crate::utils::spi::KeptPlan>> = Memo::new();
    /// `(relid, event, attnums)` → the query of the changed rows.
    pub static DELTAS: Memo<(u32, crate::queue::Event, Vec<i16>), String> = Memo::new();
    /// Table → the select list computing its virtual generated columns (`None`:
    /// it has none).
    pub static COMPUTED: Memo<Oid, Option<String>> = Memo::new();
    /// Table → the root of its partition tree (itself when it is no partition).
    pub static PARTITION_ROOTS: Memo<Oid, Oid> = Memo::new();
    /// The quoted schema of `jsonb_delta` (`None`: not installed).
    pub static JSONB_DELTA_SCHEMA: Memo<(), Option<String>> = Memo::new();
}

/// The entity dependency graph (cached per backend).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn graph() -> crate::TViewResult<EntityDepGraph> {
    if !crate::config::graph_cache_enabled() {
        return EntityDepGraph::load();
    }
    if let Some(graph) = GRAPH.with(|m| m.get(&())) {
        crate::metrics::metrics_api::record_graph_cache_hit();
        return Ok(graph);
    }
    crate::metrics::metrics_api::record_graph_cache_miss();
    let graph = EntityDepGraph::load()?;
    GRAPH.with(|m| m.insert((), graph.clone()));
    Ok(graph)
}

/// The local paths starting at `table_oid`, across every TVIEW's plan (cached
/// for the transaction).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn cascade_paths(table_oid: Oid) -> crate::TViewResult<Vec<LocalPath>> {
    CASCADE_PATHS.with(|m| {
        m.get_or_load(table_oid, || {
            Ok(TviewMeta::load_all()?
                .into_iter()
                .flat_map(|meta| meta.plan.paths)
                .filter(|path| path.source_oid == table_oid)
                .collect())
        })
    })
}

/// Forget every cached value of this backend.
pub fn invalidate_all() {
    GRAPH.with(Memo::clear);
    CASCADE_PATHS.with(Memo::clear);
    METAS.with(Memo::clear);
    KEY_TYPES.with(Memo::clear);
    COLUMNS.with(Memo::clear);
    MAPPINGS.with(Memo::clear);
    READ_SETS.with(Memo::clear);
    LOCK_VALUES.with(Memo::clear);
    PLANS.with(Memo::clear);
    DELTAS.with(Memo::clear);
    COMPUTED.with(Memo::clear);
    PARTITION_ROOTS.with(Memo::clear);
    JSONB_DELTA_SCHEMA.with(Memo::clear);
    crate::utils::forget_logged(crate::jsonb_delta::JSONB_DELTA_MISSING);
    // The catalog may have been dropped and created again under another OID.
    CATALOG_WATCHED.with(|w| w.set(false));
}

/// Forget what is valid for one transaction only.
pub fn end_transaction() {
    CASCADE_PATHS.with(Memo::clear);
}

// ── cross-backend invalidation ───────────────────────────────────────────────

thread_local! {
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static SEEN_GENERATION: Cell<u64> = const { Cell::new(0) };
    static WATCHED: RefCell<HashSet<Oid>> = RefCell::new(HashSet::new());
    static CATALOG_WATCHED: Cell<bool> = const { Cell::new(false) };
}

/// Watch `pg_tview_meta` itself: its statement trigger invalidates it on every
/// write, which is how other backends learn about a created, changed or dropped
/// TVIEW. Resolved again after every invalidation.
fn watch_catalog() {
    if CATALOG_WATCHED.with(Cell::get) {
        return;
    }
    crate::metrics::metrics_api::record_catalog_lookup();
    if let Ok(Some(oid)) = Spi::get_one::<Oid>(&format!(
        "SELECT pg_catalog.to_regclass('{}')::pg_catalog.oid",
        crate::utils::meta_table()
    )) {
        watch(&[oid]);
        CATALOG_WATCHED.with(|w| w.set(true));
    }
}

/// Watch relations whose DDL must invalidate the caches.
pub fn watch(oids: &[Oid]) {
    WATCHED.with(|w| w.borrow_mut().extend(oids.iter().copied()));
}

/// Clear every cache if a relevant invalidation arrived since the last call.
pub fn sync_generation() {
    watch_catalog();
    let current = GENERATION.with(Cell::get);
    if SEEN_GENERATION.with(Cell::get) != current {
        SEEN_GENERATION.with(|s| s.set(current));
        invalidate_all();
        watch_catalog();
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn relcache_callback(_arg: pg_sys::Datum, relid: Oid) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_all_clears_every_memo_and_the_catalog_watch() {
        // Filled directly: `insert` reads a GUC, which a test thread may not.
        COLUMNS.with(|m| m.0.borrow_mut().insert(Oid::from(1), vec!["a".into()]));
        PARTITION_ROOTS.with(|m| m.0.borrow_mut().insert(Oid::from(2), Oid::from(3)));
        CATALOG_WATCHED.with(|w| w.set(true));
        invalidate_all();
        assert!(COLUMNS.with(|m| m.0.borrow().is_empty()));
        assert!(PARTITION_ROOTS.with(|m| m.0.borrow().is_empty()));
        assert!(!CATALOG_WATCHED.with(Cell::get));
    }
}
