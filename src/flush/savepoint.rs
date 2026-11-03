//! What a subtransaction rolls back, on one stack.
//!
//! When a subtransaction starts, [`start`] records where the session state
//! stands: the pending refresh work, the rows to publish, the value locks held,
//! the executor's (the queries running, whether work is deferred to them, the
//! last refresh), and the suspension. Rolling it back ([`abort`]) restores all
//! five; committing it
//! ([`commit`]) keeps what it did. The subtransaction callback is the only
//! caller, so the stack depth always equals the number of open subtransactions.

use crate::queue::state;
use std::cell::RefCell;

/// The session state a subtransaction started from.
struct Savepoint {
    pending: state::Mark,
    locks: crate::concurrency::Mark,
    executor: crate::executor::Mark,
    suspension: crate::suspend::Suspension,
}

thread_local! {
    static SAVEPOINTS: RefCell<Vec<Savepoint>> = const { RefCell::new(Vec::new()) };
}

/// A subtransaction started.
pub fn start() {
    let savepoint = Savepoint {
        pending: state::mark(),
        locks: crate::concurrency::mark(),
        executor: crate::executor::mark(),
        suspension: crate::suspend::snapshot(),
    };
    SAVEPOINTS.with_borrow_mut(|s| s.push(savepoint));
}

/// The innermost subtransaction committed: what it did now belongs to its
/// parent.
pub fn commit() {
    if let Some(savepoint) = SAVEPOINTS.with_borrow_mut(Vec::pop) {
        state::release(savepoint.pending);
    }
}

/// The innermost subtransaction rolled back: the pending work, the value locks
/// (the lock manager released those it took), the executor's state (an error may
/// have skipped frames; a flush it ran is undone) and the suspension are what
/// they were when it started.
pub fn abort() {
    if let Some(savepoint) = SAVEPOINTS.with_borrow_mut(Vec::pop) {
        state::rollback(savepoint.pending);
        crate::concurrency::rollback(savepoint.locks);
        crate::executor::rollback(&savepoint.executor);
        crate::suspend::restore(savepoint.suspension);
    }
}

/// The transaction ended: no subtransaction is open.
pub fn clear() {
    SAVEPOINTS.with_borrow_mut(Vec::clear);
}

/// How many subtransactions are open.
#[cfg(test)]
pub fn depth() -> usize {
    SAVEPOINTS.with_borrow(Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::RefreshKey;

    #[test]
    fn depth_follows_start_commit_and_abort() {
        clear();
        state::clear();
        start();
        start();
        abort();
        assert_eq!(depth(), 1);
        start();
        commit();
        commit();
        assert_eq!(depth(), 0);
    }

    #[test]
    fn abort_restores_queue_and_suspension_together() {
        clear();
        state::clear();
        crate::suspend::force_resume();
        state::queue_insert(RefreshKey::pk("user", 1));
        start();
        crate::suspend::suspend();
        state::queue_insert(RefreshKey::pk("user", 2));
        abort();
        assert!(!crate::suspend::is_suspended());
        assert_eq!(state::get_queue_contents(), vec![RefreshKey::pk("user", 1)]);
        state::clear();
    }
}
