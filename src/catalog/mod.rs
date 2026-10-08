pub mod reads;
pub mod registered;

use crate::cascade_path::CascadePath;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
/// Type of dependency relationship for `jsonb_delta` optimization
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,

    /// Type of each dependency: Scalar (direct column), `NestedObject` (embedded JSONB),
    /// or Array (`jsonb_agg` aggregation).
    ///
    /// Length matches `fk_columns` and `dependencies` arrays.
    /// Used by `jsonb_delta` to choose patch function (scalar/nested/array).
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

    /// Direct-patch column map (issue #56): base-table columns that map
    /// identity-style to top-level keys of this entity's own `data` object.
    ///
    /// Aligned with [`Self::direct_map_keys`]: `direct_map_columns[i]` is a base
    /// column name (e.g. `bio`) and `direct_map_keys[i]` the JSONB key it feeds
    /// (e.g. `bio`). Populated at CREATE time from bare `jsonb_build_object` pairs;
    /// empty ⇒ the direct-patch fast path never engages for this entity.
    pub direct_map_columns: Vec<String>,

    /// JSONB keys aligned with [`Self::direct_map_columns`]. See that field.
    pub direct_map_keys: Vec<String>,

    /// `true` when this TVIEW's backing view is a `UNION ALL` or `UNION` query.
    ///
    /// Used to apply the duplicate-row policy when multiple rows are returned
    /// for the same PK during refresh (which can occur with non-mutually-exclusive
    /// UNION ALL branches).
    pub is_union: bool,

    /// Cascade paths defining how changes propagate to this TVIEW.
    ///
    /// Each path represents a sequence of hops from a source table to this TVIEW,
    /// enabling indirect dependency tracking for multi-level cascades.
    pub cascade_paths: Vec<CascadePath>,

    /// What a write to a base table no cascade maps to its keys does (issues
    /// #157, #158): the policy stored when the TVIEW was created.
    pub uncascaded_policy: crate::config::UncascadedPolicy,

    /// The tables declared with a policy of their own (#195), each overriding
    /// `uncascaded_policy` for writes to it.
    pub table_policies: Vec<(Oid, crate::config::UncascadedPolicy)>,

    /// The functions the definition calls, declared with the tables each reads
    /// (#193): `schema.name(argument types)`.
    pub function_reads: Vec<(String, Vec<Oid>)>,

    /// A TVIEW that reads the current time declared `time_refresh: external`
    /// (#193).
    pub time_refresh_external: bool,

    /// How a write to each base table maps to keys (ADR 0157); empty for a TVIEW
    /// registered by a release without lineage, until it is re-registered.
    pub key_mappings: Vec<crate::lineage::KeyMapping>,

    /// The column that names the TVIEW's rows (ADR 0169).
    pub identity: RowIdentity,
}

/// The column that names a TVIEW's rows (ADR 0169), as the catalog records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowIdentity {
    pub column: String,
    pub kind: crate::lineage::IdentityKind,
    /// Registered before identities were recorded (NULL in the catalog): the
    /// root table's key is read by name until it is re-registered.
    pub legacy: bool,
    /// Registered before identities were recorded, as a DISTINCT ON TVIEW: its
    /// rows are refreshed in full until it is re-registered.
    pub legacy_distinct_on: bool,
}

/// How the values of an identity column are carried and bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyType {
    /// `int2`, `int4` or `int8`: bound as `int8`.
    Int,
    /// Any other type, schema-qualified and quoted: bound as text, cast to it.
    Text(String),
}

impl RowIdentity {
    /// Read `pg_tview_meta.identity` (NULL for a row registered before it:
    /// `pk_<entity>`).
    #[must_use]
    pub fn from_catalog(
        entity: &str,
        json: Option<&serde_json::Value>,
        legacy_distinct_on: bool,
    ) -> Self {
        let column = json
            .and_then(|j| j["columns"][0]["name"].as_str())
            .map_or_else(|| format!("pk_{entity}"), str::to_string);
        let kind = match json.and_then(|j| j["kind"].as_str()) {
            Some("distinct_on") => crate::lineage::IdentityKind::DistinctOn,
            _ => crate::lineage::IdentityKind::Pk,
        };
        Self {
            column,
            kind,
            legacy: json.is_none(),
            legacy_distinct_on: json.is_none() && legacy_distinct_on,
        }
    }

    /// Whether the identity is `pk_<entity>`, the column parents join on.
    #[must_use]
    pub fn is_pk(&self, entity: &str) -> bool {
        self.column == format!("pk_{entity}")
    }
}

/// Shared SELECT column list + FROM used by every `TviewMeta` loader. Callers
/// append their own `WHERE` / `ORDER BY`. One copy keeps the loaders from drifting
/// out of sync as catalog columns are added (e.g. issue #56's direct-patch map).
pub(crate) fn meta_select() -> String {
    format!(
        "SELECT table_oid::oid AS tview_oid, view_oid::oid AS view_oid, entity, \
         fk_columns, uuid_fk_columns, \
         dependency_types, dependency_paths, array_match_keys, \
         direct_map_columns, direct_map_keys, is_union, cascade_paths, \
         uncascaded_policy, key_mappings, identity, \
         uncascaded_table_oids::oid[] AS uncascaded_table_oids, uncascaded_table_policies, \
         function_read_functions, function_read_tables::oid[] AS function_read_tables, \
         time_refresh IS NOT DISTINCT FROM 'external' AS time_refresh_external, \
         distinct_on_keys <> '{{}}' AS legacy_distinct_on \
         FROM {}",
        crate::utils::meta_table()
    )
}

fn cached(entity: &str) -> Option<TviewMeta> {
    crate::cache::METAS.with(|m| m.get(&entity.to_string()))
}

/// Count the catalog query just made and cache its result.
fn remember(loaded: Option<TviewMeta>) -> Option<TviewMeta> {
    crate::metrics::metrics_api::record_catalog_lookup();
    if let Some(meta) = &loaded {
        crate::cache::watch(&[meta.tview_oid, meta.view_oid]);
        crate::cache::METAS.with(|m| m.insert(meta.entity_name.clone(), meta.clone()));
    }
    loaded
}

impl TviewMeta {
    /// How the values of the identity column are bound: the type of the column in
    /// the TVIEW's table (cached per backend).
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read.
    pub fn key_type(&self) -> crate::TViewResult<KeyType> {
        if let Some(key_type) = crate::cache::KEY_TYPES.with(|m| m.get(&self.tview_oid)) {
            return Ok(key_type);
        }
        let name = Spi::get_one_with_args::<String>(
            "SELECT CASE WHEN a.atttypid IN ('pg_catalog.int2'::pg_catalog.regtype, \
                                             'pg_catalog.int4'::pg_catalog.regtype, \
                                             'pg_catalog.int8'::pg_catalog.regtype) THEN NULL \
                    ELSE pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(t.typname) END \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
             JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
             WHERE a.attrelid = $1 AND a.attname = $2 AND NOT a.attisdropped",
            &[
                crate::utils::spi::oid(self.tview_oid),
                crate::utils::spi::text(self.identity.column.as_str()),
            ],
        )
        .or_else(|e| match e {
            spi::Error::InvalidPosition => Ok(None),
            e => Err(e),
        })?;
        let key_type = name.map_or(KeyType::Int, KeyType::Text);
        crate::cache::KEY_TYPES.with(|m| m.insert(self.tview_oid, key_type.clone()));
        Ok(key_type)
    }

    /// What a write to `table_oid`, a table no cascade reaches, does: its own
    /// declared policy, else the TVIEW's (#195).
    #[must_use]
    pub fn policy_for(&self, table_oid: Oid) -> crate::config::UncascadedPolicy {
        self.table_policies
            .iter()
            .find(|(oid, _)| *oid == table_oid)
            .map_or(self.uncascaded_policy, |(_, policy)| *policy)
    }

    /// The mapping of base table `table_oid`, also found by name (a row restored
    /// before its relids were rebound) or through the partitioned table `root`.
    #[must_use]
    pub fn key_mapping(
        &self,
        table_oid: Oid,
        root: Option<Oid>,
    ) -> Option<&crate::lineage::KeyMapping> {
        self.key_mappings
            .iter()
            .find(|m| m.relid == table_oid.to_u32() || root.is_some_and(|r| m.relid == r.to_u32()))
    }

    /// Helper: Parse TEXT[] to Vec<DependencyType>
    fn parse_dependency_types(row_value: Option<Vec<String>>) -> Vec<DependencyType> {
        row_value
            .unwrap_or_default()
            .into_iter()
            .map(|s| DependencyType::from_str(&s))
            .collect()
    }

    /// Convert flat `TEXT[]` of dot-separated path strings into structured paths.
    ///
    /// Each element is a dot-joined key sequence (e.g. `"book.author"`).
    /// An empty string represents a `None` path (Scalar dependency).
    fn parse_dep_paths(raw: Option<Vec<Option<String>>>) -> Vec<Option<Vec<String>>> {
        raw.unwrap_or_default()
            .into_iter()
            .map(|opt| {
                opt.filter(|s| !s.is_empty())
                    .map(|s| s.split('.').map(str::to_string).collect())
            })
            .collect()
    }

    /// Look up metadata by entity name (cached per backend).
    pub fn load_by_entity(entity_name: &str) -> crate::TViewResult<Option<Self>> {
        if let Some(meta) = cached(entity_name) {
            return Ok(Some(meta));
        }
        let loaded = Spi::connect(|client| -> crate::TViewResult<Option<Self>> {
            let args = vec![crate::utils::spi::text(entity_name)];
            let mut rows =
                client.select(&format!("{} WHERE entity = $1", meta_select()), None, &args)?;

            match rows.next() {
                Some(row) => Ok(Some(Self::from_spi_row(&row)?)),
                None => Ok(None),
            }
        })?;
        Ok(remember(loaded))
    }

    /// Load all TVIEW metadata
    pub fn load_all() -> crate::TViewResult<Vec<Self>> {
        Spi::connect(|client| -> crate::TViewResult<Vec<Self>> {
            let rows = client.select(&format!("{} ORDER BY entity", meta_select()), None, &[])?;

            let mut result = Vec::new();
            for row in rows {
                result.push(Self::from_spi_row(&row)?);
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
    pub fn load_for_tview(tview_oid: Oid) -> crate::TViewResult<Option<Self>> {
        Spi::connect(|client| -> crate::TViewResult<Option<Self>> {
            let args = vec![crate::utils::spi::oid(tview_oid)];
            let mut rows = client.select(
                &format!("{} WHERE table_oid = $1", meta_select()),
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

    /// Parse SPI row into `TviewMeta` struct.
    ///
    /// Expects columns: `tview_oid`, `view_oid`, `entity`, `fk_columns`,
    /// `uuid_fk_columns`, `dependency_types`, `dependency_paths`, `array_match_keys`.
    pub fn from_spi_row(row: &spi::SpiHeapTupleData) -> crate::TViewResult<Self> {
        // Extract existing arrays
        let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value()?;
        let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value()?;

        // Extract dependency_types (TEXT[])
        let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value()?;
        let dep_types = Self::parse_dependency_types(dep_types_raw);

        let dep_paths_raw: Option<Vec<Option<String>>> = row["dependency_paths"].value()?;
        let dep_paths = Self::parse_dep_paths(dep_paths_raw);

        // array_match_keys (TEXT[]) with NULL values
        let array_keys: Option<Vec<Option<String>>> = row["array_match_keys"].value()?;

        // direct_map_columns / direct_map_keys (TEXT[]) — aligned column→key map
        // for the issue #56 direct-patch fast path. Empty for pre-#56 tviews.
        let direct_map_columns: Vec<String> = row["direct_map_columns"]
            .value::<Vec<String>>()?
            .unwrap_or_default();
        let direct_map_keys: Vec<String> = row["direct_map_keys"]
            .value::<Vec<String>>()?
            .unwrap_or_default();

        // is_union (BOOLEAN) — true when backing view is a UNION ALL / UNION query
        let is_union: bool = row["is_union"].value::<bool>()?.unwrap_or(false);

        // cascade_paths (TEXT[]) — array of JSON-serialized cascade path objects
        let cascade_paths_raw: Option<Vec<String>> = row["cascade_paths"].value()?;
        let cascade_paths = if let Some(json_strings) = cascade_paths_raw {
            json_strings
                .into_iter()
                .map(|json| serde_json::from_str(&json))
                .collect::<Result<Vec<CascadePath>, _>>()?
        } else {
            Vec::new()
        };

        let uncascaded_policy = crate::config::UncascadedPolicy::from_stored(
            &row["uncascaded_policy"]
                .value::<String>()?
                .unwrap_or_default(),
        );

        let table_policies = row["uncascaded_table_oids"]
            .value::<Vec<Oid>>()?
            .unwrap_or_default()
            .into_iter()
            .zip(
                row["uncascaded_table_policies"]
                    .value::<Vec<String>>()?
                    .unwrap_or_default(),
            )
            .map(|(oid, policy)| (oid, crate::config::UncascadedPolicy::from_stored(&policy)))
            .collect();

        let mut function_reads: Vec<(String, Vec<Oid>)> = Vec::new();
        for (function, table) in row["function_read_functions"]
            .value::<Vec<String>>()?
            .unwrap_or_default()
            .into_iter()
            .zip(
                row["function_read_tables"]
                    .value::<Vec<Option<Oid>>>()?
                    .unwrap_or_default(),
            )
        {
            if function_reads.last().is_none_or(|(f, _)| *f != function) {
                function_reads.push((function, Vec::new()));
            }
            if let (Some(table), Some((_, tables))) = (table, function_reads.last_mut()) {
                tables.push(table);
            }
        }

        let key_mappings = row["key_mappings"]
            .value::<pgrx::JsonB>()?
            .map(|j| crate::lineage::KeyMapping::parse_all(&j.0))
            .unwrap_or_default();

        let entity_name: String =
            row["entity"]
                .value()?
                .ok_or_else(|| crate::TViewError::SpiError {
                    query: String::new(),
                    error: "entity column is NULL".to_string(),
                })?;
        let identity = RowIdentity::from_catalog(
            &entity_name,
            row["identity"]
                .value::<pgrx::JsonB>()?
                .map(|j| j.0)
                .as_ref(),
            row["legacy_distinct_on"].value::<bool>()?.unwrap_or(false),
        );

        Ok(Self {
            tview_oid: row["tview_oid"]
                .value()?
                .ok_or_else(|| crate::TViewError::SpiError {
                    query: String::new(),
                    error: "tview_oid column is NULL".to_string(),
                })?,
            view_oid: row["view_oid"]
                .value()?
                .ok_or_else(|| crate::TViewError::SpiError {
                    query: String::new(),
                    error: "view_oid column is NULL".to_string(),
                })?,
            entity_name,

            fk_columns: fk_cols_val.unwrap_or_default(),
            uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
            dependency_types: dep_types,
            dependency_paths: dep_paths,
            array_match_keys: array_keys.unwrap_or_default(),
            direct_map_columns,
            direct_map_keys,
            is_union,
            cascade_paths,
            uncascaded_policy,
            table_policies,
            function_reads,
            time_refresh_external: row["time_refresh_external"]
                .value::<bool>()?
                .unwrap_or(false),
            key_mappings,
            identity,
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
        let len = self.dependency_types.len().max(self.fk_columns.len());
        let mut details = Vec::with_capacity(len);

        for i in 0..len {
            let dep_type = self
                .dependency_types
                .get(i)
                .cloned()
                .unwrap_or(DependencyType::Scalar);
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

            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![],
            dependency_paths: vec![],
            array_match_keys: vec![],
            direct_map_columns: vec![],
            direct_map_keys: vec![],
            is_union: false,
            cascade_paths: vec![],
            uncascaded_policy: crate::config::UncascadedPolicy::Warn,
            table_policies: Vec::new(),
            function_reads: Vec::new(),
            time_refresh_external: false,
            key_mappings: vec![],
            identity: RowIdentity::from_catalog("", None, false),
        }
    }
}

/// Get entity name for table OID without caching (internal use)
///
/// This is the slow path that queries `pg_class` every time.
/// Used by the cache when there's a cache miss.
pub fn entity_for_table_uncached(table_oid: Oid) -> crate::TViewResult<Option<String>> {
    // Use Spi::connect + client.select instead of Spi::get_one_with_args because
    // pgrx 0.17's get_one_with_args returns Err(InvalidPosition) when the query
    // returns 0 rows — it calls .first().get_one() which goes through
    // get_datum_by_ordinal's bounds check (current >= size) instead of the
    // get_heap_tuple path that properly returns Ok(None) for empty results.
    Spi::connect(|client| {
        // Step 1: resolve OID → table name
        let args = vec![crate::utils::spi::oid(table_oid)];
        let mut rows = client.select(
            "SELECT relname::text FROM pg_class WHERE oid = $1",
            Some(1),
            &args,
        )?;
        let table_name: String = match rows.next() {
            Some(row) => match row[1].value::<String>()? {
                Some(name) => name,
                None => return Ok(None),
            },
            None => return Ok(None),
        };

        // Step 2: check for "tb_<entity>" prefix
        let Some(entity) = table_name.strip_prefix("tb_") else {
            return Ok(None);
        };

        // Step 3: verify entity exists in pg_tview_meta
        let args = vec![crate::utils::spi::text(entity)];
        let mut meta_rows = client.select(
            &format!(
                "SELECT entity FROM {} WHERE entity = $1",
                crate::utils::meta_table()
            ),
            Some(1),
            &args,
        )?;
        match meta_rows.next() {
            Some(row) => Ok(row[1].value::<String>()?),
            None => Ok(None),
        }
    })
    .map_err(|e: spi::Error| crate::TViewError::SpiError {
        query: "entity_for_table_uncached".to_string(),
        error: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_type_from_str() {
        assert_eq!(DependencyType::from_str("scalar"), DependencyType::Scalar);
        assert_eq!(
            DependencyType::from_str("nested_object"),
            DependencyType::NestedObject
        );
        assert_eq!(DependencyType::from_str("array"), DependencyType::Array);
        assert_eq!(DependencyType::from_str("unknown"), DependencyType::Scalar);
        // default
    }

    #[test]
    fn test_dependency_type_to_str() {
        assert_eq!(DependencyType::Scalar.as_str(), "scalar");
        assert_eq!(DependencyType::NestedObject.as_str(), "nested_object");
        assert_eq!(DependencyType::Array.as_str(), "array");
    }

    #[test]
    fn test_tview_meta_has_new_fields() {
        let meta = TviewMeta {
            tview_oid: Oid::from(1234),
            view_oid: Oid::from(5678),
            entity_name: "test".to_string(),

            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![DependencyType::Scalar],
            dependency_paths: vec![None],
            array_match_keys: vec![None],
            direct_map_columns: vec!["bio".to_string(), "name".to_string()],
            direct_map_keys: vec!["bio".to_string(), "display_name".to_string()],
            is_union: false,
            cascade_paths: vec![],
            ..TviewMeta::default()
        };

        assert_eq!(meta.dependency_types.len(), 1);
        assert_eq!(meta.dependency_paths.len(), 1);
        assert_eq!(meta.array_match_keys.len(), 1);
    }

    #[test]
    fn row_identity_reads_the_catalog_and_defaults_to_pk() {
        use crate::lineage::IdentityKind;
        let doc =
            serde_json::json!({"kind": "distinct_on", "columns": [{"name": "id", "type": "uuid"}]});
        let id = RowIdentity::from_catalog("doc", Some(&doc), true);
        assert_eq!(id.column, "id");
        assert_eq!(id.kind, IdentityKind::DistinctOn);
        assert!(!id.legacy_distinct_on && !id.is_pk("doc"));

        let old = RowIdentity::from_catalog("doc", None, false);
        assert_eq!(old.column, "pk_doc");
        assert_eq!(old.kind, IdentityKind::Pk);
        assert!(old.is_pk("doc") && old.legacy && !old.legacy_distinct_on);
        assert!(!id.legacy);

        assert!(RowIdentity::from_catalog("doc", None, true).legacy_distinct_on);
    }

    #[test]
    fn test_entity_for_table_name_parsing() {
        // This is a unit test that doesn't require database access
        let test_cases = vec![
            ("tb_user", Some("user")),
            ("tb_post", Some("post")),
            ("tb_company", Some("company")),
            ("users", None),    // Not a tb_* table
            ("pg_class", None), // System table
        ];

        for (table_name, expected_entity) in test_cases {
            let result = table_name.strip_prefix("tb_").map(str::to_string);

            assert_eq!(result.as_deref(), expected_entity);
        }
    }
}
