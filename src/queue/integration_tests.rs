#[cfg(test)]
mod tests {
    use crate::queue::key::KeyValue;
    use crate::queue::ops::{clear_queue, take_queue_snapshot};
    use crate::queue::{RefreshKey, enqueue_refresh};

    #[test]
    fn test_multi_entity_queue() {
        clear_queue();

        // Simulate multiple trigger firings
        enqueue_refresh("user", KeyValue::Int(1));
        enqueue_refresh("post", KeyValue::Int(10));
        enqueue_refresh("user", KeyValue::Int(1)); // duplicate
        enqueue_refresh("post", KeyValue::Int(20));
        enqueue_refresh("user", KeyValue::Int(2));

        let snapshot = take_queue_snapshot();

        // Should have 4 unique keys: (user,1), (post,10), (post,20), (user,2)
        assert_eq!(snapshot.len(), 4);

        // Verify specific keys exist
        assert!(snapshot.contains(&RefreshKey::pk("user", 1)));
        assert!(snapshot.contains(&RefreshKey::pk("post", 10)));
    }
}
