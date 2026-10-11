//! Entry points for `pg_tviews`' own SQL (event triggers, catalog triggers), not
//! for callers.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Called by the `sql_drop` event trigger for a TVIEW whose backing view or table
/// was dropped as a dependent (see [`crate::ddl::drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), ErrorReport> {
    crate::ddl::drop::handle_dropped(entity)
        .map_err(|e| e.report_in(&format!("Failed to deregister TVIEW '{entity}'")))
}

/// Invalidate the `pg_tviews` caches of every backend (and this one) once the
/// current transaction commits, by invalidating `relid`'s relcache entry.
#[pg_extern]
fn pg_tviews_invalidate_caches(relid: pg_sys::Oid) {
    // SAFETY: the caller passes an existing relation (the trigger's TG_RELID).
    unsafe { pg_sys::CacheInvalidateRelcacheByRelid(relid) };
}
