//! The catalog's reads every write needs, memoized for the backend: every
//! TVIEW's plan, the dependency graph built from them, and the local paths of
//! each table. A TVIEW created, changed or dropped anywhere writes
//! `pg_tview_meta`, which invalidates them ([`crate::cache`]).

use super::{EntityDepGraph, PlanEntry, TviewMeta};
use crate::cache::Memo;
use crate::lineage::plan::LocalPath;
use pgrx::pg_sys::Oid;

thread_local! {
    /// Every TVIEW's entity and plan (one entry, keyed by `()`).
    static PLANS_BY_ENTITY: Memo<(), std::rc::Rc<[PlanEntry]>> = Memo::new();
    /// The entity dependency graph (one entry, keyed by `()`).
    static GRAPH: Memo<(), EntityDepGraph> = Memo::new();
    /// Table → the local paths starting at it.
    static CASCADE_PATHS: Memo<Oid, std::rc::Rc<[LocalPath]>> = Memo::new();
    /// Every TVIEW's table (one entry, keyed by `()`).
    static TVIEW_TABLES: Memo<(), std::rc::Rc<std::collections::HashSet<Oid>>> = Memo::new();
}

/// The OIDs of every TVIEW's table (cached for the backend).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn tview_tables() -> crate::TViewResult<std::rc::Rc<std::collections::HashSet<Oid>>> {
    TVIEW_TABLES.with(|m| {
        m.get_or_load((), || {
            let _path = crate::owner::CatalogPath::pin();
            Ok(crate::utils::spi::oids(
                &format!(
                    "SELECT table_oid::pg_catalog.oid FROM {}",
                    super::meta_table()
                ),
                &[],
            )?
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .into())
        })
    })
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
