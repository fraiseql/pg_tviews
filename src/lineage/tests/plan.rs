//! The plan assembled from a lineage: key mappings with their fan-out patches.

use super::super::plan::{FanoutPatch, MappingKind, assemble};
use super::super::*;

fn table(relid: u32, relname: &str, kind: TableKind) -> TableLineage {
    TableLineage {
        relid,
        relname: relname.into(),
        qualified: format!("public.{relname}"),
        kind,
        paths: Vec::new(),
        sql: None,
        reads: Vec::new(),
        columns: Vec::new(),
        lookups: Vec::new(),
        index_hints: Vec::new(),
        hop: None,
        via: None,
        fanout: None,
        root: false,
        virtual_reads: Vec::new(),
        unwatched: None,
        tview: None,
        links: None,
    }
}

/// `tv_post`: `tb_post` (root) joined to `tb_user` on `fk_user`, whose name is copied
/// into `data` as `author`.
fn post_lineage(kind: IdentityKind) -> Lineage {
    let mut post = table(10, "tb_post", TableKind::Local("pk_post".into()));
    post.root = true;
    post.columns = vec![("pk_post".into(), 1), ("fk_user".into(), 3)];
    post.links = Some(vec![1, 3]);
    let mut user = table(20, "tb_user", TableKind::Mapped);
    user.sql = Some("SELECT DISTINCT o1.{c:10:1} FROM pg_tviews_delta d, {r:10} o1".into());
    user.columns = vec![("pk_user".into(), 1), ("name".into(), 2), ("bio".into(), 4)];
    user.hop = Some(("pk_user".into(), "fk_user".into()));
    user.fanout = Some((
        "fk_user".into(),
        vec![
            ("name".into(), vec!["author".into()]),
            ("bio".into(), vec!["bio".into()]),
        ],
    ));
    user.virtual_reads = vec!["bio".into()];
    user.reads = vec![ReadSet {
        attnum: 1,
        sql: Some("SELECT 1".into()),
    }];
    Lineage {
        tables: vec![post, user],
        unread: Vec::new(),
        identity: Identity {
            name: "pk_post".into(),
            type_oid: 20,
            kind,
            columns: vec![(10, 1)],
        },
        set_operation: false,
        aggregate_embeds: Vec::new(),
        functions: Vec::new(),
        time_reads: Vec::new(),
        tview_reads: std::collections::BTreeMap::new(),
        keyed_otherwise: std::collections::BTreeSet::new(),
        data: None,
        references: Vec::new(),
    }
}

#[test]
fn a_mapped_table_one_hop_away_gets_its_fanout_patch() {
    let plan = assemble("post", &post_lineage(IdentityKind::Pk), Vec::new());
    let post = &plan.tables[0];
    assert_eq!(
        (post.kind, post.column.as_deref(), post.sql.as_deref()),
        (MappingKind::Local, Some("pk_post"), None)
    );
    assert_eq!(post.attnums, [1, 3]);
    assert_eq!(post.links.as_deref(), Some(&[1, 3][..]));
    let user = &plan.tables[1];
    assert_eq!(user.kind, MappingKind::Mapped);
    assert_eq!(user.hop, Some(("pk_user".into(), "fk_user".into())));
    assert_eq!(user.key_col.as_deref(), Some("pk_user"));
    // The virtual generated column is never patched: the trigger sees it NULL.
    assert_eq!(
        user.fanout,
        Some(FanoutPatch {
            lookup_col: "fk_user".into(),
            fields: vec![("name".into(), vec!["author".into()])],
            via: None,
        })
    );
    assert_eq!(user.reads.len(), 1);
}

#[test]
fn a_distinct_on_tview_gets_no_fanout_patch() {
    let plan = assemble("post", &post_lineage(IdentityKind::DistinctOn), Vec::new());
    assert!(
        plan.tables
            .iter()
            .all(|t| t.fanout.is_none() && t.key_col.is_none())
    );
}

#[test]
fn a_mapped_table_without_a_query_stores_an_empty_one() {
    let mut lineage = post_lineage(IdentityKind::Pk);
    lineage.tables[1].sql = None;
    let plan = assemble("post", &lineage, Vec::new());
    assert_eq!(plan.tables[1].sql.as_deref(), Some(""));
}

#[test]
fn a_template_is_rewritten_with_its_escapes_kept() {
    let rewritten = rewrite_template("SELECT {c:10:3}, '{{x}}' FROM {r:10} JOIN {r:20}", &|p| {
        Some(match p {
            Placeholder::Relation(r) => Placeholder::Relation(r + 1),
            Placeholder::Column(r, a) => Placeholder::Column(r + 1, a - 1),
        })
    });
    assert_eq!(
        rewritten.as_deref(),
        Some("SELECT {c:11:2}, '{{x}}' FROM {r:11} JOIN {r:21}")
    );
    let relation = format!("{{r:{}}}", 10);
    assert_eq!(rewrite_template(&relation, &|_| None), None);
}

#[test]
fn a_restored_plan_is_rebound_by_relid_and_column_number() {
    let mut post = post_lineage(IdentityKind::Pk);
    post.tables[0].reads = vec![ReadSet {
        attnum: 3,
        sql: Some("SELECT o.{c:10:3} FROM {r:10} o".into()),
    }];
    let paths = vec![super::super::plan::LocalPath {
        source_oid: pgrx::pg_sys::Oid::from(10),
        source_table: "tb_post".into(),
        entity_name: "post".into(),
        initial_col: "pk_post".into(),
        source_columns: vec!["pk_post".into()],
        root: true,
        initial_attnum: Some(1),
    }];
    let plan = assemble("post", &post, paths);
    assert!(
        plan.numbered_columns()
            .contains(&(10, 3, "fk_user".to_string()))
    );
    // tb_post 10 -> 110 with fk_user 3 -> 2 (a dropped column before it);
    // tb_user 20 -> 120, numbers unchanged.
    let relid = |r: u32| Some(r + 100);
    let attnum = |r: u32, a: i16| Some(if r == 10 && a == 3 { 2 } else { a });
    let rebound = plan.rebound(&relid, &attnum).unwrap();
    let post = &rebound.tables[0];
    assert_eq!((post.relid, post.attnums.clone()), (110, vec![1, 2]));
    assert_eq!(post.links.as_deref(), Some(&[1, 2][..]));
    assert_eq!(post.reads[0].attnum, 2);
    assert_eq!(
        post.reads[0].sql.as_deref(),
        Some("SELECT o.{c:110:2} FROM {r:110} o")
    );
    let user = &rebound.tables[1];
    assert_eq!(
        user.sql.as_deref(),
        Some("SELECT DISTINCT o1.{c:110:1} FROM pg_tviews_delta d, {r:110} o1")
    );
    assert_eq!(rebound.paths[0].source_oid, pgrx::pg_sys::Oid::from(110));
    // A number the plan does not name has no new value.
    let unnamed = |r: u32, a: i16| (r != 10 || a != 3).then_some(a);
    assert_eq!(
        plan.rebound(&relid, &unnamed),
        Err(super::super::plan::Unbound::Unnamed(
            "public.tb_post".into(),
            3
        ))
    );
}
