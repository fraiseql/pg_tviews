//! The transaction callbacks: keep the savepoint stack in step with the open
//! subtransactions, and reset what a transaction kept in memory when it ends.
//! No SPI here, and nothing may fail.

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::os::raw::c_void;

/// What the end of a transaction resets, however it ends: the refresh work, the
/// savepoints, the value locks held, the running queries, the flush, the
/// per-transaction caches, the audit buffer, the metrics and the affected-rows
/// report.
const RESET_AT_END: &[fn()] = &[
    crate::queue::state::clear,
    super::savepoint::clear,
    crate::concurrency::clear,
    crate::executor::reset,
    super::Flushing::reset,
    crate::cache::end_transaction,
    crate::audit::clear_audit_buffer,
    crate::metrics::metrics_api::reset_metrics,
    crate::queue::affected::clear,
];

/// What an abort resets besides: the catalog the caches memoized may be rolled
/// back with it (a TVIEW create that failed leaves no view), the
/// suspension, the revision check, the hook's pending CTAS and the crash-recovery
/// checks.
const RESET_ON_ABORT: &[fn()] = &[
    crate::cache::invalidate_all,
    crate::suspend::force_resume,
    crate::revision::reset,
    || crate::internal_ddl::release_on_abort(true),
    crate::queue::ops::clear_crash_recovery_cache,
];

/// What a rolled-back subtransaction resets besides its savepoint: the hook's
/// pending CTAS (it never reached the event trigger), the cached catalog of DDL it
/// undid, and the crash-recovery rebuilds it undid.
const RESET_ON_SUBABORT: &[fn()] = &[
    || crate::internal_ddl::release_on_abort(false),
    crate::cache::invalidate_all,
    crate::queue::ops::clear_crash_recovery_cache,
];

fn run(resets: &[fn()]) {
    for reset in resets {
        reset();
    }
}

/// Transaction event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XactEvent {
    Commit,
    Abort,
    PreCommit,
    Prepare, // XACT_EVENT_PREPARE
}

/// Register the transaction callback (called from enqueue logic)
///
/// This uses `PostgreSQL`'s `RegisterXactCallback` FFI to install our handler.
/// The callback will be invoked at transaction commit/abort.
pub unsafe fn register_xact_callback() {
    // SAFETY: Called from PostgreSQL backend context. RegisterXactCallback
    // registers a valid extern "C" callback function pointer.
    unsafe {
        pg_sys::RegisterXactCallback(Some(tview_xact_callback), std::ptr::null_mut());
    }
}

/// Register the subtransaction callback for savepoint support
///
/// This uses `PostgreSQL`'s `RegisterSubXactCallback` FFI to handle savepoints.
/// The callback will be invoked when savepoints are created/released/rolled back.
pub unsafe fn register_subxact_callback() {
    // SAFETY: Called from PostgreSQL backend context. RegisterSubXactCallback
    // registers a valid extern "C" callback function pointer.
    unsafe {
        pg_sys::RegisterSubXactCallback(Some(tview_subxact_callback), std::ptr::null_mut());
    }

    // Loaded inside a DO block: subtransactions may already be open. Mark them,
    // so their end events pair with a savepoint.
    // SAFETY: reads the backend's transaction state.
    let nest_level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
    for _ in 0..usize::try_from(nest_level).unwrap_or(0).saturating_sub(1) {
        super::savepoint::start();
    }
}

/// Transaction callback handler (invoked by `PostgreSQL`)
///
/// Runs at the end of every transaction, however it ends. It only drops
/// in-memory state and reports: no SPI is allowed here, and nothing in it may
/// fail (`#[pg_guard]` turns a bug's panic into an ERROR instead of unwinding
/// through `xact.c`).
#[pg_guard]
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn tview_xact_callback(event: u32, _arg: *mut c_void) {
    // Map PostgreSQL XactEvent C enum to our Rust enum.
    // Use pg_sys constants to be version-safe.
    #[allow(non_upper_case_globals)] // Reason: pg_sys XactEvent constants use UPPER_CASE naming
    let xact_event = match event {
        pg_sys::XactEvent::XACT_EVENT_COMMIT => XactEvent::Commit,
        pg_sys::XactEvent::XACT_EVENT_PRE_COMMIT => XactEvent::PreCommit,
        pg_sys::XactEvent::XACT_EVENT_ABORT => XactEvent::Abort,
        pg_sys::XactEvent::XACT_EVENT_PREPARE => XactEvent::Prepare,
        _ => return, // Ignore PARALLEL_*, PRE_PREPARE, etc.
    };

    // Handle event.
    //
    // NOTE: SPI is NOT available during transaction callbacks (PRE_COMMIT, COMMIT, ABORT).
    // Executing SPI queries here crashes the server. Queue flush (which uses SPI) is
    // handled by the ProcessUtility hook intercepting COMMIT instead.
    match xact_event {
        XactEvent::PreCommit | XactEvent::Commit => {
            // Every path that enqueues flushes before the commit (statement
            // trigger, ProcessUtility hook). Work still queued fails the commit
            // while it can still fail: it must never run under another
            // transaction's snapshot, and dropping it would leave TVIEWs stale.
            if xact_event == XactEvent::PreCommit {
                fail_unflushed();
            }
            // Suspended without resuming: the hook caught up before an explicit
            // COMMIT; an implicit commit ends here, where no SPI is allowed.
            if crate::suspend::is_suspended() {
                let stale = crate::suspend::get_changed_entities();
                if !stale.is_empty() {
                    warning!(
                        "pg_tviews: transaction committed with refresh suspended; TVIEWs {:?} \
                         are stale until pg_tviews_refresh() is run for each",
                        stale
                    );
                }
                crate::suspend::clear_changed_entities();
            }

            crate::suspend::force_resume();

            // The crash-recovery check stays done for this backend: an UNLOGGED
            // TVIEW is only reset by a restart, which ends every backend.
            run(RESET_AT_END);
        }
        XactEvent::Prepare => {
            // The ProcessUtility hook flushed the queue before PREPARE TRANSACTION, so
            // the refresh writes are part of the prepared transaction. This backend's
            // transaction ends here: drop its in-memory state (no SPI in callbacks).
            crate::suspend::force_resume();
            crate::queue::ops::clear_crash_recovery_cache();
            run(RESET_AT_END);
        }
        XactEvent::Abort => {
            run(RESET_ON_ABORT);
            run(RESET_AT_END);
        }
    }
}

/// Fail the commit of a transaction that still has refresh work queued (no SPI:
/// this runs in the transaction callback, at `PRE_COMMIT`, before the point of no
/// return).
fn fail_unflushed() {
    let queued = crate::queue::state::get_queue_contents();
    if queued.is_empty() {
        return;
    }
    let entities: std::collections::BTreeSet<&str> =
        queued.iter().map(|k| k.entity.as_str()).collect();
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
        format!(
            "pg_tviews: cannot commit with {} queued refreshes for {:?}: they were never \
             applied (missing flush trigger?)",
            queued.len(),
            entities
        ),
        function_name!(),
    )
    .set_hint(
        "Run tviews.pg_tviews_health_check() to find missing triggers, and \
         tviews.pg_tviews_refresh(entity) to rebuild the TVIEWs named.",
    )
    .report(PgLogLevel::ERROR);
}

/// Subtransaction callback handler (invoked by `PostgreSQL` for savepoints)
///
/// Keeps [`super::savepoint`] in step with the open subtransactions. The
/// savepoint is popped and restored first; cache work comes after, so nothing
/// that follows can leave the stack out of step. Infallible by construction;
/// `#[pg_guard]` turns a bug's panic into an ERROR.
#[pg_guard]
#[unsafe(no_mangle)]
unsafe extern "C-unwind" fn tview_subxact_callback(
    event: u32,
    _subxid: pg_sys::SubTransactionId,
    _parent_subid: pg_sys::SubTransactionId,
    _arg: *mut c_void,
) {
    match event {
        pg_sys::SubXactEvent::SUBXACT_EVENT_START_SUB => super::savepoint::start(),
        pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => super::savepoint::commit(),
        pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB => {
            super::savepoint::abort();
            run(RESET_ON_SUBABORT);
        }
        _ => {}
    }
}
