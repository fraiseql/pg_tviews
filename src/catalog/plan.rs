//! Storing and loading a TVIEW's propagation plan (ADR 0203): the version check
//! of a stored document. The plan's types are [`crate::lineage::plan`]'s.

use crate::error::{TViewError, TViewResult};
use crate::lineage::plan::{PLAN_VERSION, PlanEmbed, TviewPlan};

/// The oldest version this library reads.
const OLDEST_READ: u32 = 1;

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
    use crate::lineage::plan::{FanoutPatch, LocalPath};
    use pgrx::pg_sys::Oid;

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
