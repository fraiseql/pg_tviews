pub mod plan;
pub mod reads;
pub mod registered;

use pgrx::pg_sys::Oid;
use pgrx::prelude::*;

/// Represents a row in `pg_tview_meta` (your own catalog table).
#[derive(Debug, Clone)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,

    /// What registration derived from the backing view's query tree (ADR 0203).
    pub plan: plan::TviewPlan,

    /// What a write to a base table no cascade maps to its keys does: the policy
    /// stored when the TVIEW was created.
    pub uncascaded_policy: crate::config::UncascadedPolicy,

    /// The tables declared with a policy of their own, each overriding
    /// `uncascaded_policy` for writes to it.
    pub table_policies: Vec<(Oid, crate::config::UncascadedPolicy)>,

    /// The functions the definition calls, declared with the tables each reads:
    /// `schema.name(argument types)`.
    pub function_reads: Vec<(String, Vec<Oid>)>,

    /// A TVIEW that reads the current time declared `time_refresh: external`.
    pub time_refresh_external: bool,

    /// The column that names the TVIEW's rows (ADR 0169).
    pub identity: RowIdentity,
}

/// The column that names a TVIEW's rows (ADR 0169), as the catalog records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowIdentity {
    pub column: String,
    pub kind: crate::lineage::IdentityKind,
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
    pub fn from_catalog(entity: &str, json: Option<&serde_json::Value>) -> Self {
        let column = json
            .and_then(|j| j["columns"][0]["name"].as_str())
            .map_or_else(|| format!("pk_{entity}"), str::to_string);
        let kind = match json.and_then(|j| j["kind"].as_str()) {
            Some("distinct_on") => crate::lineage::IdentityKind::DistinctOn,
            _ => crate::lineage::IdentityKind::Pk,
        };
        Self { column, kind }
    }

    /// Whether the identity is `pk_<entity>`, the column parents join on.
    #[must_use]
    pub fn is_pk(&self, entity: &str) -> bool {
        self.column == format!("pk_{entity}")
    }
}

/// Shared SELECT column list + FROM used by every `TviewMeta` loader. Callers
/// append their own `WHERE` / `ORDER BY`. One copy keeps the loaders from drifting
/// out of sync as catalog columns are added.
pub(crate) fn meta_select() -> String {
    format!(
        "SELECT table_oid::oid AS tview_oid, view_oid::oid AS view_oid, entity, plan, \
         uncascaded_policy, identity, \
         uncascaded_table_oids::oid[] AS uncascaded_table_oids, uncascaded_table_policies, \
         function_read_functions, function_read_tables::oid[] AS function_read_tables, \
         time_refresh IS NOT DISTINCT FROM 'external' AS time_refresh_external \
         FROM {}",
        crate::utils::meta_table()
    )
}

/// The cached catalog row of `entity`, unless `pg_tviews.table_cache_enabled` is
/// off.
fn cached(entity: &str) -> Option<TviewMeta> {
    if !crate::config::table_cache_enabled() {
        return None;
    }
    let meta = crate::cache::METAS.with(|m| m.get(&entity.to_string()));
    if meta.is_some() {
        crate::metrics::metrics_api::record_table_cache_hit();
    } else {
        crate::metrics::metrics_api::record_table_cache_miss();
    }
    meta
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
    /// declared policy, else the TVIEW's.
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
        self.plan
            .tables
            .iter()
            .find(|m| m.relid == table_oid.to_u32() || root.is_some_and(|r| m.relid == r.to_u32()))
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

    /// The catalog row of the TVIEW whose table is `tview_oid`.
    ///
    /// # Errors
    /// The catalog cannot be read, or its plan does not decode.
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

    /// Parse a row of [`meta_select`] into a `TviewMeta`.
    pub fn from_spi_row(row: &spi::SpiHeapTupleData) -> crate::TViewResult<Self> {
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
        );
        let plan = plan::TviewPlan::decode(
            &entity_name,
            row["plan"]
                .value::<pgrx::JsonB>()?
                .map_or(serde_json::Value::Null, |j| j.0),
        )?;

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

            plan,
            uncascaded_policy,
            table_policies,
            function_reads,
            time_refresh_external: row["time_refresh_external"]
                .value::<bool>()?
                .unwrap_or(false),
            identity,
        })
    }
}

impl Default for TviewMeta {
    fn default() -> Self {
        Self {
            tview_oid: pg_sys::Oid::INVALID,
            view_oid: pg_sys::Oid::INVALID,
            entity_name: String::new(),

            plan: plan::TviewPlan::default(),
            uncascaded_policy: crate::config::UncascadedPolicy::Warn,
            table_policies: Vec::new(),
            function_reads: Vec::new(),
            time_refresh_external: false,
            identity: RowIdentity::from_catalog("", None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_identity_reads_the_catalog_and_defaults_to_pk() {
        use crate::lineage::IdentityKind;
        let doc =
            serde_json::json!({"kind": "distinct_on", "columns": [{"name": "id", "type": "uuid"}]});
        let id = RowIdentity::from_catalog("doc", Some(&doc));
        assert_eq!(id.column, "id");
        assert_eq!(id.kind, IdentityKind::DistinctOn);
        assert!(!id.is_pk("doc"));

        let absent = RowIdentity::from_catalog("doc", None);
        assert_eq!(absent.column, "pk_doc");
        assert_eq!(absent.kind, IdentityKind::Pk);
        assert!(absent.is_pk("doc"));
    }
}
