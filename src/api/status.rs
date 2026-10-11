//! Read-only status of the extension and its TVIEWs.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// The library's version.
#[pg_extern]
#[allow(clippy::missing_const_for_fn)] // Reason: pgrx #[pg_extern] is incompatible with const fn
fn pg_tviews_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The entity of the TVIEW `tview` names (ADR 0211): its entity, `tv_<entity>`
/// or `schema.tv_<entity>`, quoted or not. 42704 when it names no TVIEW.
#[pg_extern(stable, parallel_safe)]
fn pg_tviews_entity_of(tview: &str) -> Result<String, ErrorReport> {
    Ok(crate::catalog::resolve::find(tview)?.entity)
}

/// Whether `jsonb_delta` is installed (cached).
#[pg_extern]
fn pg_tviews_check_jsonb_delta() -> bool {
    crate::jsonb_delta::check_jsonb_delta_available()
}

/// Health of the installation: version, `jsonb_delta`, catalog revision, the
/// catalog's consistency, triggers, TVIEW count.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_health_check() -> TableIterator<
    'static,
    (
        name!(status, String),
        name!(component, String),
        name!(message, String),
        name!(severity, String),
    ),
> {
    TableIterator::new(crate::health::health_check())
}

/// Replication state of every TVIEW, safe to call on a standby. `is_empty` and
/// `needs_rebuild` are NULL for an UNLOGGED TVIEW during recovery, where its table
/// cannot be read; `needs_rebuild` is NULL for every TVIEW during recovery.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_replication_status() -> Result<
    TableIterator<
        'static,
        (
            name!(entity, String),
            name!(persistence, String),
            name!(replica_readable, bool),
            name!(is_empty, Option<bool>),
            name!(needs_rebuild, Option<bool>),
        ),
    >,
    ErrorReport,
> {
    Ok(TableIterator::new(crate::replication::replication_status()?))
}
