use super::key::{KeyValue, RefreshKey};
use super::state::{self, TX_CRASH_RECOVERY_CHECKED};

/// `QueueFull` when one more entry would take the queue past `limit`.
fn check_queue_backpressure(limit: usize) -> crate::TViewResult<()> {
    let size = state::get_queue_size();
    if size >= limit {
        return Err(crate::TViewError::QueueFull {
            size,
            max_size: limit,
        });
    }
    Ok(())
}

/// Internal helper: enqueue a single refresh with explicit limit.
/// Used for testability and backpressure enforcement.
pub fn enqueue_refresh_with_limit(
    entity: &str,
    key: KeyValue,
    limit: usize,
) -> crate::TViewResult<()> {
    check_queue_backpressure(limit)?;
    state::queue_insert(RefreshKey::new(entity, key));
    Ok(())
}

/// Enqueue the refresh of the row of `entity` whose identity is `key`.
///
/// This is the main entry point from triggers.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
///
/// A plain enqueue **poisons** any direct patch for this key: the key
/// will recompute. Only [`enqueue_refresh_patched`] preserves a fast-path patch.
pub fn enqueue_refresh(entity: &str, key: KeyValue) {
    if let Err(e) = enqueue_refresh_with_limit(entity, key.clone(), crate::config::max_queue_size())
    {
        e.raise();
    }
    super::patch::poison(RefreshKey::new(entity, key));
}

/// Enqueue a PK-based refresh **and** record a direct patch.
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
    if let Err(e) =
        enqueue_refresh_with_limit(entity, KeyValue::Int(pk), crate::config::max_queue_size())
    {
        e.raise();
    }
    // Count the capture once per fresh key: a base table feeding several tviews
    // has several row triggers that each re-record the same key in one statement.
    if super::patch::record(RefreshKey::pk(entity, pk), Vec::new(), fields) {
        crate::metrics::metrics_api::record_direct_patch_captured();
    }
}

/// Enqueue a refresh of every row of `entity`'s TVIEW. One
/// entry however many rows the statement changes; the flush absorbs the entity's
/// per-key entries into it.
pub fn enqueue_refresh_all(entity: &str) {
    let key = RefreshKey::all(entity);
    if state::queue_contains(&key) {
        return;
    }
    if let Err(e) = check_queue_backpressure(crate::config::max_queue_size()) {
        e.raise();
    }
    state::queue_insert(key);
}

/// Bulk enqueue refresh requests for several rows of the same entity.
///
/// This is the statement-level trigger entry point.
/// Deduplication is automatic (`HashSet`).
/// Raises ERROR if `max_queue_size` would be exceeded.
pub fn enqueue_refresh_bulk(entity: &str, keys: Vec<KeyValue>) {
    if let Err(e) = check_queue_backpressure(crate::config::max_queue_size()) {
        e.raise();
    }
    for key in &keys {
        state::queue_insert(RefreshKey::new(entity, key.clone()));
    }
    // Plain (bulk) enqueue poisons any direct patches for these keys.
    for key in keys {
        super::patch::poison(RefreshKey::new(entity, key));
    }
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
    fn test_enqueue_and_drain() {
        state::clear();

        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap();
        enqueue_refresh_with_limit("post", KeyValue::Int(2), usize::MAX).unwrap();
        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap(); // duplicate

        assert_eq!(state::drain().queue.len(), 2); // Deduplicated
        assert_eq!(state::drain().queue.len(), 0); // drained
    }

    #[test]
    fn test_clear_queue() {
        state::clear();

        enqueue_refresh_with_limit("user", KeyValue::Int(1), usize::MAX).unwrap();
        enqueue_refresh_with_limit("post", KeyValue::Int(2), usize::MAX).unwrap();

        state::clear();

        assert_eq!(state::drain().queue.len(), 0);
    }

    #[test]
    fn test_multi_entity_queue() {
        state::clear();

        crate::queue::enqueue_refresh("user", KeyValue::Int(1));
        crate::queue::enqueue_refresh("post", KeyValue::Int(10));
        crate::queue::enqueue_refresh("user", KeyValue::Int(1)); // duplicate
        crate::queue::enqueue_refresh("post", KeyValue::Int(20));
        crate::queue::enqueue_refresh("user", KeyValue::Int(2));

        let queue = state::drain().queue;
        assert_eq!(queue.len(), 4);
        assert!(queue.contains(&RefreshKey::pk("user", 1)));
        assert!(queue.contains(&RefreshKey::pk("post", 10)));
    }

    #[test]
    fn test_enqueue_respects_max_queue_size() {
        state::clear();

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
        assert_eq!(state::drain().queue.len(), 2);
    }
}
