//! Entry points for `pg_tviews`' own SQL (event triggers, catalog triggers), not
//! for callers.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Called by the `sql_drop` event trigger for a TVIEW whose backing view or table
/// was dropped as a dependent (see [`crate::ddl::drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::ddl::drop::handle_dropped(entity)
        .map_err(|e| e.report_in(&format!("Failed to deregister TVIEW '{entity}'")))
}

/// Invalidate the `pg_tviews` caches of every backend (and this one) once the
/// current transaction commits, by invalidating `relid`'s relcache entry.
#[pg_extern]
fn pg_tviews_invalidate_caches(relid: pg_sys::Oid) {
    let _path = crate::owner::CatalogPath::pin();
    // SAFETY: the caller passes an existing relation (the trigger's TG_RELID).
    unsafe { pg_sys::CacheInvalidateRelcacheByRelid(relid) };
}

/// The plan registering the view `view` as the TVIEW `tview` (its entity) would
/// store (default options), without creating it: the plan-snapshot tests
/// (`test/sql/plans/`) read the walker through it.
#[pg_extern]
fn pg_tviews_plan_of(view: pg_sys::Oid, tview: &str) -> Result<pgrx::JsonB, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    crate::ddl::create::plan_of(tview, view)
        .map(pgrx::JsonB)
        .map_err(|e| e.report_in(&format!("pg_tviews: the plan of tv_{tview}")))
}

/// Called by the catalog's `BEFORE INSERT` trigger for each row a restore loads
/// into `pg_tview_meta`: its plan rebound to this database
/// ([`crate::catalog::plan::rebind_restored`]).
#[pg_extern]
fn pg_tviews_plan_rebind(tview: &str, plan: pgrx::JsonB) -> Result<pgrx::JsonB, ErrorReport> {
    use crate::lineage::plan::Unbound;
    let _path = crate::owner::CatalogPath::pin();
    let entity = tview;
    let operation = format!("Rebind the restored plan of tv_{entity}");
    let decoded = crate::lineage::plan::TviewPlan::decode(entity, plan.0)
        .map_err(|e| e.report_in(&operation))?;
    let rebound =
        crate::catalog::plan::rebind_restored(&decoded).map_err(|e| e.report_in(&operation))?;
    let reregister = format!("SELECT tviews.pg_tviews_reregister('{entity}') re-derives it.");
    let (code, message, hint) = match rebound {
        Ok(plan) => {
            return serde_json::to_value(&plan).map(pgrx::JsonB).map_err(|e| {
                crate::TViewError::CatalogError {
                    operation: operation.clone(),
                    pg_error: e.to_string(),
                }
                .report_in(&operation)
            });
        }
        Err(Unbound::Table(tables)) => (
            PgSqlErrorCode::ERRCODE_UNDEFINED_TABLE,
            format!("the plan of TVIEW tv_{entity} names tables that do not exist: {tables}"),
            "Restore the tables the TVIEW reads before its catalog row.".to_string(),
        ),
        Err(Unbound::Column(table, column)) => (
            PgSqlErrorCode::ERRCODE_UNDEFINED_COLUMN,
            format!(
                "the plan of TVIEW tv_{entity} names a column that does not exist: \
                 {table}.{column}"
            ),
            "Restore the tables the TVIEW reads, with the columns it reads, before its catalog \
             row."
                .to_string(),
        ),
        Err(Unbound::Unnamed(table, _)) => (
            PgSqlErrorCode::ERRCODE_DATA_CORRUPTED,
            format!("the plan of TVIEW tv_{entity} numbers a column of {table} it does not name"),
            reregister,
        ),
        Err(Unbound::Path(table)) => (
            PgSqlErrorCode::ERRCODE_DATA_CORRUPTED,
            format!(
                "the plan of TVIEW tv_{entity} has a path from {table}, which it maps no write of"
            ),
            reregister,
        ),
    };
    Err(ErrorReport::new(code, message, function_name!()).set_hint(hint))
}
