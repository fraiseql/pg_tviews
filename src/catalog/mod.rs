pub(crate) mod indexes;
pub mod plan;
pub mod reads;
pub mod registered;
pub mod resolve;

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
    /// The definition reads the current time: two computations of a row may
    /// differ with no write between them.
    pub time_dependent: bool,

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
    /// Read `pg_tview_meta.identity`, which every registration writes.
    ///
    /// # Errors
    /// It is NULL, names no column, or has an unknown kind.
    pub fn from_catalog(
        entity: &str,
        json: Option<&serde_json::Value>,
    ) -> crate::TViewResult<Self> {
        let unreadable = |why: &str| crate::TViewError::CatalogError {
            operation: format!("Read the row identity of tv_{entity}"),
            pg_error: why.to_string(),
        };
        let json = json.ok_or_else(|| unreadable("it is NULL"))?;
        let column = json["columns"][0]["name"]
            .as_str()
            .ok_or_else(|| unreadable("it names no column"))?
            .to_string();
        let kind = match json["kind"].as_str() {
            Some("pk") => crate::lineage::IdentityKind::Pk,
            Some("distinct_on") => crate::lineage::IdentityKind::DistinctOn,
            other => return Err(unreadable(&format!("unknown kind {other:?}"))),
        };
        Ok(Self { column, kind })
    }

    /// No identity: what a catalog row awaiting re-registration has.
    const fn unknown() -> Self {
        Self {
            column: String::new(),
            kind: crate::lineage::IdentityKind::Pk,
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
/// out of sync as catalog columns are added.
pub(crate) fn meta_select() -> String {
    format!(
        "SELECT table_oid::oid AS tview_oid, view_oid::oid AS view_oid, entity, plan, \
         uncascaded_policy, identity, \
         uncascaded_table_oids::oid[] AS uncascaded_table_oids, uncascaded_table_policies, \
         function_read_functions, function_read_tables::oid[] AS function_read_tables, \
         time_refresh IS NOT DISTINCT FROM 'external' AS time_refresh_external, \
         time_dependent \
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

    /// The TVIEWs whose catalog row does not decode, each with why.
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn unreadable() -> crate::TViewResult<Vec<(String, String)>> {
        Spi::connect(|client| -> crate::TViewResult<Vec<(String, String)>> {
            let rows = client.select(&format!("{} ORDER BY entity", meta_select()), None, &[])?;
            let mut unreadable = Vec::new();
            for row in rows {
                if let Err(e) = Self::from_spi_row(&row) {
                    let entity = row["entity"].value::<String>()?.unwrap_or_default();
                    unreadable.push((entity, e.to_string()));
                }
            }
            Ok(unreadable)
        })
    }

    /// The entity of the TVIEW whose table is `tview_oid`, read without decoding
    /// its plan: what dropping a TVIEW needs.
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn entity_of_table(tview_oid: Oid) -> crate::TViewResult<Option<String>> {
        crate::utils::spi::one::<String>(
            &format!(
                "SELECT entity FROM {} WHERE table_oid::pg_catalog.oid = $1",
                crate::utils::meta_table()
            ),
            &[crate::utils::spi::oid(tview_oid)],
        )
    }

    /// The catalog row of `entity_name` for re-registration, which derives its
    /// plan and identity again: either one that does not decode reads as empty, so
    /// re-registering is the remedy for it. Never cached.
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn load_to_rederive(entity_name: &str) -> crate::TViewResult<Option<Self>> {
        Spi::connect(|client| -> crate::TViewResult<Option<Self>> {
            let args = vec![crate::utils::spi::text(entity_name)];
            let mut rows =
                client.select(&format!("{} WHERE entity = $1", meta_select()), None, &args)?;
            rows.next()
                .map(|row| Self::decode(&row, Derived::Rederive))
                .transpose()
        })
    }

    /// Parse a row of [`meta_select`] into a `TviewMeta`.
    pub fn from_spi_row(row: &spi::SpiHeapTupleData) -> crate::TViewResult<Self> {
        Self::decode(row, Derived::Strict)
    }

    fn decode(row: &spi::SpiHeapTupleData, derived: Derived) -> crate::TViewResult<Self> {
        let entity_name: String =
            row["entity"]
                .value()?
                .ok_or_else(|| crate::TViewError::SpiError {
                    query: String::new(),
                    error: "entity column is NULL".to_string(),
                })?;
        let policy = |name: &str| {
            crate::config::UncascadedPolicy::parse(name).ok_or_else(|| {
                crate::TViewError::CatalogError {
                    operation: format!("Read the uncascaded policies of tv_{entity_name}"),
                    pg_error: format!("unknown policy {name:?}"),
                }
            })
        };
        let uncascaded_policy = policy(
            &row["uncascaded_policy"]
                .value::<String>()?
                .unwrap_or_default(),
        )?;

        let table_policies = row["uncascaded_table_oids"]
            .value::<Vec<Oid>>()?
            .unwrap_or_default()
            .into_iter()
            .zip(
                row["uncascaded_table_policies"]
                    .value::<Vec<String>>()?
                    .unwrap_or_default(),
            )
            .map(|(oid, name)| Ok((oid, policy(&name)?)))
            .collect::<crate::TViewResult<_>>()?;

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

        let identity = RowIdentity::from_catalog(
            &entity_name,
            row["identity"]
                .value::<pgrx::JsonB>()?
                .map(|j| j.0)
                .as_ref(),
        );
        let identity = match (identity, derived) {
            (Ok(identity), _) => identity,
            (Err(_), Derived::Rederive) => RowIdentity::unknown(),
            (Err(e), Derived::Strict) => return Err(e),
        };
        let plan = plan::TviewPlan::decode(
            &entity_name,
            row["plan"]
                .value::<pgrx::JsonB>()?
                .map_or(serde_json::Value::Null, |j| j.0),
        );
        let plan = match (plan, derived) {
            (Ok(plan), _) => plan,
            (Err(_), Derived::Rederive) => plan::TviewPlan::default(),
            (Err(e), Derived::Strict) => return Err(e),
        };

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
            time_dependent: row["time_dependent"].value::<bool>()?.unwrap_or(false),
            identity,
        })
    }
}

/// How [`TviewMeta::decode`] treats what registration derived (the plan, the
/// identity) when it does not decode.
#[derive(Clone, Copy)]
enum Derived {
    /// An error naming the TVIEW.
    Strict,
    /// Empty: the caller derives it again.
    Rederive,
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
            time_dependent: false,
            identity: RowIdentity::unknown(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_identity_reads_the_catalog() {
        use crate::lineage::IdentityKind;
        let doc =
            serde_json::json!({"kind": "distinct_on", "columns": [{"name": "id", "type": "uuid"}]});
        let id = RowIdentity::from_catalog("doc", Some(&doc)).unwrap();
        assert_eq!(id.column, "id");
        assert_eq!(id.kind, IdentityKind::DistinctOn);
        assert!(!id.is_pk("doc"));

        let pk = serde_json::json!({"kind": "pk", "columns": [{"name": "pk_doc"}]});
        assert!(
            RowIdentity::from_catalog("doc", Some(&pk))
                .unwrap()
                .is_pk("doc")
        );
    }

    #[test]
    fn an_unreadable_row_identity_is_a_catalog_error() {
        let unknown = serde_json::json!({"kind": "natural", "columns": [{"name": "id"}]});
        for json in [
            None,
            Some(&serde_json::json!({"kind": "pk"})),
            Some(&unknown),
        ] {
            let err = RowIdentity::from_catalog("doc", json).unwrap_err();
            assert!(err.to_string().contains("tv_doc"), "{err}");
        }
    }
}
