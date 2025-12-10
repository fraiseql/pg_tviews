#[cfg(test)]
mod tests {
    use crate::queue::{enqueue_refresh, take_queue_snapshot, clear_queue, RefreshKey};

    #[test]
    fn test_multi_entity_queue() {
        clear_queue();

        // Simulate multiple trigger firings
        enqueue_refresh("user", 1).unwrap();
        enqueue_refresh("post", 10).unwrap();
        enqueue_refresh("user", 1).unwrap(); // duplicate
        enqueue_refresh("post", 20).unwrap();
        enqueue_refresh("user", 2).unwrap();

        let snapshot = take_queue_snapshot();

        // Should have 4 unique keys: (user,1), (post,10), (post,20), (user,2)
        assert_eq!(snapshot.len(), 4);

        // Verify specific keys exist
        assert!(snapshot.contains(&RefreshKey { entity: "user".to_string(), pk: 1 }));
        assert!(snapshot.contains(&RefreshKey { entity: "post".to_string(), pk: 10 }));
    }
}