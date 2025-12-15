//! Queue persistence for 2PC support
//!
//! This module handles serialization and deserialization of refresh queues
//! for prepared transactions. Supports both JSONB and binary formats.

use pgrx::prelude::*;
use pgrx::pg_sys;
use pgrx::JsonB;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use crate::queue::RefreshKey;
use crate::TViewResult;

/// Serialized queue format for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedQueue {
    /// Schema version for forward compatibility
    pub version: u32,
    /// The refresh keys to process
    pub keys: Vec<RefreshKey>,
    /// Metadata about when and how the queue was created
    pub metadata: QueueMetadata,
}

/// Metadata for queue serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueMetadata {
    /// ISO8601 timestamp when queue was enqueued
    pub enqueued_at: String,
    /// Session user who created the queue
    pub source_session: String,
    /// Savepoint depth when queue was created
    pub savepoint_depth: usize,
}

impl SerializedQueue {
    /// Create a serialized queue from a `HashSet` of refresh keys
    pub fn from_queue(queue: HashSet<RefreshKey>) -> Self {
        Self {
            version: 1,
            keys: queue.into_iter().collect(),
            metadata: QueueMetadata {
                enqueued_at: chrono::Utc::now().to_rfc3339(),
                source_session: get_session_id(),
                savepoint_depth: get_savepoint_depth(),
            },
        }
    }

    /// Convert back to a `HashSet` of refresh keys
    pub fn into_queue(self) -> HashSet<RefreshKey> {
        self.keys.into_iter().collect()
    }

    /// Serialize to JSONB format (human-readable, easier debugging)
    pub fn into_jsonb(self) -> TViewResult<JsonB> {
        let json = serde_json::to_value(self)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to serialize queue to JSON: {}", e),
            })?;
        Ok(JsonB(json))
    }

    /// Deserialize from JSONB format
    pub fn from_jsonb(jsonb: JsonB) -> TViewResult<Self> {
        serde_json::from_value(jsonb.0)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to deserialize queue from JSON: {}", e),
            })
    }

    /// Serialize to binary format (compact, faster for large queues)
    #[allow(dead_code)]
    pub fn to_binary(&self) -> TViewResult<Vec<u8>> {
        bincode::serialize(self)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to serialize queue to binary: {}", e),
            })
    }

    /// Deserialize from binary format
    #[allow(dead_code)]
    pub fn from_binary(data: &[u8]) -> TViewResult<Self> {
        bincode::deserialize(data)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to deserialize binary queue: {}", e),
            })
    }

    /// Serialize to compressed JSONB format (balance of readability and size)
    #[allow(dead_code)]
    pub fn to_compressed_jsonb(&self) -> TViewResult<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let json = serde_json::to_vec(self)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to serialize queue to JSON: {}", e),
            })?;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&json)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to compress queue: {}", e),
            })?;

        encoder.finish()
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to finish compression: {}", e),
            })
    }

    /// Deserialize from compressed JSONB format
    #[allow(dead_code)]
    pub fn from_compressed_jsonb(data: &[u8]) -> TViewResult<Self> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(data);
        let mut json_bytes = Vec::new();
        decoder.read_to_end(&mut json_bytes)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Decompression failed: {}", e),
            })?;

        serde_json::from_slice(&json_bytes)
            .map_err(|e| crate::TViewError::SerializationError {
                message: format!("Failed to deserialize JSON: {}", e),
            })
    }
}

/// Get current session ID for metadata
fn get_session_id() -> String {
    // Try to get session ID from PostgreSQL
    match Spi::get_one::<String>("SELECT session_user") {
        Ok(Some(user)) => user,
        Ok(None) => "unknown".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

/// Get current savepoint depth using PostgreSQL's transaction nesting level
///
/// PostgreSQL's `GetCurrentTransactionNestLevel()` returns:
/// - 1 for top-level transaction (no savepoints)
/// - 2 for first savepoint level
/// - 3 for nested savepoint, etc.
///
/// We convert this to savepoint depth:
/// - 0 = no savepoints (nest level 1)
/// - 1 = one savepoint active (nest level 2)
/// - etc.
fn get_savepoint_depth() -> usize {
    // Safety: GetCurrentTransactionNestLevel is safe to call from any transaction context
    let nest_level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };

    // Convert nest level to savepoint depth
    // nest_level is always >= 1 when in a transaction
    if nest_level > 1 {
        (nest_level - 1) as usize
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_queue_serialization_jsonb() {
        let mut queue = HashSet::new();
        queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });
        queue.insert(RefreshKey { entity: "post".to_string(), pk: 2 });

        let serialized = SerializedQueue::from_queue(queue);
        let jsonb = serialized.into_jsonb().unwrap();
        let deserialized = SerializedQueue::from_jsonb(jsonb).unwrap();
        let restored_queue = deserialized.into_queue();

        assert_eq!(restored_queue.len(), 2);
        assert!(restored_queue.contains(&RefreshKey { entity: "user".to_string(), pk: 1 }));
        assert!(restored_queue.contains(&RefreshKey { entity: "post".to_string(), pk: 2 }));
    }

    #[test]
    fn test_queue_serialization_binary() {
        let mut queue = HashSet::new();
        queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });
        queue.insert(RefreshKey { entity: "post".to_string(), pk: 2 });

        let serialized = SerializedQueue::from_queue(queue);
        let binary = serialized.to_binary().unwrap();
        let deserialized = SerializedQueue::from_binary(&binary).unwrap();
        let restored_queue = deserialized.into_queue();

        assert_eq!(restored_queue.len(), 2);
        assert!(restored_queue.contains(&RefreshKey { entity: "user".to_string(), pk: 1 }));
        assert!(restored_queue.contains(&RefreshKey { entity: "post".to_string(), pk: 2 }));
    }

    #[test]
    fn test_queue_serialization_compressed() {
        let mut queue = HashSet::new();
        queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });
        queue.insert(RefreshKey { entity: "post".to_string(), pk: 2 });

        let serialized = SerializedQueue::from_queue(queue);
        let compressed = serialized.to_compressed_jsonb().unwrap();
        let deserialized = SerializedQueue::from_compressed_jsonb(&compressed).unwrap();
        let restored_queue = deserialized.into_queue();

        assert_eq!(restored_queue.len(), 2);
        assert!(restored_queue.contains(&RefreshKey { entity: "user".to_string(), pk: 1 }));
        assert!(restored_queue.contains(&RefreshKey { entity: "post".to_string(), pk: 2 }));
    }

    #[test]
    fn test_savepoint_depth_conversion_logic() {
        // Test the conversion logic from nest level to savepoint depth
        // This function tests the mathematical conversion without calling FFI

        fn convert_nest_level_to_savepoint_depth(nest_level: i32) -> usize {
            if nest_level > 1 {
                (nest_level - 1) as usize
            } else {
                0
            }
        }

        // Test cases based on PostgreSQL's transaction nesting levels
        assert_eq!(convert_nest_level_to_savepoint_depth(1), 0, "Top-level transaction should have savepoint depth 0");
        assert_eq!(convert_nest_level_to_savepoint_depth(2), 1, "First savepoint should have savepoint depth 1");
        assert_eq!(convert_nest_level_to_savepoint_depth(3), 2, "Second savepoint should have savepoint depth 2");
        assert_eq!(convert_nest_level_to_savepoint_depth(4), 3, "Third savepoint should have savepoint depth 3");
        assert_eq!(convert_nest_level_to_savepoint_depth(10), 9, "Tenth savepoint should have savepoint depth 9");
    }

    #[test]
    fn test_serialized_queue_includes_savepoint_depth() {
        // Test that SerializedQueue captures savepoint depth when created
        let mut queue = HashSet::new();
        queue.insert(RefreshKey { entity: "user".to_string(), pk: 1 });

        let serialized = SerializedQueue::from_queue(queue);

        // The savepoint_depth should be captured (though we can't control its exact value
        // in a unit test environment without PostgreSQL context)
        // We can at least verify it's a valid usize value
        let _savepoint_depth = serialized.metadata.savepoint_depth;
        // If we get here without panicking, the function call succeeded
    }
}