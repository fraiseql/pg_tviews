pub mod collections;
pub mod graph;
pub(crate) mod indexes;
mod partition;
pub mod plan;
pub mod queries;
pub mod reads;
pub mod registered;
pub mod resolve;
pub mod row;

pub use collections::Collection;
pub use graph::EntityDepGraph;
pub use partition::partition_root;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::rc::Rc;

/// The catalog table, schema-qualified: every read or write of it names it so.
#[must_use]
pub fn meta_table() -> String {
    format!("{}.pg_tview_meta", crate::utils::ext_schema())
}

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
    /// How the values of that column are bound.
    pub key_type: KeyType,
    /// The columns of its backing view, in order: what its upserts write.
    pub view_columns: Vec<String>,

    /// The GraphQL type name of its rows ([`typename`]), shared by every change
    /// the journal records.
    pub typename: std::rc::Rc<str>,
    /// How its table's `id` column, the rows' public id that the journal carries,
    /// is written in JSON; `None` without such a column.
    pub public_id: Option<PublicId>,
    /// Its references to other TVIEWs, as the change feed names the collections
    /// its rows belong to: read only while the feed names them.
    pub collections: Vec<Rc<Collection>>,
    /// Whether `collections` was read.
    collections_read: bool,
}

/// A TVIEW's entity and plan: all a write reads of the TVIEWs it does not reach
/// (the cascade paths starting at the written table, the dependency graph).
#[derive(Debug, Clone, Default)]
pub struct PlanEntry {
    /// The TVIEW's entity.
    pub entity: String,
    /// Its propagation plan.
    pub plan: plan::TviewPlan,
}

/// How a TVIEW's public `id` is written in JSON (as `to_jsonb` writes it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicId {
    /// An integer or numeric: its text is the JSON number.
    Number,
    /// Anything else (a uuid): a JSON string.
    String,
}

/// A row's public id as the write that changed it returned it
/// ([`TviewMeta::id_text`]): kept as text, written in JSON only where it is
/// reported or published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowId {
    pub text: String,
    pub kind: PublicId,
}

impl RowId {
    /// The id as JSON text, as `to_jsonb` writes it.
    #[must_use]
    pub fn json(&self) -> String {
        match self.kind {
            PublicId::Number => self.text.clone(),
            PublicId::String => serde_json::Value::String(self.text.clone()).to_string(),
        }
    }

    /// The id as a JSON value.
    #[must_use]
    pub fn value(&self) -> serde_json::Value {
        match self.kind {
            PublicId::Number => serde_json::from_str(&self.text)
                .unwrap_or_else(|_| serde_json::Value::String(self.text.clone())),
            PublicId::String => serde_json::Value::String(self.text.clone()),
        }
    }
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
///
/// The collections are read only when `with_collections`: planning their subquery
/// costs ten times the rest. Inside the extension's own script (an upgrade
/// re-registering TVIEWs between two scripts) the catalog can be an earlier
/// release's, without the view they are read from: they are left out, and nothing
/// loaded then is cached ([`remember`]).
pub(crate) fn meta_select(with_collections: bool) -> String {
    let collections = if with_collections && !crate::revision::in_own_script() {
        collections::collections_sql()
    } else {
        "NULL::pg_catalog.jsonb AS collections".to_string()
    };
    format!(
        "SELECT table_oid::oid AS tview_oid, view_oid::oid AS view_oid, entity, plan, \
         uncascaded_policy, identity, \
         uncascaded_table_oids::oid[] AS uncascaded_table_oids, uncascaded_table_policies, \
         function_read_functions, function_read_tables::oid[] AS function_read_tables, \
         time_refresh IS NOT DISTINCT FROM 'external' AS time_refresh_external, \
         time_dependent, graphql_typename, \
         (SELECT a.atttypid IN ('pg_catalog.int2'::pg_catalog.regtype, \
                                'pg_catalog.int4'::pg_catalog.regtype, \
                                'pg_catalog.int8'::pg_catalog.regtype, \
                                'pg_catalog.numeric'::pg_catalog.regtype) \
          FROM pg_catalog.pg_attribute a WHERE a.attrelid = table_oid \
          AND a.attname = 'id' AND a.attnum > 0 AND NOT a.attisdropped) AS public_id_number, \
         (SELECT CASE WHEN a.atttypid IN ('pg_catalog.int2'::pg_catalog.regtype, \
                                          'pg_catalog.int4'::pg_catalog.regtype, \
                                          'pg_catalog.int8'::pg_catalog.regtype) THEN NULL \
                 ELSE pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(t.typname) END \
          FROM pg_catalog.pg_attribute a \
          JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
          JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
          WHERE a.attrelid = table_oid AND a.attname = identity -> 'columns' -> 0 ->> 'name' \
          AND NOT a.attisdropped) AS key_type, \
         ARRAY(SELECT a.attname::pg_catalog.text FROM pg_catalog.pg_attribute a \
               WHERE a.attrelid = view_oid AND a.attnum > 0 AND NOT a.attisdropped \
               ORDER BY a.attnum) AS view_columns, \
         {collections} \
         FROM {}",
        crate::catalog::meta_table()
    )
}

/// Whether the change feed names collections: then the catalog rows read carry them.
fn feed_collections() -> bool {
    crate::config::publish_changes() == Some(crate::queue::published::Publish::Rows)
}

/// The cached catalog row of `entity`, unless `pg_tviews.table_cache_enabled` is
/// off, or the feed names collections and it was read without them.
fn cached(entity: &str) -> Option<Rc<TviewMeta>> {
    if !crate::config::table_cache_enabled() {
        return None;
    }
    let meta = crate::cache::METAS
        .with(|m| m.get(&entity.to_string()))
        .filter(|meta| meta.collections_read || !feed_collections());
    if meta.is_some() {
        crate::metrics::metrics_api::record_table_cache_hit();
    } else {
        crate::metrics::metrics_api::record_table_cache_miss();
    }
    meta
}

/// Cache a catalog row just read, unless the extension's own script is running
/// ([`meta_select`]).
fn remember(loaded: TviewMeta) -> Rc<TviewMeta> {
    let meta = Rc::new(loaded);
    if crate::revision::in_own_script() {
        return meta;
    }
    crate::cache::watch(&[meta.tview_oid, meta.view_oid]);
    // A parent's `id` column is part of how the collections are named.
    let parents: Vec<Oid> = meta
        .collections
        .iter()
        .map(|c| c.parent_table_oid)
        .collect();
    crate::cache::watch(&parents);
    crate::cache::METAS.with(|m| m.insert(meta.entity_name.clone(), meta.clone()));
    meta
}

impl TviewMeta {
    /// The rows' public id as text, read from `table`, its table's name or alias
    /// in a statement; `NULL` for a TVIEW without an `id` column.
    #[must_use]
    pub fn id_text(&self, table: &str) -> String {
        if self.public_id.is_some() {
            format!("{table}.\"id\"::pg_catalog.text")
        } else {
            "NULL::pg_catalog.text".to_string()
        }
    }

    /// The public id `text` a write returned ([`Self::id_text`]), with its kind.
    #[must_use]
    pub fn row_id(&self, text: String) -> RowId {
        RowId {
            text,
            kind: self.public_id.unwrap_or(PublicId::String),
        }
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

    /// Look up metadata by entity name (cached per backend, shared: a lookup
    /// copies nothing).
    ///
    /// # Errors
    /// The catalog cannot be read, or the row does not decode.
    pub fn load_by_entity(entity_name: &str) -> crate::TViewResult<Option<Rc<Self>>> {
        if let Some(meta) = cached(entity_name) {
            return Ok(Some(meta));
        }
        Self::load(&[entity_name.to_string()], entity_name)
    }

    /// `entity_name`'s catalog row ([`Self::load_by_entity`]), read on a miss with
    /// those of every other TVIEW whose plan maps `table_oid` and that is not
    /// cached: one catalog read for the TVIEWs a write to the table reaches.
    ///
    /// # Errors
    /// The catalog cannot be read, or `entity_name`'s row does not decode.
    pub fn load_reaching(
        entity_name: &str,
        table_oid: Oid,
    ) -> crate::TViewResult<Option<Rc<Self>>> {
        if let Some(meta) = cached(entity_name) {
            return Ok(Some(meta));
        }
        let mut entities = vec![entity_name.to_string()];
        if crate::config::table_cache_enabled() {
            let relid = table_oid.to_u32();
            entities.extend(
                crate::cache::plans()?
                    .iter()
                    .filter(|e| e.entity != entity_name)
                    .filter(|e| e.plan.tables.iter().any(|m| m.relid == relid))
                    .filter(|e| cached(&e.entity).is_none())
                    .map(|e| e.entity.clone()),
            );
        }
        Self::load(&entities, entity_name)
    }

    /// Read the catalog rows of `entities` and cache them; `wanted`'s. A row of
    /// another entity that does not decode is left for its own read.
    fn load(entities: &[String], wanted: &str) -> crate::TViewResult<Option<Rc<Self>>> {
        let mut found = None;
        for (entity, meta) in Self::read(
            "WHERE entity OPERATOR(pg_catalog.=) ANY ($1)",
            &[crate::utils::spi::text_array_of(entities)],
            Derived::Strict,
        )? {
            if entity == wanted {
                found = Some(remember(meta?));
            } else if let Ok(meta) = meta {
                remember(meta);
            }
        }
        Ok(found)
    }

    /// The catalog rows `clause` (a `WHERE` or `ORDER BY` clause) selects, each
    /// with its entity and decoded as `derived` says.
    fn read(
        clause: &str,
        args: &[pgrx::datum::DatumWithOid<'_>],
        derived: Derived,
    ) -> crate::TViewResult<Vec<(String, crate::TViewResult<Self>)>> {
        let with_collections = feed_collections();
        let sql = format!("{} {clause}", meta_select(with_collections));
        crate::utils::spi::catalog_rows(&sql, args, |row| {
            let entity = row.named::<String>("entity")?.unwrap_or_default();
            let meta = Self::decode(row, derived).map(|mut meta| {
                meta.collections_read = with_collections;
                meta
            });
            Ok((entity, meta))
        })
    }

    /// Load all TVIEW metadata
    ///
    /// # Errors
    /// The catalog cannot be read, or a row does not decode.
    pub fn load_all() -> crate::TViewResult<Vec<Self>> {
        Self::read("ORDER BY entity", &[], Derived::Strict)?
            .into_iter()
            .map(|(_, meta)| meta)
            .collect()
    }

    /// Every TVIEW's entity and plan, by entity: the write path's read of the
    /// whole catalog. It carries no per-TVIEW subquery and resolves no name, so a
    /// write does not depend on what the writer may see of the TVIEWs it does not
    /// reach (#251); their full rows are loaded by entity when a write reaches them.
    ///
    /// # Errors
    /// The catalog cannot be read, or a plan does not decode.
    pub fn load_plans() -> crate::TViewResult<Vec<PlanEntry>> {
        crate::utils::spi::catalog_rows(
            &format!(
                "SELECT entity, plan FROM {} ORDER BY entity",
                crate::catalog::meta_table()
            ),
            &[],
            |row| {
                let entity = row.named::<String>("entity")?.unwrap_or_default();
                let plan = plan::TviewPlan::decode(
                    &entity,
                    row.named::<pgrx::JsonB>("plan")?
                        .map_or(serde_json::Value::Null, |j| j.0),
                )?;
                Ok(PlanEntry { entity, plan })
            },
        )
    }

    /// The TVIEWs whose catalog row does not decode, each with why.
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn unreadable() -> crate::TViewResult<Vec<(String, String)>> {
        Ok(Self::read("ORDER BY entity", &[], Derived::Strict)?
            .into_iter()
            .filter_map(|(entity, meta)| meta.err().map(|e| (entity, e.to_string())))
            .collect())
    }

    /// The entity of the TVIEW whose table is `tview_oid`, read without decoding
    /// its plan: what dropping a TVIEW needs.
    ///
    /// # Errors
    /// The catalog cannot be read. Raises the revision error for a catalog of
    /// another revision.
    pub fn entity_of_table(tview_oid: Oid) -> crate::TViewResult<Option<String>> {
        // The hooks' first catalog read in a statement: a catalog of another
        // revision fails here with its remedy, not on a table or column it lacks.
        crate::revision::check();
        crate::utils::spi::one::<String>(
            &format!(
                "SELECT entity FROM {} WHERE table_oid::pg_catalog.oid = $1",
                crate::catalog::meta_table()
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
        Self::read(
            "WHERE entity OPERATOR(pg_catalog.=) ANY ($1)",
            &[crate::utils::spi::text_array_of(&[entity_name.to_string()])],
            Derived::Rederive,
        )?
        .into_iter()
        .next()
        .map(|(_, meta)| meta)
        .transpose()
    }

    fn decode(row: &crate::utils::spi::KeptRow<'_>, derived: Derived) -> crate::TViewResult<Self> {
        let entity_name: String =
            row.named::<String>("entity")?
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
            &row.named::<String>("uncascaded_policy")?
                .unwrap_or_default(),
        )?;

        let table_policies = row
            .named::<Vec<Oid>>("uncascaded_table_oids")?
            .unwrap_or_default()
            .into_iter()
            .zip(
                row.named::<Vec<String>>("uncascaded_table_policies")?
                    .unwrap_or_default(),
            )
            .map(|(oid, name)| Ok((oid, policy(&name)?)))
            .collect::<crate::TViewResult<_>>()?;

        let mut function_reads: Vec<(String, Vec<Oid>)> = Vec::new();
        for (function, table) in row
            .named::<Vec<String>>("function_read_functions")?
            .unwrap_or_default()
            .into_iter()
            .zip(
                row.named::<Vec<Option<Oid>>>("function_read_tables")?
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
            row.named::<pgrx::JsonB>("identity")?.map(|j| j.0).as_ref(),
        );
        let identity = match (identity, derived) {
            (Ok(identity), _) => identity,
            (Err(_), Derived::Rederive) => RowIdentity::unknown(),
            (Err(e), Derived::Strict) => return Err(e),
        };
        let plan = plan::TviewPlan::decode(
            &entity_name,
            row.named::<pgrx::JsonB>("plan")?
                .map_or(serde_json::Value::Null, |j| j.0),
        );
        let plan = match (plan, derived) {
            (Ok(plan), _) => plan,
            (Err(_), Derived::Rederive) => plan::TviewPlan::default(),
            (Err(e), Derived::Strict) => return Err(e),
        };

        let typename: Rc<str> =
            typename(&entity_name, row.named::<String>("graphql_typename")?).into();
        let collections = collections::decode(
            &typename,
            row.named::<pgrx::JsonB>("collections")?
                .map(|j| j.0)
                .as_ref(),
        );
        Ok(Self {
            tview_oid: row.named::<Oid>("tview_oid")?.ok_or_else(|| {
                crate::TViewError::SpiError {
                    query: String::new(),
                    error: "tview_oid column is NULL".to_string(),
                }
            })?,
            view_oid: row
                .named::<Oid>("view_oid")?
                .ok_or_else(|| crate::TViewError::SpiError {
                    query: String::new(),
                    error: "view_oid column is NULL".to_string(),
                })?,
            entity_name,

            plan,
            uncascaded_policy,
            table_policies,
            function_reads,
            time_refresh_external: row.named::<bool>("time_refresh_external")?.unwrap_or(false),
            time_dependent: row.named::<bool>("time_dependent")?.unwrap_or(false),
            identity,
            key_type: row
                .named::<String>("key_type")?
                .map_or(KeyType::Int, KeyType::Text),
            view_columns: row
                .named::<Vec<String>>("view_columns")?
                .unwrap_or_default(),
            typename,
            public_id: row.named::<bool>("public_id_number")?.map(|number| {
                if number {
                    PublicId::Number
                } else {
                    PublicId::String
                }
            }),
            collections,
            collections_read: false,
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
            key_type: KeyType::Int,
            view_columns: Vec::new(),
            typename: "".into(),
            public_id: None,
            collections: Vec::new(),
            collections_read: false,
        }
    }
}

/// The GraphQL type name of `entity`'s rows: the `typename` option when declared,
/// else the entity in Pascal case (`blog_post` → `BlogPost`), capitalising ASCII letters
/// only (GraphQL names are ASCII), so `tviews.registry.typename` derives the same name in
/// SQL whatever the database's locale. The name `pg_tviews_flush_and_report()` emits and
/// its `types` argument takes.
#[must_use]
pub fn typename(entity: &str, declared: Option<String>) -> String {
    declared.unwrap_or_else(|| {
        entity
            .split('_')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_public_id_is_written_in_json_as_to_jsonb_writes_it() {
        let meta = |public_id| super::TviewMeta {
            public_id,
            ..super::TviewMeta::default()
        };
        let number = meta(Some(super::PublicId::Number));
        let string = meta(Some(super::PublicId::String));
        assert_eq!(number.row_id("42".into()).json(), "42");
        assert_eq!(number.row_id("42".into()).value(), serde_json::json!(42));
        assert_eq!(string.row_id("8f0c".into()).json(), r#""8f0c""#);
        assert_eq!(string.row_id(r#"a"b\c"#.into()).json(), r#""a\"b\\c""#);
        assert_eq!(
            string.row_id("8f0c".into()).value(),
            serde_json::json!("8f0c")
        );
        assert_eq!(meta(None).id_text("t"), "NULL::pg_catalog.text");
        assert_eq!(string.id_text("t"), r#"t."id"::pg_catalog.text"#);
    }

    #[test]
    fn typename_is_the_option_or_the_pascal_case_of_the_entity() {
        assert_eq!(super::typename("post", None), "Post");
        assert_eq!(super::typename("blog_post", None), "BlogPost");
        assert_eq!(super::typename("order_line_item", None), "OrderLineItem");
        assert_eq!(super::typename("post", Some("Article".into())), "Article");
    }

    #[test]
    fn typename_capitalises_ascii_letters_only_whatever_the_locale() {
        // GraphQL names are ASCII; tviews.registry.typename derives the same name in SQL,
        // which must not depend on the database's ctype (Turkish dotted I, German sharp s).
        assert_eq!(super::typename("item", None), "Item");
        assert_eq!(super::typename("été", None), "été");
        assert_eq!(super::typename("ßeta_x", None), "ßetaX");
        assert_eq!(super::typename("_post__line_", None), "PostLine");
    }

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
