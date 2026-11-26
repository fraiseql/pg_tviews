//! The drain of `tviews.pg_tview_time_handoff`: the keys a writer older than a
//! time boundary wrote after it was announced (`crate::boundary`), refreshed again
//! at an instant after the boundary.
//!
//! A transaction that handed keys off starts a worker as it commits. The worker
//! waits for that transaction to end, then refreshes every key handed off in its
//! database in a transaction of its own, whose `now()` is after every announced
//! boundary. The time refresh drains a TVIEW's keys too, before it compares the
//! TVIEW: a backstop for a worker that could not start.

use pgrx::bgworkers::{BackgroundWorker, BackgroundWorkerBuilder, SignalWakeFlags};
use pgrx::prelude::*;

/// Refresh the keys handed off for `entity` (every TVIEW: `None`), committed ones
/// only, and forget them. Returns how many rows of the hand-off were drained.
///
/// # Errors
/// Returns an error if the hand-off cannot be read or a refresh fails.
pub fn drain(entity: Option<&str>) -> crate::TViewResult<usize> {
    let handed = crate::owner::as_extension_owner(|| {
        crate::utils::spi::rows_written(
            &format!(
                "DELETE FROM {}.pg_tview_time_handoff h \
                 WHERE $1::pg_catalog.text IS NULL OR h.entity OPERATOR(pg_catalog.=) $1 \
                 RETURNING h.entity, h.pk",
                crate::utils::ext_schema()
            ),
            &[crate::utils::spi::text(entity)],
            |row| {
                let entity: Option<String> = row.get(1)?;
                let pk: Option<i64> = row.get(2)?;
                Ok((entity.unwrap_or_default(), pk))
            },
        )
    })?;
    let drained = handed.len();
    let mut keys: std::collections::BTreeMap<String, Option<Vec<i64>>> =
        std::collections::BTreeMap::new();
    for (entity, pk) in handed {
        let entry = keys.entry(entity).or_insert_with(|| Some(Vec::new()));
        match (entry.as_mut(), pk) {
            (Some(pks), Some(pk)) => pks.push(pk),
            _ => *entry = None,
        }
    }
    for (entity, pks) in keys {
        crate::concurrency::note_link_change();
        match pks {
            None => crate::queue::enqueue_refresh_all(&entity),
            Some(pks) => crate::queue::enqueue_refresh_bulk(
                &entity,
                pks.into_iter()
                    .map(crate::queue::key::KeyValue::Int)
                    .collect(),
            ),
        }
    }
    crate::flush::flush_refresh_queue()?;
    Ok(drained)
}

/// Start the drain worker of this database for the committing transaction, from
/// its `PRE_COMMIT`. A worker that cannot start is logged: the next time refresh
/// of each TVIEW drains its keys.
pub fn start_for_committing() {
    // SAFETY: backend globals and the current transaction's id, read in a
    // transaction about to commit; a hand-off wrote rows, so it has an XID.
    let (database, xid) = unsafe {
        (
            pg_sys::MyDatabaseId.to_u32(),
            pg_sys::GetCurrentTransactionIdIfAny(),
        )
    };
    let started = BackgroundWorkerBuilder::new("pg_tviews time hand-off drain")
        .set_type("pg_tviews time drain")
        .set_library("pg_tviews")
        .set_function("pg_tviews_time_drain_main")
        .set_argument(Some(pg_sys::Datum::from(database)))
        .set_extra(&xid.into_inner().to_string())
        .enable_spi_access()
        .load_dynamic();
    if let Err(e) = started {
        log!("pg_tviews: could not start the time hand-off drain: {e:?}");
    }
}

/// Worker entry point: wait for the transaction that handed keys off, then drain
/// every key handed off in the database.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn pg_tviews_time_drain_main(arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGTERM);
    #[allow(clippy::cast_possible_truncation)]
    // Reason: the argument holds an OID, set by start_for_committing
    let database = pg_sys::Oid::from(arg.value() as u32);
    let xid: u32 = BackgroundWorker::get_extra().parse().unwrap_or(0);
    // SAFETY: a background worker with database access, connecting once, before
    // any transaction.
    unsafe {
        pg_sys::BackgroundWorkerInitializeConnectionByOid(database, pg_sys::InvalidOid, 0);
    }
    BackgroundWorker::transaction(|| {
        // SAFETY: in a transaction; waits until transaction `xid` ended (at once
        // when it already has), as a row lock waits on its holder.
        unsafe {
            pg_sys::XactLockTableWait(
                pg_sys::TransactionId::from(xid),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                pg_sys::XLTW_Oper::XLTW_None,
            );
        }
    });
    BackgroundWorker::transaction(|| {
        // As the rebuild worker: a fixed search_path, each TVIEW refreshed as its
        // owner.
        if let Err(e) = crate::utils::spi::run(
            "SELECT pg_catalog.set_config('search_path', 'pg_catalog, pg_temp', true)",
            &[],
        ) {
            error!("pg_tviews: the time hand-off drain could not set search_path: {e}");
        }
        if let Err(e) = drain(None) {
            error!("pg_tviews: the time hand-off drain failed: {e}");
        }
    });
}
