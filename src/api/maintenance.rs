//! Functions acting on every TVIEW: an operator's (`EXECUTE` revoked from
//! `PUBLIC`, `docs/user-guides/operators.md`).

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

use crate::error::TViewError;

/// Rebuild every TVIEW, dependencies first, and report how many were rebuilt,
/// in which order, and how long it took.
#[pg_extern]
fn pg_tviews_refresh_all() -> Result<pgrx::datum::JsonB, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    crate::concurrency::require_read_committed();
    if crate::suspend::is_suspended() {
        return Err(TViewError::WrongState {
            reason: "Cannot refresh: triggers are suspended".to_string(),
        }
        .into());
    }
    let start = std::time::Instant::now();
    let order = crate::admin::refresh_all_in_dependency_order()?;
    Ok(pgrx::datum::JsonB(serde_json::json!({
        "refreshed_count": order.len(),
        "order": order,
        "duration_ms": start.elapsed().as_millis(),
    })))
}

/// Rebuild TVIEWs from their backing views, dependencies first, and return each
/// rebuilt entity with the rows it inserted (`rows_inserted`), in rebuild order.
/// With `only_empty` (the default) only the UNLOGGED TVIEWs PostgreSQL reset are
/// filled: run it after a promotion, a crash restart or a restore. With
/// `only_empty => false` every TVIEW is rebuilt: every row of its view is
/// inserted again.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_rebuild_all(
    only_empty: default!(bool, true),
) -> Result<TableIterator<'static, (name!(entity, String), name!(rows_inserted, i64))>, ErrorReport>
{
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    crate::concurrency::require_read_committed();
    Ok(TableIterator::new(crate::replication::rebuild_all(
        only_empty,
    )?))
}

/// Zero the refresh statistics of every TVIEW of the database (`tviews.stats`,
/// ADR 0221).
#[pg_extern]
fn pg_tviews_reset_stats_all() -> Result<(), ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let tables: Vec<_> = crate::catalog::row::relations()?
        .into_iter()
        .map(|listed| listed.table)
        .collect();
    crate::stats::reset(&tables, true)?;
    Ok(())
}

/// Create the propagation indexes every TVIEW is missing, and return their DDL;
/// with `dry_run`, only return it.
#[pg_extern]
fn pg_tviews_ensure_propagation_indexes_all(
    dry_run: default!(bool, false),
) -> Result<SetOfIterator<'static, String>, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    let metas: Vec<_> = crate::catalog::TviewMeta::load_all()?
        .into_iter()
        .map(std::rc::Rc::new)
        .collect();
    Ok(SetOfIterator::new(
        crate::admin::ensure_propagation_indexes(&metas, dry_run)?,
    ))
}

/// The tag (`objid` in `pg_locks`) of the value lock on `value` of
/// `column_name` of `relation` (a table a TVIEW reads, a partitioned table's
/// root), or, with `column_name` NULL, of the key `value` of TVIEW table
/// `relation` being created. Superusers only (`EXECUTE` revoked from `PUBLIC`).
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx passes SQL arguments by value
fn pg_tviews_value_lock_tag(
    relation: pgrx::PgRelation,
    column_name: Option<&str>,
    value: &str,
) -> Result<i64, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    Ok(crate::concurrency::value_lock_tag(
        relation.oid(),
        column_name,
        value,
    )?)
}
