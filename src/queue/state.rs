use super::key::RefreshKey;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// Transaction-local queue of refresh requests
    ///
    /// - Populated by triggers on `tb_*` tables
    /// - Deduplicated automatically (`HashSet`)
    /// - Flushed at commit time by `tx_commit_handler()`
    /// - Cleared on transaction abort
    pub static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = RefCell::new(HashSet::new());

    /// Transaction-local set of entities that have been checked for post-crash truncation
    ///
    /// - Prevents duplicate EXISTS queries per entity per transaction
    /// - Ensures auto-refresh runs at most once per entity per transaction
    pub static TX_CRASH_RECOVERY_CHECKED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());

}

/// Replace the current queue with a new one (used for savepoint rollback)
pub fn replace_queue(new_queue: HashSet<RefreshKey>) {
    TX_REFRESH_QUEUE.with(|q| {
        *q.borrow_mut() = new_queue;
    });
}

/// Get the current queue size (for metrics)
pub fn get_queue_size() -> usize {
    TX_REFRESH_QUEUE.with(|q| q.borrow().len())
}

/// Get a copy of current queue contents (for debugging)
pub fn get_queue_contents() -> Vec<RefreshKey> {
    TX_REFRESH_QUEUE.with(|q| q.borrow().iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_thread_local() {
        // Each thread gets its own queue
        TX_REFRESH_QUEUE.with(|q| {
            let mut queue = q.borrow_mut();
            queue.insert(RefreshKey::pk("user", 1));
            assert_eq!(queue.len(), 1);
        });

        // Clear for next test
        TX_REFRESH_QUEUE.with(|q| q.borrow_mut().clear());
    }
}
