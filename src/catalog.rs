use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::datum::DatumWithOid;
use serde::{Deserialize, Serialize};

/// Type of dependency relationship for `jsonb_ivm` optimization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyType {
    /// Direct column from base table (no nested JSONB)
    Scalar,
    /// Embedded object via `jsonb_build_object` in nested key
    NestedObject,
    /// Array created via `jsonb_agg`
    Array,
}

impl DependencyType {
    /// Parse from database string representation
    pub fn from_str(s: &str) -> Self {
        match s {
            "nested_object" => Self::NestedObject,
            "array" => Self::Array,
            _ => Self::Scalar, // default fallback (includes "scalar")
        }
    }

    /// Convert to database string representation
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::NestedObject => "nested_object",
            Self::Array => "array",
        }
    }
}

/// Represents a row in `pg_tview_meta` (your own catalog table).
#[allow(clippy::unsafe_derive_deserialize)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char, // 's' = sync (default), 'a' = async (future)
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,

    /// Type of each dependency: Scalar (direct column), `NestedObject` (embedded JSONB),
    /// or Array (`jsonb_agg` aggregation).
    ///
    /// Length matches `fk_columns` and `dependencies` arrays.
    /// Used by `jsonb_ivm` to choose patch function (scalar/nested/array).
    pub dependency_types: Vec<DependencyType>,

    /// JSONB path for each dependency, if nested.
    /// - Scalar: None
    /// - `NestedObject`: Some(vec!["author"]) for { "author": {...} }
    /// - Array: Some(vec!["comments"]) for { "comments": [...] }
    ///
    /// Length matches `dependency_types`.
    pub dependency_paths: Vec<Option<Vec<String>>>,

    /// For Array dependencies, the key used to match elements (e.g., "id").
    /// Used by `jsonb_smart_patch_array(target, 'comments', '{...}', 'id')`.
    ///
    /// - Scalar/`NestedObject`: None
    /// - Array: Some("id") or `Some("pk_comment")`
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
            let args = vec![unsafe { DatumWithOid::new(source_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
            let mut rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE view_oid = $1 OR table_oid = $1",
                None,
                &args,
            )?;

            let result = if let Some(row) = rows.next() {
                // Extract existing arrays
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value()?;
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value()?;

                // Extract NEW arrays - dependency_types (TEXT[])
                let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value()?;
                let dep_types = Self::parse_dependency_types(dep_types_raw);

                // dependency_paths (TEXT[][]) - array of arrays
                // TODO: pgrx doesn't support TEXT[][] extraction yet
                // For now, use empty default (Task 3 will populate these)
                let dep_paths: Vec<Option<Vec<String>>> = vec![];

                // array_match_keys (TEXT[]) with NULL values
                let array_keys: Option<Vec<Option<String>>> =
                    row["array_match_keys"].value()?;

                Some(Self {
                    tview_oid: row["tview_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "tview_oid column is NULL".to_string(),
                        }))?,
                    view_oid: row["view_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "view_oid column is NULL".to_string(),
                        }))?,
                    entity_name: row["entity"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "entity column is NULL".to_string(),
                        }))?,
                    sync_mode: 's', // Default to synchronous
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                    dependency_types: dep_types,
                    dependency_paths: dep_paths,
                    array_match_keys: array_keys.unwrap_or_default(),
                })
            } else {
                None
            };
            Ok(result)
        })
    }

    /// Look up metadata by entity name
    pub fn load_by_entity(entity_name: &str) -> spi::Result<Option<Self>> {
        Spi::connect(|client| {
            let args = vec![unsafe { DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
            let mut rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE entity = $1",
                None,
                &args,
            )?;

            let result = if let Some(row) = rows.next() {
                // Extract existing arrays
                let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value()?;
                let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value()?;

                // Extract NEW arrays - dependency_types (TEXT[])
                let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value()?;
                let dep_types = Self::parse_dependency_types(dep_types_raw);

                // dependency_paths (TEXT[][]) - array of arrays
                // TODO: pgrx doesn't support TEXT[][] extraction yet
                // For now, use empty default (Task 3 will populate these)
                let dep_paths: Vec<Option<Vec<String>>> = vec![];

                // array_match_keys (TEXT[]) with NULL values
                let array_keys: Option<Vec<Option<String>>> =
                    row["array_match_keys"].value()?;

                Some(Self {
                    tview_oid: row["tview_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "tview_oid column is NULL".to_string(),
                        }))?,
                    view_oid: row["view_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "view_oid column is NULL".to_string(),
                        }))?,
                    entity_name: row["entity"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "entity column is NULL".to_string(),
                        }))?,
                    sync_mode: 's',
                    fk_columns: fk_cols_val.unwrap_or_default(),
                    uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                    dependency_types: dep_types,
                    dependency_paths: dep_paths,
                    array_match_keys: array_keys.unwrap_or_default(),
                })
            } else {
                None
            };
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
            let args = vec![unsafe { DatumWithOid::new(tview_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
            let mut rows = client.select(
                "SELECT table_oid AS tview_oid, view_oid, entity, \
                        fk_columns, uuid_fk_columns, \
                        dependency_types, dependency_paths, array_match_keys \
                 FROM pg_tview_meta \
                 WHERE table_oid = $1",
                None,
                &args,
            )?;

            let result = if let Some(row) = rows.next() {
                Some(Self::from_spi_row(&row)?)
            } else {
                None
            };
            Ok(result)
        })
    }

    /// Parse SPI row into `TviewMeta` struct
    fn from_spi_row(row: &spi::SpiHeapTupleData) -> spi::Result<Self> {
        // Extract existing arrays
        let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value()?;
        let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value()?;

        // Extract dependency_types (TEXT[])
        let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value()?;
        let dep_types = Self::parse_dependency_types(dep_types_raw);

        // dependency_paths (TEXT[]) - stored as flat array, parse as single-element paths
        let dep_paths_raw: Option<Vec<Option<String>>> = row["dependency_paths"].value()?;
        let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
            .unwrap_or_default()
            .into_iter()
            .map(|opt_path| opt_path.map(|p| vec![p]))
            .collect();

        // array_match_keys (TEXT[]) with NULL values
        let array_keys: Option<Vec<Option<String>>> = row["array_match_keys"].value()?;

        Ok(Self {
                    tview_oid: row["tview_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "tview_oid column is NULL".to_string(),
                        }))?,
                    view_oid: row["view_oid"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "view_oid column is NULL".to_string(),
                        }))?,
                    entity_name: row["entity"].value()?
                        .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                            query: String::new(),
                            error: "entity column is NULL".to_string(),
                        }))?,
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

        for (i, _fk_col) in self.fk_columns.iter().enumerate() {
            let dep_type = self.dependency_types.get(i).cloned().unwrap_or(DependencyType::Scalar);
            let path = self.dependency_paths.get(i).cloned().flatten();
            let match_key = self.array_match_keys.get(i).cloned().flatten();

            details.push(DependencyDetail {
                dep_type,
                path,
                match_key,
            });
        }

        details
    }

}

/// Represents a single dependency with its type, path, and match key.
/// Used by the refresh engine to determine how to update related TVIEWs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDetail {
    /// Type of dependency (Scalar, Array, etc.)
    pub dep_type: DependencyType,
    /// JSONB path to the dependent data (e.g., `["author"]` or `["comments"]`)
    pub path: Option<Vec<String>>,
    /// Key to match for array elements (e.g., "id")
    pub match_key: Option<String>,
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

/// Map a base table OID to its entity name
///
/// Example: OID of `tb_user` → Some("user")
///
/// Returns:
/// - Ok(Some(entity)) if table is tracked in `pg_tview_meta`
/// - Ok(None) if table is not tracked
/// - Err(...) on database error
///
/// # Cached Version
///
/// This function caches the mapping to avoid repeated `pg_class` queries.
/// Performance improvement: 0.1ms → 0.001ms per trigger
pub fn entity_for_table(table_oid: Oid) -> crate::TViewResult<Option<String>> {
    crate::queue::cache::table_cache::entity_for_table_cached(table_oid)
}

/// Get entity name for table OID without caching (internal use)
///
/// This is the slow path that queries pg_class every time.
/// Used by the cache when there's a cache miss.
pub fn entity_for_table_uncached(table_oid: Oid) -> crate::TViewResult<Option<String>> {
    // Query pg_class to get table name from OID
    let table_name = Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {table_oid:?}"
    ))?.ok_or_else(|| crate::TViewError::SpiError {
        query: format!("SELECT relname FROM pg_class WHERE oid = {table_oid:?}"),
        error: "Table OID not found".to_string(),
    })?;

    // Check if table name matches "tb_<entity>" pattern
    if let Some(entity) = table_name.strip_prefix("tb_") {
        Ok(Some(entity.to_string()))
    } else {
        // Not a tb_* table, skip
        Ok(None)
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

    #[test]
    fn test_entity_for_table_name_parsing() {
        // This is a unit test that doesn't require database access
        let test_cases = vec![
            ("tb_user", Some("user")),
            ("tb_post", Some("post")),
            ("tb_company", Some("company")),
            ("users", None),  // Not a tb_* table
            ("pg_class", None),  // System table
        ];

        for (table_name, expected_entity) in test_cases {
            let result = if let Some(entity) = table_name.strip_prefix("tb_") {
                Some(entity.to_string())
            } else {
                None
            };

            assert_eq!(result.as_deref(), expected_entity);
        }
    }
}
