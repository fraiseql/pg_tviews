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
use crate::catalog::{PlanEntry, TviewMeta};
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

    fn retain(&self, keep: impl FnMut(&K, &mut V) -> bool) {
        self.0.borrow_mut().retain(keep);
    }
}

thread_local! {
    /// Every TVIEW's entity and plan (one entry, keyed by `()`).
    pub static PLANS_BY_ENTITY: Memo<(), std::rc::Rc<[PlanEntry]>> = Memo::new();
    /// The entity dependency graph (one entry, keyed by `()`).
    pub static GRAPH: Memo<(), EntityDepGraph> = Memo::new();
    /// Table → the local paths starting at it.
    pub static CASCADE_PATHS: Memo<Oid, std::rc::Rc<[LocalPath]>> = Memo::new();
    /// Every `TviewMeta` read so far (keyed by entity).
    pub static METAS: Memo<String, std::rc::Rc<TviewMeta>> = Memo::new();
    /// Relation → its column names, in order.
    pub static COLUMNS: Memo<Oid, Vec<String>> = Memo::new();
    /// `(entity, relid)` → the rendered mapping query (`None`: a relation is gone).
    pub static MAPPINGS: Memo<(String, u32), Option<String>> = Memo::new();
    /// `(entity, relid)` → the rendered lookup of a fan-out through a table in
    /// between (`None`: a relation is gone).
    pub static FANOUT_VIAS: Memo<(String, u32), Option<String>> = Memo::new();
    /// `(entity, relid, attnum)` → the rendered read-set query (`None`: a
    /// relation is gone).
    pub static READ_SETS: Memo<(String, u32, i16), Option<String>> = Memo::new();
    /// `(relid, attnum)` → how a value of a column writers lock values of is locked.
    pub static LOCK_VALUES: Memo<(u32, i16), Option<crate::concurrency::LockValue>> = Memo::new();
    /// `(how it is planned, SQL)` → its kept statement, for the queries every
    /// refresh runs and the catalog reads. Kept across [`invalidate_all`]:
    /// PostgreSQL revalidates a kept plan when what it reads changes, and a TVIEW
    /// changed changes the SQL generated for it.
    pub static PLANS: Memo<(crate::utils::spi::Plan, String), std::rc::Rc<crate::utils::spi::KeptPlan>> =
        Memo::new();
    /// `(SQL, argument types, mutating, the table whose transition tables it
    /// reads)` → the statement kept for the backend. Kept across
    /// [`invalidate_all`] as [`PLANS`] are, but for the statements over transition
    /// tables: PostgreSQL does not revalidate those when their table changes.
    pub static STATEMENTS: Memo<StatementKey, std::rc::Rc<pgrx::spi::OwnedPreparedStatement>> =
        Memo::new();
    /// `(table, attnums)` → how its old and new transition rows pair up, and the
    /// names of those columns (`None`: they can't be paired).
    pub static PAIRINGS: Memo<(Oid, Vec<i16>), Option<RowPairing>> = Memo::new();
    /// Table → its primary key columns, each with whether the equality operator
    /// of its type is in `pg_catalog`.
    pub static PRIMARY_KEYS: Memo<Oid, std::rc::Rc<[(String, bool)]>> = Memo::new();
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

/// What a kept statement is found by: its SQL, its argument types, whether it
/// may change data, and the table whose transition tables it reads.
pub type StatementKey = (String, Vec<Oid>, bool, Option<Oid>);

/// The condition pairing an updated row's two images in a mapped table's
/// transition tables, and the names of the columns the TVIEW reads.
pub type RowPairing = (String, Vec<String>);

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
    let graph = EntityDepGraph::from_plans(&plans()?)?;
    GRAPH.with(|m| m.insert((), graph.clone()));
    Ok(graph)
}

/// The local paths starting at `table_oid`, across every TVIEW's plan (cached
/// for the backend: a TVIEW created, changed or dropped anywhere writes
/// `pg_tview_meta`, which invalidates it).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn cascade_paths(table_oid: Oid) -> crate::TViewResult<std::rc::Rc<[LocalPath]>> {
    CASCADE_PATHS.with(|m| {
        m.get_or_load(table_oid, || {
            Ok(plans()?
                .iter()
                .flat_map(|entry| &entry.plan.paths)
                .filter(|path| path.source_oid == table_oid)
                .cloned()
                .collect::<Vec<_>>()
                .into())
        })
    })
}

/// Every TVIEW's entity and plan (cached for the backend), which the dependency
/// graph and the cascade paths are built from: not the TVIEWs' full catalog rows
/// ([`TviewMeta::load_plans`]).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn plans() -> crate::TViewResult<std::rc::Rc<[PlanEntry]>> {
    PLANS_BY_ENTITY.with(|m| m.get_or_load((), || Ok(TviewMeta::load_plans()?.into())))
}

/// Forget every cached value of this backend but its kept plans ([`PLANS`],
/// [`STATEMENTS`] not over transition tables).
pub fn invalidate_all() {
    CHANGED_IN_XACT.with(|c| c.set(true));
    PLANS_BY_ENTITY.with(Memo::clear);
    GRAPH.with(Memo::clear);
    CASCADE_PATHS.with(Memo::clear);
    METAS.with(Memo::clear);
    COLUMNS.with(Memo::clear);
    MAPPINGS.with(Memo::clear);
    FANOUT_VIAS.with(Memo::clear);
    READ_SETS.with(Memo::clear);
    LOCK_VALUES.with(Memo::clear);
    STATEMENTS.with(|m| m.retain(|(_, _, _, over), _| over.is_none()));
    DELTAS.with(Memo::clear);
    PAIRINGS.with(Memo::clear);
    PRIMARY_KEYS.with(Memo::clear);
    COMPUTED.with(Memo::clear);
    PARTITION_ROOTS.with(Memo::clear);
    JSONB_DELTA_SCHEMA.with(Memo::clear);
    crate::utils::forget_logged(crate::jsonb_delta::JSONB_DELTA_MISSING);
    // The catalog may have been dropped and created again under another OID.
    CATALOG_WATCHED.with(|w| w.set(false));
}

// ── cross-backend invalidation ───────────────────────────────────────────────

thread_local! {
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static SEEN_GENERATION: Cell<u64> = const { Cell::new(0) };
    static WATCHED: RefCell<HashSet<Oid>> = RefCell::new(HashSet::new());
    static CATALOG_WATCHED: Cell<bool> = const { Cell::new(false) };
    /// A relation invalidation arrived, or the caches were cleared, in the open
    /// transaction.
    static CHANGED_IN_XACT: Cell<bool> = const { Cell::new(false) };
}

/// At the abort of a transaction, or of a subtransaction: forget the caches if
/// the transaction may have memoized a catalog change being rolled back. A change
/// of its own came through the utility hook ([`mark_changed`]), as a relation
/// invalidation (processed locally at each command boundary) or a clear by the
/// DDL path that made it; without one, the
/// caches hold what the committed catalog says, and the next write needs no lookup.
pub fn forget_on_abort() {
    if changed_in_xact() {
        invalidate_all();
    }
}

/// The open transaction ran a statement that may change the catalog.
pub fn mark_changed() {
    CHANGED_IN_XACT.with(|c| c.set(true));
}

/// Whether the open transaction may have changed the catalog: what an abort
/// must forget (see [`forget_on_abort`]).
pub fn changed_in_xact() -> bool {
    CHANGED_IN_XACT.with(Cell::get)
}

/// At the end of a transaction, however it ends.
pub fn end_transaction() {
    CHANGED_IN_XACT.with(|c| c.set(false));
}

/// Watch `pg_tview_meta` itself: its statement trigger invalidates it on every
/// write, which is how other backends learn about a created, changed or dropped
/// TVIEW. Resolved again after every invalidation.
fn watch_catalog() {
    if CATALOG_WATCHED.with(Cell::get) {
        return;
    }
    if let Some(oid) = crate::catalog::queries::meta_table_oid() {
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
    // Any relation's: what the caches memoized of one they don't watch may be rolled
    // back too. Plain writes send none.
    CHANGED_IN_XACT.with(|c| c.set(true));
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
