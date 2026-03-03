use std::collections::HashSet;
use super::key::RefreshKey;
use super::state::TX_REFRESH_QUEUE;

/// Enqueue a refresh request for the given entity and pk
///
/// This is the main entry point from triggers.
/// Deduplication is automatic (`HashSet`).
pub fn enqueue_refresh(entity: &str, pk: i64) {
    let key = RefreshKey {
        entity: entity.to_string(),
        pk,
    };

    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();
        queue.insert(key);
    });
}

/// Bulk enqueue refresh requests for multiple PKs of the same entity
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) {
    TX_REFRESH_QUEUE.with(|q| {
        let mut queue = q.borrow_mut();

        // Insert all keys at once (HashSet deduplicates automatically)
        for pk in pks {
            let key = RefreshKey {
                entity: entity.to_string(),
                pk,
            };
            queue.insert(key);
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