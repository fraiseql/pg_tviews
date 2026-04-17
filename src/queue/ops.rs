use super::key::RefreshKey;
use super::state::TX_REFRESH_QUEUE;
use std::collections::HashSet;

/// Check queue size against a limit and raise ERROR if exceeded.
/// Returns Ok(()) if the queue size is still below the limit.
/// Returns Err with a descriptive message if the limit would be exceeded.
fn check_queue_backpressure(limit: usize) -> Result<(), String> {
    let current_size = TX_REFRESH_QUEUE.with(|q| q.borrow().len());
    if current_size >= limit {
        return Err(format!(
            "refresh queue backpressure: queue size ({}) would exceed max_queue_size ({})",
            current_size, limit
        ));
    }
    Ok(())
}

/// Internal helper: enqueue a single PK-based refresh with explicit limit.
/// Used for testability and backpressure enforcement.
pub fn enqueue_refresh_with_limit(entity: &str, pk: i64, limit: usize) -> Result<(), String> {
    check_queue_backpressure(limit)?;
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().insert(RefreshKey::pk(entity, pk));
    });
    Ok(())
}

/// Enqueue a standard PK-based refresh request.
///
/// This is the main entry point from triggers for normal TVIEWs.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if max_queue_size would be exceeded.
pub fn enqueue_refresh(entity: &str, pk: i64) {
    if let Err(msg) = enqueue_refresh_with_limit(entity, pk, crate::config::max_queue_size()) {
        pgrx::error!("{}", msg);
    }
}

/// Enqueue a DISTINCT ON dedup-key refresh request.
///
/// Used by triggers on DISTINCT ON TVIEWs.  The `dedup_key` is the value
/// of the DISTINCT ON column (cast to TEXT) identifying the group to re-evaluate.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if max_queue_size would be exceeded.
pub fn enqueue_refresh_dedup(entity: &str, dedup_key: &str) {
    TX_REFRESH_QUEUE.with(|q| {
        let limit = crate::config::max_queue_size();
        check_queue_backpressure(limit).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        q.borrow_mut().insert(RefreshKey::dedup(entity, dedup_key));
    });
}

/// Bulk enqueue PK-based refresh requests for multiple PKs of the same entity.
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if max_queue_size would be exceeded.
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) {
    TX_REFRESH_QUEUE.with(|q| {
        let limit = crate::config::max_queue_size();
        check_queue_backpressure(limit).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        let mut queue = q.borrow_mut();
        for pk in pks {
            queue.insert(RefreshKey::pk(entity, pk));
        }
    });
}

/// Take a snapshot of the current queue and clear it
///
/// Called by commit handler to get all pending refreshes.
/// Thread-local state is cleared after snapshot.
pub fn take_queue_snapshot() -> HashSet<RefreshKey> {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        std::mem::take(&mut *queue)
    })
}

/// Check whether the refresh queue has any pending items (non-destructive).
pub fn is_queue_empty() -> bool {
    TX_REFRESH_QUEUE.with(|q| q.borrow().is_empty())
}

/// Clear the queue (used on transaction abort)
pub fn clear_queue() {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().clear();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_snapshot() {
        clear_queue();

        enqueue_refresh("user", 1);
        enqueue_refresh("post", 2);
        enqueue_refresh("user", 1); // duplicate

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2); // Deduplicated

        // Queue should be empty after snapshot
        let empty_snapshot = take_queue_snapshot();
        assert_eq!(empty_snapshot.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        clear_queue();

        enqueue_refresh("user", 1);
        enqueue_refresh("post", 2);

        clear_queue();

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 0);
    }

    #[test]
    fn test_enqueue_respects_max_queue_size() {
        clear_queue();

        // Test that enqueue_refresh_with_limit raises an error when limit is exceeded.
        // This test calls the internal helper directly with a small limit.
        let limit = 2;

        // Should succeed: queue size is 0, adding 1 (total 1) doesn't exceed limit
        enqueue_refresh_with_limit("user", 1, limit).expect("first insert should succeed");

        // Should succeed: queue size is 1, adding 1 (total 2) doesn't exceed limit
        enqueue_refresh_with_limit("post", 2, limit).expect("second insert should succeed");

        // Should fail: queue size is 2, adding 1 (total 3) exceeds limit
        assert!(
            enqueue_refresh_with_limit("user", 3, limit).is_err(),
            "third insert should fail"
        );

        // Verify queue only has 2 items (backpressure prevented the 3rd)
        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
