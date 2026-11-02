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
