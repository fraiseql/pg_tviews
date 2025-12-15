use std::collections::HashSet;
use super::key::RefreshKey;
use super::state::{TX_REFRESH_QUEUE, TX_REFRESH_SCHEDULED};
use crate::TViewResult;

/// Enqueue a refresh request for the given entity and pk
///
/// This is the main entry point from triggers.
/// Deduplication is automatic (`HashSet`).
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

/// Bulk enqueue refresh requests for multiple PKs of the same entity (Phase 9A)
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
#[allow(dead_code)]
pub fn enqueue_refresh_bulk(entity: &str, pks: Vec<i64>) -> TViewResult<()> {
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

/// Clear queue and reset scheduled flag (public API for DISCARD ALL handling)
#[allow(dead_code)]
pub fn clear_queue_and_reset() {
    clear_queue();
    reset_scheduled_flag();
}

/// Queue statistics for introspection
#[derive(Debug, Clone)]
pub struct QueueStats {
    pub size: usize,
    pub entities: Vec<String>,
}

/// Get current queue statistics (for introspection)
///
/// Returns the number of pending refresh requests and list of unique entities.
/// Safe to call from SQL context.
pub fn get_queue_stats() -> QueueStats {
    TX_REFRESH_QUEUE.with(|q| {
        let queue = q.borrow();

        // Count unique entities
        let mut entity_set = std::collections::HashSet::new();
        for key in queue.iter() {
            entity_set.insert(key.entity.clone());
        }

        let entities: Vec<String> = entity_set.into_iter().collect();

        QueueStats {
            size: queue.len(),
            entities,
        }
    })
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

    #[test]
    fn test_get_queue_stats() {
        clear_queue();

        // Empty queue
        let stats = get_queue_stats();
        assert_eq!(stats.size, 0);
        assert!(stats.entities.is_empty());

        // Add some items
        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("user", 2).unwrap();
        enqueue_refresh("post", 1).unwrap();
        enqueue_refresh("user", 1).unwrap(); // duplicate

        let stats = get_queue_stats();
        assert_eq!(stats.size, 3); // 3 unique keys
        assert_eq!(stats.entities.len(), 2); // 2 unique entities
        assert!(stats.entities.contains(&"user".to_string()));
        assert!(stats.entities.contains(&"post".to_string()));

        // Clear and check again
        clear_queue();
        let stats = get_queue_stats();
        assert_eq!(stats.size, 0);
        assert!(stats.entities.is_empty());
    }
}