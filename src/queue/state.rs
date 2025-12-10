use std::cell::RefCell;
use std::collections::HashSet;
use super::key::RefreshKey;

thread_local! {
    /// Transaction-local queue of refresh requests
    ///
    /// - Populated by triggers on tb_* tables
    /// - Deduplicated automatically (HashSet)
    /// - Flushed at commit time by tx_commit_handler()
    /// - Cleared on transaction abort
    pub static TX_REFRESH_QUEUE: RefCell<HashSet<RefreshKey>> = RefCell::new(HashSet::new());

    /// Flag: has commit callback been registered for this transaction?
    ///
    /// - Set to true when first refresh is enqueued
    /// - Prevents multiple callback registrations
    /// - Reset to false after commit/abort
    pub static TX_REFRESH_SCHEDULED: RefCell<bool> = const { RefCell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_thread_local() {
        // Each thread gets its own queue
        TX_REFRESH_QUEUE.with(|q| {
            let mut queue = q.borrow_mut();
            queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });
            assert_eq!(queue.len(), 1);
        });

        // Clear for next test
        TX_REFRESH_QUEUE.with(|q| q.borrow_mut().clear());
    }
}