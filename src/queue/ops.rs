use std::collections::HashSet;
use super::key::RefreshKey;
use super::state::TX_REFRESH_QUEUE;

/// Enqueue a standard PK-based refresh request.
///
/// This is the main entry point from triggers for normal TVIEWs.
/// Deduplication is automatic (`HashSet`).
pub fn enqueue_refresh(entity: &str, pk: i64) {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().insert(RefreshKey::pk(entity, pk));
    });
}

/// Enqueue a DISTINCT ON dedup-key refresh request.
///
/// Used by triggers on DISTINCT ON TVIEWs.  The `dedup_key` is the value
/// of the DISTINCT ON column (cast to TEXT) identifying the group to re-evaluate.
/// Deduplication is automatic (`HashSet`).
pub fn enqueue_refresh_dedup(entity: &str, dedup_key: &str) {
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().insert(RefreshKey::dedup(entity, dedup_key));
    });
}

/// Bulk enqueue PK-based refresh requests for multiple PKs of the same entity.
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) {
    TX_REFRESH_QUEUE.with(|q| {
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
}