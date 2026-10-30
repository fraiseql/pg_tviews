//! Functions acting on the caller's session or transaction.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Suspend trigger-based refresh in this transaction: writes record which TVIEWs
/// they change instead of refreshing them, until the matching resume or the end
/// of the transaction, which refreshes them.
#[pg_extern]
fn pg_tviews_suspend_triggers() {
    let _path = crate::owner::CatalogPath::pin();
    crate::suspend::suspend();
}

/// Resume trigger-based refresh. When the outermost suspension ends, every TVIEW
/// changed while suspended (and every TVIEW reading one of them) is rebuilt.
#[pg_extern]
fn pg_tviews_resume_triggers() -> Result<(), ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    crate::suspend::resume()?;
    if !crate::suspend::is_suspended() {
        crate::suspend::catch_up()?;
    }
    Ok(())
}

/// Whether this transaction's trigger-based refresh is suspended.
#[pg_extern]
fn pg_tviews_is_suspended() -> bool {
    let _path = crate::owner::CatalogPath::pin();
    crate::suspend::is_suspended()
}

/// The TVIEWs changed while this transaction's refresh is suspended.
#[pg_extern]
fn pg_tviews_suspended_entities() -> Vec<String> {
    let _path = crate::owner::CatalogPath::pin();
    crate::suspend::get_changed_entities()
}

/// Flush the transaction's refresh queue and report the TVIEW rows it changed,
/// as `FraiseQL`'s cascade response reads them (`docs/user-guides/graphql-cascade.md`).
#[pg_extern]
fn pg_tviews_flush_and_report(
    max_entities: default!(i32, 500),
    include_data: default!(bool, true),
    reset: default!(bool, true),
    types: default!(Option<Vec<String>>, "NULL"),
) -> Result<pgrx::JsonB, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    Ok(pgrx::JsonB(crate::report::flush_and_report(
        max_entities,
        include_data,
        reset,
        types,
    )?))
}

/// The session's refresh counters (debugging; `tviews.stats` is per TVIEW and
/// readable from any session).
#[pg_extern]
fn pg_tviews_queue_stats() -> pgrx::JsonB {
    let _path = crate::owner::CatalogPath::pin();
    pgrx::JsonB(crate::health::queue_stats())
}

/// The transactions the cluster committed without publishing their changes on
/// the change feed (the NOTIFY queue being nearly full), since the server
/// started (#247). A listener reads it when it connects and on each heartbeat,
/// and evicts everything when it moved.
#[pg_extern]
fn pg_tviews_feed_gaps() -> Result<i64, ErrorReport> {
    let _path = crate::owner::CatalogPath::pin();
    let gaps = crate::stats::feed_gaps()?;
    Ok(i64::try_from(gaps).unwrap_or(i64::MAX))
}

/// The transaction's refresh queue (debugging).
#[pg_extern]
fn pg_tviews_debug_queue() -> pgrx::JsonB {
    let _path = crate::owner::CatalogPath::pin();
    pgrx::JsonB(crate::health::debug_queue())
}
