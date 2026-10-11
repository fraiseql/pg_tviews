//! The flush: applies the refresh work a transaction queued (`crate::queue`) to
//! the TVIEWs, dependencies first, following each refreshed row to the TVIEWs
//! that embed it (ADR 0203).
//!
//! - [`drain`]: the loop that takes the queue and refreshes one entity at a time
//!   in dependency order until nothing is left;
//! - [`apply`]: how one entity's keys are applied (full refresh, direct patch,
//!   recompute, fan-out) and their parents found;
//! - [`xact`]: the transaction callbacks, and what the end of a transaction resets.

mod apply;
mod drain;
mod savepoint;
mod xact;

pub use xact::{register_subxact_callback, register_xact_callback};

use crate::TViewResult;

/// Flush the refresh queue: process all pending TVIEW refreshes.
///
/// Called after each statement (the flush trigger), by the `ProcessUtility` hook
/// before COMMIT and PREPARE TRANSACTION, and by the functions that must see
/// fresh TVIEWs. SPI must be available, so never from a transaction callback.
///
/// # Correctness
///
/// - Each (entity, key) is refreshed at most once per flush.
/// - An entity is refreshed after every TVIEW it reads: the next entity is the
///   first pending one in topological order, and propagation only adds entities
///   after it.
/// - The first error aborts the transaction.
///
/// # Errors
/// What a refresh, a catalog read or a propagation query returns, and
/// [`crate::TViewError::DepthExceeded`] when propagation does not settle.
pub fn flush_refresh_queue() -> TViewResult<()> {
    // A refresh write can fire a flush trigger (a TVIEW's table read by another
    // TVIEW): that call returns at once, and the running flush drains what it
    // queued before finishing.
    let Some(_flushing) = Flushing::enter() else {
        return Ok(());
    };
    drain::flush_pending()
}

thread_local! {
    static FLUSHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While alive, a flush is running in this backend. Dropped when the flush
/// returns or unwinds; the transaction's end clears it too.
struct Flushing;

impl Flushing {
    fn enter() -> Option<Self> {
        if FLUSHING.replace(true) {
            return None;
        }
        Some(Self)
    }

    /// No flush is running (the transaction ended).
    fn reset() {
        FLUSHING.set(false);
    }
}

impl Drop for Flushing {
    fn drop(&mut self) {
        Self::reset();
    }
}
