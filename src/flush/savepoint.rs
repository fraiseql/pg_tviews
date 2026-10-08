//! What a subtransaction rolls back, on one stack.
//!
//! When a subtransaction starts, [`start`] records where the session state
//! stands: the pending refresh work, how many queries are running, and the
//! suspension. Rolling it back ([`abort`]) restores all three; committing it
//! ([`commit`]) keeps what it did. The subtransaction callback is the only
//! caller, so the stack depth always equals the number of open subtransactions.

use crate::queue::state;
use std::cell::RefCell;

/// The session state a subtransaction started from.
struct Savepoint {
    pending: state::Mark,
    frames: usize,
    suspension: crate::suspend::Suspension,
}

thread_local! {
    static SAVEPOINTS: RefCell<Vec<Savepoint>> = const { RefCell::new(Vec::new()) };
}

/// A subtransaction started.
pub fn start() {
    let savepoint = Savepoint {
        pending: state::mark(),
        frames: crate::executor::depth(),
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

/// The innermost subtransaction rolled back: the pending work, the running
/// queries (an error may have skipped their frames) and the suspension are what
/// they were when it started.
pub fn abort() {
    if let Some(savepoint) = SAVEPOINTS.with_borrow_mut(Vec::pop) {
        state::rollback(savepoint.pending);
        crate::executor::truncate_to(savepoint.frames);
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
