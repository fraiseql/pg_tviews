use std::collections::HashSet;
use super::key::RefreshKey;
use super::state::{TX_REFRESH_QUEUE, TX_REFRESH_SCHEDULED};
use crate::TViewResult;

/// Enqueue a refresh request for the given entity and pk
///
/// This is the main entry point from triggers.
/// Deduplication is automatic (HashSet).
#[allow(dead_code)]
pub fn enqueue_refresh(entity: &str, pk: i64) -> TViewResult<()> {
    let key = RefreshKey {
        entity: entity.to_string(),
        pk,
    };

    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        queue.insert(key);
    });

    Ok(())
}

/// Take a snapshot of the current queue and clear it
///
/// Called by commit handler to get all pending refreshes.
/// Thread-local state is cleared after snapshot.
#[allow(dead_code)]
pub fn take_queue_snapshot() -> HashSet<RefreshKey> {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        std::mem::take(&mut *queue)
    })
}

/// Clear the queue (used on transaction abort)
#[allow(dead_code)]
pub fn clear_queue() {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().clear();
    });
}

/// Register transaction commit callback (once per transaction)
pub fn register_commit_callback_once() -> TViewResult<()> {
    TX_REFRESH_SCHEDULED.with(|flag| {
        let mut scheduled = flag.borrow_mut();
        if *scheduled {
            // Already registered, skip
            return Ok(());
        }

        // Register transaction and subtransaction callbacks
        unsafe {
            super::xact::register_xact_callback()?;
            super::xact::register_subxact_callback()?;
        }

        *scheduled = true;
        Ok(())
    })
}

/// Reset the scheduled flag (called after commit/abort)
#[allow(dead_code)]
pub fn reset_scheduled_flag() {
    TX_REFRESH_SCHEDULED.with(|flag| {
        *flag.borrow_mut() = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_snapshot() {
        clear_queue();

        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 2).unwrap();
        enqueue_refresh("user", 1).unwrap(); // duplicate

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2); // Deduplicated

        // Queue should be empty after snapshot
        let empty_snapshot = take_queue_snapshot();
        assert_eq!(empty_snapshot.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        clear_queue();

        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 2).unwrap();

        clear_queue();

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 0);
    }
}