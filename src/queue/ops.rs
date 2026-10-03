use super::key::{KeyValue, RefreshKey};
use super::state::{TX_CRASH_RECOVERY_CHECKED, TX_REFRESH_QUEUE};
use std::collections::HashSet;

/// Check queue size against a limit and raise ERROR if exceeded.
/// Returns Ok(()) if the queue size is still below the limit.
/// Returns Err with a descriptive message if the limit would be exceeded.
fn check_queue_backpressure(limit: usize) -> Result<(), String> {
    let current_size = TX_REFRESH_QUEUE.with(|q| q.borrow().len());
    if current_size >= limit {
        return Err(format!(
            "refresh queue backpressure: queue size ({current_size}) would exceed max_queue_size ({limit})"
        ));
    }
    Ok(())
}

/// Internal helper: enqueue a single refresh with explicit limit.
/// Used for testability and backpressure enforcement.
pub fn enqueue_refresh_with_limit(entity: &str, key: KeyValue, limit: usize) -> Result<(), String> {
    check_queue_backpressure(limit)?;
    TX_REFRESH_QUEUE.with(|q| {
        q.borrow_mut().insert(RefreshKey::new(entity, key));
    });
    Ok(())
}

/// Enqueue the refresh of the row of `entity` whose identity is `key`.
///
/// This is the main entry point from triggers.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
///
/// A plain enqueue **poisons** any direct patch for this key (issue #56): the key
/// will recompute. Only [`enqueue_refresh_patched`] preserves a fast-path patch.
pub fn enqueue_refresh(entity: &str, key: KeyValue) {
    if let Err(msg) =
        enqueue_refresh_with_limit(entity, key.clone(), crate::config::max_queue_size())
    {
        pgrx::error!("{}", msg);
    }
    super::patch::poison(RefreshKey::new(entity, key));
}

/// Enqueue a PK-based refresh **and** record a direct patch (issue #56 fast path).
///
/// Inserts `(entity, pk)` into the refresh queue under the same backpressure limit
/// as [`enqueue_refresh`], then records the captured `fields` as a top-level
/// (`prefix = []`) direct patch. If the key was already poisoned this transaction
/// (e.g. an INSERT then UPDATE of the same row), the patch is ignored and the key
/// recomputes — a patch alone cannot materialise a not-yet-created row.
pub fn enqueue_refresh_patched(
    entity: &str,
    pk: i64,
    fields: serde_json::Map<String, serde_json::Value>,
) {
    if let Err(msg) =
        enqueue_refresh_with_limit(entity, KeyValue::Int(pk), crate::config::max_queue_size())
    {
        pgrx::error!("{}", msg);
    }
    // Count the capture once per fresh key: a base table feeding several tviews
    // has several row triggers that each re-record the same key in one statement.
    if super::patch::record(RefreshKey::pk(entity, pk), Vec::new(), fields) {
        crate::metrics::metrics_api::record_direct_patch_captured();
    }
}

/// Enqueue a refresh of every row of `entity`'s TVIEW (issues #157, #158). One
/// entry however many rows the statement changes; the flush absorbs the entity's
/// per-key entries into it.
pub fn enqueue_refresh_all(entity: &str) {
    TX_REFRESH_QUEUE.with(|q| {
        let key = RefreshKey::all(entity);
        if q.borrow().contains(&key) {
            return;
        }
        check_queue_backpressure(crate::config::max_queue_size()).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        q.borrow_mut().insert(key);
    });
}

/// Bulk enqueue refresh requests for several rows of the same entity.
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
pub fn enqueue_refresh_bulk(entity: &str, keys: Vec<KeyValue>) {
    TX_REFRESH_QUEUE.with(|q| {
        let limit = crate::config::max_queue_size();
        check_queue_backpressure(limit).unwrap_or_else(|msg| {
            pgrx::error!("{}", msg);
        });
        let mut queue = q.borrow_mut();
        for key in &keys {
            queue.insert(RefreshKey::new(entity, key.clone()));
        }
    });
    // Plain (bulk) enqueue poisons any direct patches for these keys (issue #56).
    for key in keys {
        super::patch::poison(RefreshKey::new(entity, key));
    }
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

/// Check if crash recovery has already been checked for this entity in this transaction
pub fn is_crash_recovery_checked(entity: &str) -> bool {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| checked.borrow().contains(entity))
}

/// Mark that crash recovery has been checked for this entity in this transaction
pub fn mark_crash_recovery_checked(entity: &str) {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| {
        checked.borrow_mut().insert(entity.to_string());
    });
}

/// Clear the crash recovery check cache (used on transaction abort)
pub fn clear_crash_recovery_cache() {
    TX_CRASH_RECOVERY_CHECKED.with(|checked| {
        checked.borrow_mut().clear();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_snapshot() {
        clear_queue();

        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap();
        enqueue_refresh_with_limit("post", KeyValue::Int(2), usize::MAX).unwrap();
        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap(); // duplicate

        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2); // Deduplicated

        // Queue should be empty after snapshot
        let empty_snapshot = take_queue_snapshot();
        assert_eq!(empty_snapshot.len(), 0);
    }

    #[test]
    fn test_clear_queue() {
        clear_queue();

        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap();
        enqueue_refresh_with_limit("post", KeyValue::Int(2), usize::MAX).unwrap();

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
        enqueue_refresh_with_limit("user", KeyValue::Int(1), limit)
            .expect("first insert should succeed");

        // Should succeed: queue size is 1, adding 1 (total 2) doesn't exceed limit
        enqueue_refresh_with_limit("post", KeyValue::Int(2), limit)
            .expect("second insert should succeed");

        // Should fail: queue size is 2, adding 1 (total 3) exceeds limit
        assert!(
            enqueue_refresh_with_limit("user", KeyValue::Int(3), limit).is_err(),
            "third insert should fail"
        );

        // Verify queue only has 2 items (backpressure prevented the 3rd)
        let snapshot = take_queue_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
