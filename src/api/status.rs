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

/// The rows of `tviews.stats` (ADR 0221): each TVIEW of the database with its
/// refresh counters since the server started or the last reset. Untracked (the
/// shared table was full) when its counters are NULL.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_stats_rows() -> Result<
    TableIterator<
        'static,
        (
            name!(schema, Option<String>),
            name!(name, String),
            name!(entity, String),
            name!(view_recomputes, Option<i64>),
            name!(noop_skipped, Option<i64>),
            name!(patch_captured, Option<i64>),
            name!(patch_applied, Option<i64>),
            name!(patch_fallbacks, Option<i64>),
            name!(propagation_pruned, Option<i64>),
            name!(rows_written, Option<i64>),
            name!(rows_deleted, Option<i64>),
            name!(full_refreshes, Option<i64>),
            name!(refresh_ms, Option<f64>),
            name!(stats_reset, Option<pgrx::datum::TimestampWithTimeZone>),
            name!(untracked, bool),
        ),
    >,
    ErrorReport,
> {
    use crate::stats::Counter;
    let (counts, overflowed) = crate::stats::read()?;
    let rows = crate::catalog::row::relations()?
        .into_iter()
        .map(|listed| {
            let crate::catalog::row::Listed {
                entity,
                table,
                schema,
                name,
            } = listed;
            let name = name.unwrap_or_else(|| format!("tv_{entity}"));
            let found = counts.get(&table);
            // Absent from a table that never overflowed: no refresh since the
            // counters started.
            let value = |counter: Counter| match found {
                Some(c) => Some(i64::try_from(c.get(counter)).unwrap_or(i64::MAX)),
                None if overflowed => None,
                None => Some(0),
            };
            #[allow(clippy::cast_precision_loss)]
            // Reason: milliseconds to show; 2^52 µs is 142 years
            let refresh_ms = value(Counter::RefreshMicros).map(|us| us as f64 / 1000.0);
            (
                schema,
                name,
                entity,
                value(Counter::ViewRecomputes),
                value(Counter::NoopSkipped),
                value(Counter::PatchCaptured),
                value(Counter::PatchApplied),
                value(Counter::PatchFallbacks),
                value(Counter::PropagationPruned),
                value(Counter::RowsWritten),
                value(Counter::RowsDeleted),
                value(Counter::FullRefreshes),
                refresh_ms,
                found.and_then(|c| pgrx::datum::TimestampWithTimeZone::try_from(c.since).ok()),
                found.is_none() && overflowed,
            )
        })
        .collect::<Vec<_>>();
    Ok(TableIterator::new(rows))
}

extension_sql!(
    r"
-- Per-TVIEW refresh statistics, readable from any session (ADR 0221). Counts,
-- no data: readable by every role.
CREATE VIEW @extschema@.stats AS SELECT * FROM @extschema@.pg_tviews_stats_rows();
COMMENT ON VIEW @extschema@.stats IS
'Refresh statistics of each TVIEW since the server started or pg_tviews_stats_reset()';
GRANT SELECT ON @extschema@.stats TO PUBLIC;
    ",
    name = "stats_view",
    requires = [pg_tviews_stats_rows],
);
