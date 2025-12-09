use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::IntoDatum;
use serde::{Deserialize, Serialize};

/// Represents a row in pg_tview_meta (your own catalog table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char, // 's' = sync (default), 'a' = async (future)
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,
}

impl TviewMeta {
    /// Look up metadata by source table OID or view OID.
    pub fn load_for_source(source_oid: Oid) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, fk_columns, uuid_fk_columns \
                 FROM pg_tview_meta \
                 WHERE view_oid = $1 OR table_oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), source_oid.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                // Extract FK columns from array
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

                result = Some(Self {
                    tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                    view_oid: row["view_oid"].value().unwrap().unwrap(),
                    entity_name: row["entity"].value().unwrap().unwrap(),
                    sync_mode: 's', // Default to synchronous
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                });
                break; // Only get first row
            }
            Ok(result)
        })
    }

    /// Look up metadata by entity name
    pub fn load_by_entity(entity_name: &str) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, fk_columns, uuid_fk_columns \
                 FROM pg_tview_meta \
                 WHERE entity = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), entity_name.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                // Extract FK columns from array
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

                result = Some(Self {
                    tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                    view_oid: row["view_oid"].value().unwrap().unwrap(),
                    entity_name: row["entity"].value().unwrap().unwrap(),
                    sync_mode: 's', // Default to synchronous
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                });
                break; // Only get first row
            }
            Ok(result)
        })
    }

    /// TODO: function to register a new TVIEW (used by CREATE TVIEW)
    pub fn register_new(_view_oid: Oid, _tview_oid: Oid, _entity_name: &str) -> spi::Result<()> {
        // Implementation: insert into pg_tview_meta
        // This will be invoked from a CREATE TVIEW support function.
        Ok(())
    }
}

// Phase 5 Task 2 RED: Tests for metadata enhancement
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_type_from_str() {
        // Test will fail: DependencyType doesn't exist yet
        assert_eq!(DependencyType::from_str("scalar"), DependencyType::Scalar);
        assert_eq!(DependencyType::from_str("nested_object"), DependencyType::NestedObject);
        assert_eq!(DependencyType::from_str("array"), DependencyType::Array);
        assert_eq!(DependencyType::from_str("unknown"), DependencyType::Scalar); // default
    }

    #[test]
    fn test_dependency_type_to_str() {
        // Test will fail: DependencyType doesn't exist yet
        assert_eq!(DependencyType::Scalar.as_str(), "scalar");
        assert_eq!(DependencyType::NestedObject.as_str(), "nested_object");
        assert_eq!(DependencyType::Array.as_str(), "array");
    }

    #[test]
    fn test_tview_meta_has_new_fields() {
        // Test will fail: TviewMeta doesn't have these fields yet
        let meta = TviewMeta {
            tview_oid: Oid::from(1234),
            view_oid: Oid::from(5678),
            entity_name: "test".to_string(),
            sync_mode: 's',
            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![DependencyType::Scalar],
            dependency_paths: vec![None],
            array_match_keys: vec![None],
        };

        assert_eq!(meta.dependency_types.len(), 1);
        assert_eq!(meta.dependency_paths.len(), 1);
        assert_eq!(meta.array_match_keys.len(), 1);
    }
}
