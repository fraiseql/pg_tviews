//! A TVIEW's propagation plan (ADR 0203): everything registration derives from
//! its backing view's query tree, stored as one versioned document in
//! `pg_tview_meta.plan` and read back by the triggers and the flush.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use serde::{Deserialize, Serialize};

/// The version this library writes. It also reads version 1, a plan without
/// references, and version 2, one without the columns its rows are linked by
/// (what the upgrade scripts of earlier releases write before they re-derive);
/// a plan of any other version is re-derived by `ALTER EXTENSION pg_tviews
/// UPDATE`, never read.
pub const PLAN_VERSION: u32 = 3;

/// The oldest version this library reads.
const OLDEST_READ: u32 = 1;

/// What a TVIEW's definition says about propagating writes to its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TviewPlan {
    pub version: u32,
    /// The rows come from the branches of a set operation (UNION, INTERSECT,
    /// EXCEPT): they are recomputed, never patched.
    #[serde(default)]
    pub set_operation: bool,
    /// The TVIEWs it embeds.
    #[serde(default)]
    pub embeds: Vec<PlanEmbed>,
    /// The direct-patch map: `(column of the table holding the identity, data
    /// key)` for the columns copied into `data` and read nowhere else.
    #[serde(default)]
    pub direct: Vec<(String, String)>,
    /// How a write to each base table maps to its keys (ADR 0157): what the
    /// statement trigger runs.
    #[serde(default)]
    pub tables: Vec<crate::lineage::KeyMapping>,
    /// The tables whose rows carry the key: what the row trigger reads.
    #[serde(default)]
    pub paths: Vec<LocalPath>,
    /// The output columns a foreign key of the root table proves to hold another
    /// table's key (`tviews.registry.references` publishes those naming a TVIEW).
    #[serde(default)]
    pub references: Vec<crate::lineage::Reference>,
}

/// Another TVIEW this one embeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEmbed {
    pub entity: String,
    /// The output columns of this TVIEW equal to the child's `pk_<child>`, one
    /// per read of the child: the parents of a refreshed child row are the rows
    /// holding its key in one of them.
    pub lookups: Vec<String>,
    pub kind: crate::lineage::EmbedKind,
    /// Where the child's document lands in `data` (empty for a scalar embed).
    #[serde(default)]
    pub path: Vec<String>,
}

/// A table whose changed row holds a key of the TVIEW in `initial_col`: the
/// table holding the identity (`root`), a table joined on the key, or a source of
/// an aggregate TVIEW's group key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPath {
    pub source_oid: Oid,
    pub source_table: String,
    pub entity_name: String,
    pub initial_col: String,
    /// The columns of the table the TVIEW reads: an UPDATE touching none of
    /// them changes no row. Empty means unknown: every UPDATE refreshes.
    #[serde(default)]
    pub source_columns: Vec<String>,
    /// The table holds the TVIEW's identity: its own rows are the TVIEW's rows.
    #[serde(default)]
    pub root: bool,
    /// The attribute number of `initial_col` when registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_attnum: Option<i16>,
}

/// How an UPDATE of a mapped table is written into every TVIEW row it reaches
/// in one statement: the rows whose `lookup_col` holds the changed
/// row's key, each changed column in `fields` written at its path in `data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutPatch {
    pub lookup_col: String,
    /// `(source column, path in data)` pairs copied unchanged; a column copied
    /// to several paths has a pair for each.
    #[serde(deserialize_with = "fanout_fields")]
    pub fields: Vec<(String, Vec<String>)>,
    /// For a table two equalities away, through a table in between: the query
    /// template from the changed rows' key values (`$1`, `int8[]`) to the
    /// `lookup_col` values they reach, as `(key, lookup value)` pairs (#228).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// The fields of a fan-out patch. A plan written before nested paths holds a
/// top-level key where a path now is: it reads as that one-key path.
fn fanout_fields<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(String, Vec<String>)>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Path {
        Keys(Vec<String>),
        Key(String),
    }
    let fields: Vec<(String, Path)> = Vec::deserialize(deserializer)?;
    Ok(fields
        .into_iter()
        .map(|(column, path)| match path {
            Path::Keys(keys) => (column, keys),
            Path::Key(key) => (column, vec![key]),
        })
        .collect())
}

impl TviewPlan {
    /// Decode `entity`'s stored plan.
    ///
    /// # Errors
    /// A [`TViewError::CatalogError`] naming the entity when the document does not
    /// decode or is of another version: never read as "no dependencies".
    pub fn decode(entity: &str, json: serde_json::Value) -> TViewResult<Self> {
        let plan: Self = serde_json::from_value(json).map_err(|e| TViewError::CatalogError {
            operation: format!("Read the propagation plan of tv_{entity}"),
            pg_error: e.to_string(),
        })?;
        if !(OLDEST_READ..=PLAN_VERSION).contains(&plan.version) {
            return Err(TViewError::CatalogError {
                operation: format!("Read the propagation plan of tv_{entity}"),
                pg_error: format!(
                    "plan version {} is not {PLAN_VERSION}: ALTER EXTENSION pg_tviews UPDATE \
                     re-derives it",
                    plan.version
                ),
            });
        }
        if let Some(table) = plan
            .tables
            .iter()
            .find(|t| t.kind == crate::lineage::MappingKind::Mapped && t.sql.is_none())
        {
            return Err(TViewError::CatalogError {
                operation: format!("Read the propagation plan of tv_{entity}"),
                pg_error: format!(
                    "writes to {} map through a query, and the plan stores none",
                    table.table
                ),
            });
        }
        Ok(plan)
    }

    /// The columns of the TVIEW's table its rows are looked up by: those holding
    /// an embedded TVIEW's key, those a fan-out patch writes through, and its
    /// references, which a reader composes by. Each needs an index leading with it.
    #[must_use]
    pub fn lookup_columns(&self) -> std::collections::BTreeSet<&str> {
        self.embeds
            .iter()
            .flat_map(|e| e.lookups.iter().map(String::as_str))
            .chain(
                self.tables
                    .iter()
                    .filter_map(|t| t.fanout.as_ref().map(|f| f.lookup_col.as_str())),
            )
            .chain(self.references.iter().map(|r| r.column.as_str()))
            .collect()
    }

    /// How this TVIEW embeds `child`, if it does.
    #[must_use]
    pub fn embed(&self, child: &str) -> Option<&PlanEmbed> {
        self.embeds.iter().find(|e| e.entity == child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_round_trips() {
        let plan = TviewPlan {
            version: PLAN_VERSION,
            set_operation: false,
            embeds: vec![PlanEmbed {
                entity: "user".into(),
                lookups: vec!["author_pk".into()],
                kind: crate::lineage::EmbedKind::Nested,
                path: vec!["author".into()],
            }],
            direct: vec![("title".into(), "title".into())],
            tables: vec![],
            paths: vec![LocalPath {
                source_oid: Oid::from(1),
                source_table: "tb_post".into(),
                entity_name: "post".into(),
                initial_col: "pk_post".into(),
                source_columns: vec!["title".into()],
                root: true,
                initial_attnum: Some(1),
            }],
            references: vec![crate::lineage::Reference {
                column: "fk_author".into(),
                source: "fk_author".into(),
                table: "public.tb_user".into(),
                key: "pk_user".into(),
                constraint: "tb_post_fk_author_fkey".into(),
                unique: false,
                nullable: false,
            }],
        };
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(TviewPlan::decode("post", json).unwrap(), plan);
    }

    #[test]
    fn lookup_columns_are_embed_lookups_and_fanout_columns() {
        let plan = TviewPlan::decode(
            "post",
            serde_json::json!({
                "version": PLAN_VERSION,
                "embeds": [{"entity": "user", "lookups": ["author_pk", "editor_pk"],
                            "kind": "nested_object", "path": ["author"]}],
                "tables": [{"relid": 1, "table": "public.tb_tag", "kind": "mapped", "sql": "",
                            "fanout": {"lookup_col": "tag_pk", "fields": []}}]
            }),
        )
        .unwrap();
        assert_eq!(
            plan.lookup_columns().into_iter().collect::<Vec<_>>(),
            ["author_pk", "editor_pk", "tag_pk"]
        );
    }

    #[test]
    fn reference_columns_are_lookup_columns() {
        let plan = TviewPlan::decode(
            "comment",
            serde_json::json!({
                "version": PLAN_VERSION,
                "references": [{"column": "author_ref", "source": "author_ref",
                                "table": "public.tb_user", "key": "pk_user",
                                "constraint": "c", "unique": false, "nullable": true}]
            }),
        )
        .unwrap();
        assert_eq!(
            plan.lookup_columns().into_iter().collect::<Vec<_>>(),
            ["author_ref"]
        );
    }

    #[test]
    fn a_version_1_plan_reads_as_one_without_references() {
        // What upgrade scripts of earlier releases write before re-deriving.
        let plan = TviewPlan::decode("post", serde_json::json!({"version": 1})).unwrap();
        assert!(plan.references.is_empty());
        let err = TviewPlan::decode("post", serde_json::json!({"version": 4})).unwrap_err();
        assert!(err.to_string().contains("plan version 4 is not 3"), "{err}");
    }

    #[test]
    fn fanout_fields_read_paths_and_top_level_keys() {
        let fanout: FanoutPatch = serde_json::from_value(serde_json::json!({
            "lookup_col": "fk_user",
            "fields": [["name", "author_name"], ["bio", ["author", "bio"]]]
        }))
        .unwrap();
        assert_eq!(
            fanout.fields,
            [
                ("name".to_string(), vec!["author_name".to_string()]),
                (
                    "bio".to_string(),
                    vec!["author".to_string(), "bio".to_string()]
                )
            ]
        );
    }

    fn with_table(table: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({"version": PLAN_VERSION, "tables": [table]})
    }

    #[test]
    fn a_mapping_kind_decodes_by_name() {
        let plan = TviewPlan::decode(
            "post",
            with_table(&serde_json::json!({
                "relid": 1, "table": "public.tb_tag", "kind": "all_keys", "reason": "r"
            })),
        )
        .unwrap();
        assert_eq!(plan.tables[0].kind, crate::lineage::MappingKind::AllKeys);
    }

    #[test]
    fn read_sets_decode_and_default_to_none() {
        let plan = TviewPlan::decode(
            "post",
            with_table(&serde_json::json!({
                "relid": 1, "table": "public.tb_user", "kind": "mapped", "sql": "",
                "reads": [{"attnum": 1, "sql": "SELECT 1"}, {"attnum": 0}]
            })),
        )
        .unwrap();
        assert_eq!(
            plan.tables[0].reads,
            [
                crate::lineage::ReadSet {
                    attnum: 1,
                    sql: Some("SELECT 1".into())
                },
                crate::lineage::ReadSet {
                    attnum: 0,
                    sql: None
                }
            ]
        );
        let older = TviewPlan::decode(
            "post",
            with_table(&serde_json::json!({
                "relid": 1, "table": "public.tb_user", "kind": "mapped", "sql": ""
            })),
        )
        .unwrap();
        assert!(older.tables[0].reads.is_empty());
    }

    #[test]
    fn an_unknown_mapping_kind_is_refused() {
        let err = TviewPlan::decode(
            "post",
            with_table(&serde_json::json!({"relid": 1, "table": "public.tb_tag", "kind": "fk"})),
        )
        .unwrap_err();
        assert!(err.to_string().contains("tv_post"), "{err}");
    }

    #[test]
    fn a_mapped_table_without_its_query_is_refused() {
        let err = TviewPlan::decode(
            "post",
            with_table(
                &serde_json::json!({"relid": 1, "table": "public.tb_tag", "kind": "mapped"}),
            ),
        )
        .unwrap_err();
        assert!(err.to_string().contains("public.tb_tag"), "{err}");
    }

    #[test]
    fn another_version_or_shape_is_a_catalog_error() {
        let other = serde_json::json!({"version": 99});
        assert!(matches!(
            TviewPlan::decode("post", other),
            Err(TViewError::CatalogError { .. })
        ));
        let broken = serde_json::json!({"version": 1, "embeds": 3});
        assert!(TviewPlan::decode("post", broken).is_err());
    }
}
