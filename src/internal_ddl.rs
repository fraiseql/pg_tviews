//! The guard that lets `pg_tviews` run its own DDL through the `ProcessUtility`
//! hook unhandled.
//!
//! Only the statements `pg_tviews` issues itself pass: any other statement is
//! handled by the hook.

use pgrx::pg_sys;
use std::cell::{Cell, RefCell};
use std::ffi::c_char;

thread_local! {
    /// The transaction nest level of every [`InternalDdl`] alive, outermost first:
    /// a (sub)transaction abort that unwinds past one releases it.
    static SCOPES: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
    /// The text of the statement `pg_tviews` is executing inside a scope
    /// ([`issuing`]): `PostgreSQL` hands the hook that very pointer for it.
    static ISSUING: Cell<*const c_char> = const { Cell::new(std::ptr::null()) };
}

/// While alive, `pg_tviews` works for a statement: the DDL it issues itself
/// ([`issuing`]) passes through the hook unhandled. The hook holds one while it
/// works on a statement (never while the statement itself runs), and `pg_tviews`
/// around its own `GRANT`s and owner changes, which the hook would otherwise
/// follow mid-way. Released when dropped, also when an error unwinds past it.
pub(crate) struct InternalDdl {
    depth: usize,
}

impl InternalDdl {
    pub(crate) fn begin() -> Self {
        // SAFETY: reads the current transaction nest level.
        let level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
        let depth = SCOPES.with_borrow_mut(|scopes| {
            scopes.push(level);
            scopes.len() - 1
        });
        Self { depth }
    }

    /// Whether `query_string`, the text the hook was handed, is the statement
    /// `pg_tviews` is issuing inside a scope.
    pub(crate) fn issued(query_string: *const c_char) -> bool {
        let issuing = ISSUING.get();
        !issuing.is_null() && std::ptr::eq(issuing, query_string)
    }
}

impl Drop for InternalDdl {
    fn drop(&mut self) {
        SCOPES.with_borrow_mut(|scopes| scopes.truncate(self.depth));
    }
}

/// While alive, `sql` is the statement `pg_tviews` is executing.
pub(crate) struct Issuing {
    previous: *const c_char,
}

/// Mark `sql` (the pointer `PostgreSQL` executes and hands the hook) as the
/// statement `pg_tviews` issues, when a scope is open; `None` outside one, where
/// the hook handles `pg_tviews`' statements as any other.
pub(crate) fn issuing(sql: *const c_char) -> Option<Issuing> {
    SCOPES
        .with_borrow(|scopes| !scopes.is_empty())
        .then(|| Issuing {
            previous: ISSUING.replace(sql),
        })
}

impl Drop for Issuing {
    fn drop(&mut self) {
        ISSUING.set(self.previous);
    }
}

/// Release the scopes of the hook invocations an abort unwound past.
///
/// An `ereport(ERROR)` raised while the hook runs a statement (e.g. a CTAS inside a DO
/// block failing with "already exists", caught by the block's EXCEPTION clause) may
/// longjmp past the hook before its scope is dropped. The scopes opened at or above
/// the aborting (sub)transaction's level are released here; those of an enclosing,
/// still-running hook invocation (lower level) are left alone. `whole_xact` is true
/// for a top-level transaction abort. No SPI: safe in callbacks.
pub fn release_on_abort(whole_xact: bool) {
    // SAFETY: reads the current transaction nest level.
    let level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
    SCOPES.with_borrow_mut(|scopes| {
        let kept = if whole_xact {
            0
        } else {
            scopes.iter().take_while(|&&at| at < level).count()
        };
        scopes.truncate(kept);
        if scopes.is_empty() {
            ISSUING.set(std::ptr::null());
        }
    });
}
