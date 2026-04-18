//! Cascade path data structures for multi-hop dependency propagation.
//!
//! A `CascadePath` describes how to follow a chain of table joins from a
//! source base table back to a TVIEW's entity PK. At trigger time, the
//! path is traversed hop-by-hop via SPI queries to discover which parent
//! entity rows need refreshing.

use pgrx::pg_sys::Oid;
use serde::{Deserialize, Serialize};

/// One intermediate SPI query in a cascade path.
///
/// At trigger time, each hop becomes:
/// ```sql
/// SELECT {carry_col} FROM {table_name} WHERE {lookup_col} = ANY($1)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadeHop {
    /// OID of the intermediate table to query
    pub table_oid: Oid,
    /// Name of the intermediate table (for SQL generation)
    pub table_name: String,
    /// Column to match against incoming IDs (WHERE clause)
    pub lookup_col: String,
    /// Column to extract for the next hop (SELECT clause)
    pub carry_col: String,
}

/// A complete path from a source base table to a TVIEW entity's PK.
///
/// Single-hop example (tb_comment → tv_post):
/// ```json
/// {
///   "source_oid": 12345,
///   "source_table": "tb_comment",
///   "entity_name": "post",
///   "initial_col": "fk_post",
///   "hops": []
/// }
/// ```
/// The trigger reads `fk_post` from the changed `tb_comment` row and
/// enqueues refresh of entity "post" for that PK directly.
///
/// Multi-hop example (tb_item → tb_group → tv_order):
/// ```json
/// {
///   "source_oid": 12345,
///   "source_table": "tb_item",
///   "entity_name": "order",
///   "initial_col": "fk_group",
///   "hops": [{
///     "table_oid": 67890,
///     "table_name": "tb_group",
///     "lookup_col": "pk_group",
///     "carry_col": "fk_order"
///   }]
/// }
/// ```
/// The trigger reads `fk_group` from the changed row, then queries
/// `SELECT fk_order FROM tb_group WHERE pk_group = ANY($1)`, and
/// enqueues refresh of "order" for each resulting PK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CascadePath {
    /// OID of the source table where the trigger fires
    pub source_oid: Oid,
    /// Name of the source table
    pub source_table: String,
    /// Target TVIEW entity name (e.g. "order")
    pub entity_name: String,
    /// Column to read from the changed row to start the chain
    pub initial_col: String,
    /// Intermediate hops (empty for single-hop / direct FK)
    pub hops: Vec<CascadeHop>,
    /// If true, path could not be statically resolved — fall back to full refresh
    #[serde(default)]
    pub unresolvable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_hop_round_trip() {
        let path = CascadePath {
            source_oid: Oid::from(12345),
            source_table: "tb_comment".to_string(),
            entity_name: "post".to_string(),
            initial_col: "fk_post".to_string(),
            hops: vec![],
            unresolvable: false,
        };

        let json = serde_json::to_string(&path).unwrap();
        let deserialized: CascadePath = serde_json::from_str(&json).unwrap();
        assert_eq!(path, deserialized);
    }

    #[test]
    fn test_multi_hop_round_trip() {
        let path = CascadePath {
            source_oid: Oid::from(11111),
            source_table: "tb_item".to_string(),
            entity_name: "order".to_string(),
            initial_col: "fk_group".to_string(),
            hops: vec![CascadeHop {
                table_oid: Oid::from(22222),
                table_name: "tb_group".to_string(),
                lookup_col: "pk_group".to_string(),
                carry_col: "fk_order".to_string(),
            }],
            unresolvable: false,
        };

        let json = serde_json::to_string(&path).unwrap();
        let deserialized: CascadePath = serde_json::from_str(&json).unwrap();
        assert_eq!(path, deserialized);
        assert_eq!(path.hops.len(), 1);
    }

    #[test]
    fn test_unresolvable_defaults_to_false() {
        let json =
            r#"{"source_oid":1,"source_table":"t","entity_name":"e","initial_col":"c","hops":[]}"#;
        let path: CascadePath = serde_json::from_str(json).unwrap();
        assert!(!path.unresolvable);
    }
}
