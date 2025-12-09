use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::IntoDatum;
use serde::{Deserialize, Serialize};

/// Type of dependency relationship for jsonb_ivm optimization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyType {
    /// Direct column from base table (no nested JSONB)
    Scalar,
    /// Embedded object via jsonb_build_object in nested key
    NestedObject,
    /// Array created via jsonb_agg
    Array,
}

impl DependencyType {
    /// Parse from database string representation
    pub fn from_str(s: &str) -> Self {
        match s {
            "scalar" => DependencyType::Scalar,
            "nested_object" => DependencyType::NestedObject,
            "array" => DependencyType::Array,
            _ => DependencyType::Scalar, // default fallback
        }
    }

    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            DependencyType::Scalar => "scalar",
            DependencyType::NestedObject => "nested_object",
            DependencyType::Array => "array",
        }
    }
}

/// Represents a row in pg_tview_meta (your own catalog table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char, // 's' = sync (default), 'a' = async (future)
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,

    /// Type of each dependency: Scalar (direct column), NestedObject (embedded JSONB),
    /// or Array (jsonb_agg aggregation).
    ///
    /// Length matches `fk_columns` and `dependencies` arrays.
    /// Used by jsonb_ivm to choose patch function (scalar/nested/array).
    pub dependency_types: Vec<DependencyType>,

    /// JSONB path for each dependency, if nested.
    /// - Scalar: None
    /// - NestedObject: Some(vec!["author"]) for { "author": {...} }
    /// - Array: Some(vec!["comments"]) for { "comments": [...] }
    ///
    /// Length matches `dependency_types`.
    pub dependency_paths: Vec<Option<Vec<String>>>,

    /// For Array dependencies, the key used to match elements (e.g., "id").
    /// Used by `jsonb_smart_patch_array(target, 'comments', '{...}', 'id')`.
    ///
    /// - Scalar/NestedObject: None
    /// - Array: Some("id") or Some("pk_comment")
    ///
    /// Length matches `dependency_types`.
    pub array_match_keys: Vec<Option<String>>,
}

impl TviewMeta {
    /// Helper: Parse TEXT[] to Vec<DependencyType>
    fn parse_dependency_types(row_value: Option<Vec<String>>) -> Vec<DependencyType> {
        row_value
            .unwrap_or_default()
            .into_iter()
            .map(|s| DependencyType::from_str(&s))
            .collect()
    }

    /// Look up metadata by source table OID or view OID.
    pub fn load_for_source(source_oid: Oid) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE view_oid = $1 OR table_oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), source_oid.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                // Extract existing arrays
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

                // Extract NEW arrays - dependency_types (TEXT[])
                let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
                let dep_types = Self::parse_dependency_types(dep_types_raw);

                // dependency_paths (TEXT[][]) - array of arrays
                // TODO: pgrx doesn't support TEXT[][] extraction yet
                // For now, use empty default (Task 3 will populate these)
                let dep_paths: Vec<Option<Vec<String>>> = vec![];

                // array_match_keys (TEXT[]) with NULL values
                let array_keys: Option<Vec<Option<String>>> =
                    row["array_match_keys"].value().unwrap_or(None);

                result = Some(Self {
                    tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                    view_oid: row["view_oid"].value().unwrap().unwrap(),
                    entity_name: row["entity"].value().unwrap().unwrap(),
                    sync_mode: 's', // Default to synchronous
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                    dependency_types: dep_types,
                    dependency_paths: dep_paths,
                    array_match_keys: array_keys.unwrap_or_default(),
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
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE entity = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), entity_name.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                // Extract existing arrays
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

                // Extract NEW arrays - dependency_types (TEXT[])
                let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
                let dep_types = Self::parse_dependency_types(dep_types_raw);

                // dependency_paths (TEXT[][]) - array of arrays
                // TODO: pgrx doesn't support TEXT[][] extraction yet
                // For now, use empty default (Task 3 will populate these)
                let dep_paths: Vec<Option<Vec<String>>> = vec![];

                // array_match_keys (TEXT[]) with NULL values
                let array_keys: Option<Vec<Option<String>>> =
                    row["array_match_keys"].value().unwrap_or(None);

                result = Some(Self {
                    tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                    view_oid: row["view_oid"].value().unwrap().unwrap(),
                    entity_name: row["entity"].value().unwrap().unwrap(),
                    sync_mode: 's',
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                    dependency_types: dep_types,
                    dependency_paths: dep_paths,
                    array_match_keys: array_keys.unwrap_or_default(),
                });
                break; // Only get first row
            }
            Ok(result)
        })
    }

    /// Load metadata for a specific TVIEW OID.
    ///
    /// Queries `pg_tview_meta` to retrieve dependency information needed for
    /// smart JSONB patching. Used by `apply_patch()` to determine how to update
    /// the JSONB `data` column.
    ///
    /// # Arguments
    ///
    /// * `tview_oid` - OID of the TVIEW table (e.g., `tv_post`)
    ///
    /// # Returns
    ///
    /// - `Ok(Some(TviewMeta))` if metadata found
    /// - `Ok(None)` if no metadata exists (legacy TVIEW)
    /// - `Err` if query fails
    ///
    /// # Example
    ///
    /// ```rust
    /// let meta = TviewMeta::load_for_tview(tview_oid)?;
    /// if let Some(m) = meta {
    ///     let deps = m.parse_dependencies();
    ///     // Use deps for smart patching
    /// }
    /// ```
    pub fn load_for_tview(tview_oid: Oid) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE table_oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), tview_oid.into_datum())]),
            )?;

            let mut result = None;
            for row in rows {
                result = Some(Self::from_spi_row(&row)?);
                break; // Only get first row
            }
            Ok(result)
        })
    }

    /// Parse SPI row into TviewMeta struct
    fn from_spi_row(row: &spi::SpiHeapTupleData) -> spi::Result<TviewMeta> {
        // Extract existing arrays
        let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
        let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

        // Extract dependency_types (TEXT[])
        let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
        let dep_types = Self::parse_dependency_types(dep_types_raw);

        // dependency_paths (TEXT[]) - stored as flat array, parse as single-element paths
        let dep_paths_raw: Option<Vec<Option<String>>> = row["dependency_paths"].value().unwrap_or(None);
        let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
            .unwrap_or_default()
            .into_iter()
            .map(|opt_path| opt_path.map(|p| vec![p]))
            .collect();

        // array_match_keys (TEXT[]) with NULL values
        let array_keys: Option<Vec<Option<String>>> = row["array_match_keys"].value().unwrap_or(None);

        Ok(TviewMeta {
            tview_oid: row["tview_oid"].value().unwrap().unwrap(),
            view_oid: row["view_oid"].value().unwrap().unwrap(),
            entity_name: row["entity"].value().unwrap().unwrap(),
            sync_mode: 's', // Default to synchronous
            fk_columns: fk_cols_val.unwrap_or_default(),
            uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
            dependency_types: dep_types,
            dependency_paths: dep_paths,
            array_match_keys: array_keys.unwrap_or_default(),
        })
    }

    /// Parse dependency metadata into structured form for smart patching.
    ///
    /// Converts raw metadata arrays (`dependency_types`, `dependency_paths`, etc.)
    /// into a vector of `DependencyDetail` structs, one per FK column. Each detail
    /// contains the dependency type, JSONB path, and array match key if applicable.
    ///
    /// # Returns
    ///
    /// Vector of `DependencyDetail` structs, one per FK column in `fk_columns`.
    ///
    /// # Example
    ///
    /// ```rust
    /// let deps = meta.parse_dependencies();
    /// for dep in deps {
    ///     match dep.dep_type {
    ///         DependencyType::NestedObject => {
    ///             println!("Nested at path: {:?}", dep.path);
    ///         }
    ///         DependencyType::Array => {
    ///             println!("Array at path: {:?}, key: {:?}", dep.path, dep.match_key);
    ///         }
    ///         DependencyType::Scalar => {
    ///             println!("Scalar FK: {}", dep.fk_column);
    ///         }
    ///     }
    /// }
    /// ```
    pub fn parse_dependencies(&self) -> Vec<DependencyDetail> {
        let mut details = Vec::new();

        for (i, fk_col) in self.fk_columns.iter().enumerate() {
            let dep_type = self.dependency_types.get(i).cloned().unwrap_or(DependencyType::Scalar);
            let path = self.dependency_paths.get(i).cloned().flatten();
            let match_key = self.array_match_keys.get(i).cloned().flatten();

            details.push(DependencyDetail {
                fk_column: fk_col.clone(),
                dep_type,
                path,
                match_key,
            });
        }

        details
    }

    /// Get dependency info for a specific FK column
    pub fn get_dependency(&self, fk_column: &str) -> Option<DependencyDetail> {
        self.parse_dependencies()
            .into_iter()
            .find(|d| d.fk_column == fk_column)
    }

    /// TODO: function to register a new TVIEW (used by CREATE TVIEW)
    pub fn register_new(_view_oid: Oid, _tview_oid: Oid, _entity_name: &str) -> spi::Result<()> {
        // Implementation: insert into pg_tview_meta
        // This will be invoked from a CREATE TVIEW support function.
        Ok(())
    }
}

/// Represents a single dependency with its type, path, and match key.
///
/// This struct packages all information needed to apply smart JSONB patching
/// for one FK relationship. Created by `TviewMeta::parse_dependencies()`.
///
/// # Fields
///
/// * `fk_column` - Foreign key column name (e.g., `"fk_user"`)
/// * `dep_type` - Type of dependency (`Scalar`, `NestedObject`, or `Array`)
/// * `path` - JSONB path where dependency data lives (e.g., `vec!["author"]`)
/// * `match_key` - For arrays, the key to match elements (e.g., `"id"`)
///
/// # Example
///
/// ```rust
/// // For nested author object:
/// DependencyDetail {
///     fk_column: "fk_user".to_string(),
///     dep_type: DependencyType::NestedObject,
///     path: Some(vec!["author".to_string()]),
///     match_key: None,
/// }
///
/// // For comments array:
/// DependencyDetail {
///     fk_column: "fk_comment".to_string(),
///     dep_type: DependencyType::Array,
///     path: Some(vec!["comments".to_string()]),
///     match_key: Some("id".to_string()),
/// }
/// ```
#[derive(Debug, Clone)]
pub struct DependencyDetail {
    pub fk_column: String,
    pub dep_type: DependencyType,
    pub path: Option<Vec<String>>,    // e.g., Some(vec!["author"]) or Some(vec!["comments"])
    pub match_key: Option<String>,     // e.g., Some("id") for arrays
}

impl Default for TviewMeta {
    fn default() -> Self {
        Self {
            tview_oid: pg_sys::Oid::INVALID,
            view_oid: pg_sys::Oid::INVALID,
            entity_name: String::new(),
            sync_mode: 's',
            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![],
            dependency_paths: vec![],
            array_match_keys: vec![],
        }
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
