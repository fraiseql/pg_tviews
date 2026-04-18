//! Cascade path data structures for tracking dependency propagation.

use pgrx::pg_sys::Oid;
use serde::{Deserialize, Serialize};

/// A single hop in a cascade path: represents one step in the dependency chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadeHop {
    /// OID of the source table/entity that triggers the cascade
    pub source_oid: Oid,
    /// Name of the target entity (TVIEW) that gets updated
    pub target_entity: String,
}

/// A complete cascade path: sequence of hops from source table to final TVIEW.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadePath {
    /// The sequence of hops in this cascade path
    pub hops: Vec<CascadeHop>,
}

impl CascadePath {
    /// Get the source OID of this cascade path (first hop's source)
    #[allow(dead_code)] // Reason: Will be used in future cascade implementation phases
    pub fn source_oid(&self) -> Option<Oid> {
        self.hops.first().map(|hop| hop.source_oid)
    }

    /// Get the final target entity of this cascade path (last hop's target)
    #[allow(dead_code)] // Reason: Will be used in future cascade implementation phases
    pub fn target_entity(&self) -> Option<&str> {
        self.hops.last().map(|hop| hop.target_entity.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_cascade_path_serialization_round_trip() {
        let hop1 = CascadeHop {
            source_oid: Oid::from(12345),
            target_entity: "post".to_string(),
        };
        let hop2 = CascadeHop {
            source_oid: Oid::from(67890),
            target_entity: "comment".to_string(),
        };
        let path = CascadePath {
            hops: vec![hop1.clone(), hop2.clone()],
        };

        // Serialize to JSON
        let json = serde_json::to_string(&path).expect("Failed to serialize CascadePath");
        println!("Serialized JSON: {}", json);

        // Deserialize back
        let deserialized: CascadePath = serde_json::from_str(&json).expect("Failed to deserialize CascadePath");

        // Verify round-trip
        assert_eq!(path, deserialized);
        assert_eq!(path.source_oid(), Some(Oid::from(12345)));
        assert_eq!(path.target_entity(), Some("comment"));
    }

    #[test]
    fn test_single_hop_cascade_path() {
        let hop = CascadeHop {
            source_oid: Oid::from(11111),
            target_entity: "user".to_string(),
        };
        let path = CascadePath {
            hops: vec![hop],
        };

        let json = serde_json::to_string(&path).unwrap();
        let deserialized: CascadePath = serde_json::from_str(&json).unwrap();

        assert_eq!(path, deserialized);
        assert_eq!(path.source_oid(), Some(Oid::from(11111)));
        assert_eq!(path.target_entity(), Some("user"));
    }
}