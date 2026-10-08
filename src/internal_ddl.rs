//! The guard that lets `pg_tviews` run its own DDL through the `ProcessUtility`
//! hook unhandled.

use pgrx::pg_sys;

/// Set while `pg_tviews` runs its own work for a statement (an [`InternalDdl`] is
/// alive): the DDL it issues through SPI reaches the hook again and must pass
/// through unhandled. Never set while a user's statement runs, so DDL nested in
/// it (a function called by `EXECUTE` or `CREATE TABLE AS`) is intercepted.
static mut HOOK_IN_PROGRESS: bool = false;

/// Transaction nest level at which `HOOK_IN_PROGRESS` was set, so a (sub)transaction abort
/// that unwinds past the hook can release a guard the hook never got to reset.
static mut HOOK_GUARD_LEVEL: i32 = 0;

/// While alive, statements `pg_tviews` runs pass through the hook unhandled. The
/// hook holds one while it works on a statement (never while the statement
/// itself runs), and `pg_tviews` around its own `GRANT`s and owner changes, which
/// the hook would otherwise follow mid-way. Released when dropped, also when an
/// error unwinds past it.
pub(crate) struct InternalDdl {
    took: bool,
}

impl InternalDdl {
    /// Whether `pg_tviews` is running its own DDL.
    pub(crate) fn active() -> bool {
        // SAFETY: single-threaded backend; a plain read of a process-local static.
        unsafe { HOOK_IN_PROGRESS }
    }

    pub(crate) fn begin() -> Self {
        // SAFETY: single-threaded backend; plain reads/writes of process-local statics.
        unsafe {
            if HOOK_IN_PROGRESS {
                return Self { took: false };
            }
            HOOK_IN_PROGRESS = true;
            HOOK_GUARD_LEVEL = pg_sys::GetCurrentTransactionNestLevel();
        }
        Self { took: true }
    }
}

impl Drop for InternalDdl {
    fn drop(&mut self) {
        if self.took {
            // SAFETY: as in `begin`.
            unsafe { HOOK_IN_PROGRESS = false };
        }
    }
}

/// Release the reentrancy guard if the hook invocation that took it was aborted.
///
/// An `ereport(ERROR)` raised while the hook runs a statement (e.g. a CTAS inside a DO
/// block failing with "already exists", caught by the block's EXCEPTION clause) longjmps
/// past the hook before it can reset `HOOK_IN_PROGRESS`. The guard is released here when
/// the aborting (sub)transaction is at or above the level that took it; a guard taken by an
/// enclosing, still-running hook invocation (lower level) is left alone.
/// `whole_xact` is true for a top-level transaction abort. No SPI: safe in callbacks.
pub fn release_on_abort(whole_xact: bool) {
    // SAFETY: single-threaded backend; plain reads/writes of process-local statics.
    unsafe {
        if HOOK_IN_PROGRESS
            && (whole_xact || pg_sys::GetCurrentTransactionNestLevel() <= HOOK_GUARD_LEVEL)
        {
            HOOK_IN_PROGRESS = false;
        }
    }
}
