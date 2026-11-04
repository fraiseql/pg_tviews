//! Test builds only: what a refresh upsert failing with 21000 conflicted with (#278).
//!
//! `ON CONFLICT DO UPDATE command cannot affect row a second time` (21000) was seen
//! once under the concurrency fuzzer and never reproduced. With
//! `pg_tviews.test_diag_21000` on, each refresh upsert runs in a subtransaction;
//! when it fails with 21000 the subtransaction is rolled back and a WARNING
//! `DIAG21000` names the keys written, the keys the source returned more than
//! once, the conflicting TVIEW rows' xmin/cmin/xmax/cmax/ctid and the command ids,
//! before the error is raised again with its SQLSTATE.

use pgrx::datum::DatumWithOid;
use pgrx::pg_sys;
use pgrx::pg_sys::panic::CaughtError;
use pgrx::prelude::*;

/// Run `upsert`, logging what conflicted when it fails with 21000.
///
/// # Panics
/// Re-raises the error `upsert` raised.
pub(super) fn upsert<R>(
    qi_tv: &str,
    qi_pk: &str,
    source_sql: &str,
    args: &[DatumWithOid],
    upsert: impl FnOnce() -> R,
) -> R {
    if !crate::config::test_diag_21000() {
        return upsert();
    }
    // SAFETY: backend globals read and restored on the backend's own thread; the
    // subtransaction is released or rolled back on every path below.
    let (context, owner, command_before) = unsafe {
        let saved = (
            pg_sys::CurrentMemoryContext,
            pg_sys::CurrentResourceOwner,
            pg_sys::GetCurrentCommandId(false),
        );
        pg_sys::BeginInternalSubTransaction(std::ptr::null());
        pg_sys::MemoryContextSwitchTo(saved.0);
        saved
    };
    let upsert = std::panic::AssertUnwindSafe(upsert);
    PgTryBuilder::new(move || {
        let wrapped = upsert;
        let out = (wrapped.0)();
        // SAFETY: closes the subtransaction opened above.
        unsafe {
            pg_sys::ReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(context);
            pg_sys::CurrentResourceOwner = owner;
        }
        out
    })
    .catch_others(|error| {
        // SAFETY: rolls back the subtransaction opened above.
        unsafe {
            pg_sys::MemoryContextSwitchTo(context);
            pg_sys::RollbackAndReleaseCurrentSubTransaction();
            pg_sys::MemoryContextSwitchTo(context);
            pg_sys::CurrentResourceOwner = owner;
        }
        let cardinality = match &error {
            CaughtError::PostgresError(report) | CaughtError::ErrorReport(report) => {
                report.sql_error_code() == PgSqlErrorCode::ERRCODE_CARDINALITY_VIOLATION
            }
            CaughtError::RustPanic { .. } => false,
        };
        if cardinality {
            let report = report(qi_tv, qi_pk, source_sql, args);
            // SAFETY: a backend global read on the backend's own thread.
            let command_now = unsafe { pg_sys::GetCurrentCommandId(false) };
            pgrx::warning!(
                "DIAG21000 tv={qi_tv} cid_before={command_before} cid_now={command_now} {report}"
            );
        }
        error.rethrow()
    })
    .execute()
}

/// The keys, the source's duplicate keys and the conflicting rows, as one line.
fn report(qi_tv: &str, qi_pk: &str, source_sql: &str, args: &[DatumWithOid]) -> String {
    crate::utils::spi::connect(|client| {
        let lines = |sql: &str| -> String {
            client.select(sql, None, args).map_or_else(
                |e| format!("<{e}>"),
                |rows| {
                    rows.map(|row| row.get::<String>(1).ok().flatten().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(" | ")
                },
            )
        };
        // Every query reads the source again with the upsert's arguments, so none
        // depends on how they are laid out.
        let keys = lines(&format!(
            "SELECT pg_catalog.string_agg(s.{qi_pk}::pg_catalog.text, ',') FROM ({source_sql}) s"
        ));
        let duplicates = lines(&format!(
            "SELECT s.{qi_pk}::pg_catalog.text || ' x' || pg_catalog.count(*) \
             FROM ({source_sql}) s GROUP BY s.{qi_pk} HAVING pg_catalog.count(*) > 1"
        ));
        let rows = lines(&format!(
            "SELECT pg_catalog.format('%s xmin=%s cmin=%s xmax=%s cmax=%s ctid=%s', \
                    t.{qi_pk}, t.xmin, t.cmin, t.xmax, t.cmax, t.ctid) \
             FROM {qi_tv} t WHERE t.{qi_pk} IN (SELECT s.{qi_pk} FROM ({source_sql}) s)"
        ));
        let xact = lines("SELECT pg_catalog.pg_current_xact_id_if_assigned()::pg_catalog.text");
        format!("keys={keys} source_duplicates=[{duplicates}] rows=[{rows}] xact={xact}")
    })
}
