//! Functions acting on every TVIEW: an operator's (`EXECUTE` revoked from
//! `PUBLIC`, `docs/user-guides/operators.md`).

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

use crate::error::TViewError;

/// Rebuild every TVIEW, dependencies first, and report how many were rebuilt,
/// in which order, and how long it took.
#[pg_extern]
fn pg_tviews_refresh_all() -> Result<pgrx::datum::JsonB, ErrorReport> {
    crate::revision::check();
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
/// rebuilt entity with its row count, in rebuild order. With `only_empty` (the
/// default) only the UNLOGGED TVIEWs PostgreSQL reset are filled: run it after a
/// promotion, a crash restart or a restore. With `only_empty => false` every TVIEW
/// is rebuilt.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_rebuild_all(
    only_empty: default!(bool, true),
) -> Result<TableIterator<'static, (name!(entity, String), name!(rows, i64))>, ErrorReport> {
    crate::revision::check();
    Ok(TableIterator::new(crate::replication::rebuild_all(
        only_empty,
    )?))
}
