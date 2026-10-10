use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn occ(relid: u32, relname: &str) -> Occurrence {
    Occurrence {
        relid,
        relname: relname.to_string(),
        qualified: format!("public.{relname}"),
        unions: Vec::new(),
        via_view: None,
        via_tview: None,
        in_sublink: false,
        opaque_level: None,
        matview: false,
        tview_table: None,
    }
}

fn col(occ: usize, name: &str) -> Column {
    Column {
        occ,
        attnum: 1,
        name: name.to_string(),
    }
}

fn eq(a: Column, b: Column, a_to_b: bool, b_to_a: bool) -> Conjunct {
    let mut sql = a.sql();
    sql.push_text(" OPERATOR(pg_catalog.=) ");
    sql.push_sql(b.sql());
    Conjunct {
        sql,
        a: a.occ,
        b: b.occ,
        a_to_b: if a_to_b { Maps::Yes } else { Maps::No },
        b_to_a: if b_to_a { Maps::Yes } else { Maps::No },
        equality: Some((a, b)),
        lookups: vec![],
    }
}

fn graph(occurrences: Vec<Occurrence>, conjuncts: Vec<Conjunct>, key: Column) -> QueryGraph {
    QueryGraph {
        occurrences,
        conjuncts,
        roots: vec![Root {
            key,
            expr: None,
            scope: Vec::new(),
        }],
        holes: vec![],
        untracked_functions: vec![],
        time_reads: vec![],
        unread_tables: std::collections::BTreeSet::new(),
        identity: None,
        set_operation: false,
        virtual_columns: std::collections::BTreeSet::new(),
        tview_keys: BTreeMap::new(),
        data: None,
        outputs: vec![],
    }
}

const NONE: &dyn Fn(&str, u32) -> bool = &|_, _| false;

#[test]
fn root_is_local_on_its_key() {
    let g = graph(vec![occ(1, "tb_order")], vec![], col(0, "pk_order"));
    assert_eq!(g.classify(0, NONE), Kind::Local("pk_order".into()));
}

#[test]
fn direct_fk_is_local() {
    // tb_order o LEFT JOIN tb_line l ON l.fk_order = o.pk_order
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
}

#[test]
fn two_hops_are_mapped() {
    // tb_order o JOIN tb_line l ON l.fk_order = o.pk_order JOIN tb_sku s ON s.pk_sku = l.fk_sku
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line"), occ(3, "tb_sku")],
        vec![
            eq(col(1, "fk_order"), col(0, "pk_order"), true, true),
            eq(col(2, "pk_sku"), col(1, "fk_sku"), true, true),
        ],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![1, 0]));
}

#[test]
fn a_preserved_side_does_not_map_through_an_outer_join() {
    // tb_line l LEFT JOIN tb_order o ON o.pk_order = l.fk_order, key on o
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![eq(col(0, "pk_order"), col(1, "fk_order"), true, false)],
        col(0, "pk_order"),
    );
    assert!(matches!(g.classify(1, NONE), Kind::AllKeys(_)));
}

/// `l.fk_order = o2.pk_order` from `tb_line l LEFT JOIN tb_order o2`: it holds
/// for rows of `o2`, and toward `o2` only for a line that has a match.
fn outer_on(l: usize, o2: usize) -> Conjunct {
    let mut c = eq(col(l, "fk_order"), col(o2, "pk_order"), false, true);
    c.a_to_b = Maps::IfMatched;
    c
}

#[test]
fn a_nullable_step_is_taken_when_the_path_goes_on() {
    // tv over o, reading v_line (l LEFT JOIN o2) where v.order_id = o.id.
    let g = graph(
        vec![occ(1, "tb_order"), occ(1, "tb_order"), occ(2, "tb_line")],
        vec![outer_on(2, 1), eq(col(1, "id"), col(0, "id"), true, false)],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![0, 1]));
}

#[test]
fn a_nullable_step_may_end_at_the_key() {
    // tb_line l LEFT JOIN tb_order o, keyed on o: a line with no order has a
    // NULL key, which is no TVIEW row; one with an order maps to it.
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![outer_on(1, 0)],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
}

#[test]
fn a_nullable_step_goes_on_only_by_an_equality() {
    // After l → o2 (nullable), only an equality is known to fail on o2's NULLs.
    let mut sql = Sql::default();
    sql.push_text("COALESCE(o2.id, 0) IS NOT DISTINCT FROM o.id");
    let loose = Conjunct {
        sql,
        a: 1,
        b: 0,
        a_to_b: Maps::Yes,
        b_to_a: Maps::No,
        equality: None,
        lookups: vec![],
    };
    let g = graph(
        vec![occ(1, "tb_order"), occ(1, "tb_order"), occ(2, "tb_line")],
        vec![outer_on(2, 1), loose],
        col(0, "pk_order"),
    );
    assert!(matches!(g.classify(2, NONE), Kind::AllKeys(_)));
}

#[test]
fn an_unlinked_subquery_is_all_keys_with_the_reason() {
    let mut line = occ(2, "tb_line");
    line.in_sublink = true;
    let g = graph(vec![occ(1, "tb_order"), line], vec![], col(0, "pk_order"));
    assert_eq!(
        g.classify(1, NONE),
        Kind::AllKeys("read in a subquery, with no condition linking it to the TVIEW key".into())
    );
}

#[test]
fn an_opaque_top_level_without_a_root_is_all_keys_with_the_reason() {
    // SELECT pk_win, … count(*) OVER () FROM tb_win: the walker gives the
    // opaque top level no root and stamps its occurrences.
    let mut win = occ(1, "tb_win");
    win.opaque_level = Some("read under a window function in the top-level SELECT".into());
    let g = QueryGraph {
        occurrences: vec![win],
        conjuncts: vec![],
        roots: vec![],
        holes: vec![],
        untracked_functions: vec![],
        time_reads: vec![],
        unread_tables: std::collections::BTreeSet::new(),
        identity: None,
        set_operation: false,
        virtual_columns: std::collections::BTreeSet::new(),
        tview_keys: BTreeMap::new(),
        data: None,
        outputs: vec![],
    };
    assert_eq!(
        g.classify(0, NONE),
        Kind::AllKeys("read under a window function in the top-level SELECT".into())
    );
}

#[test]
fn an_embedded_tview_s_table_is_propagated() {
    let mut user = occ(3, "tb_user");
    user.via_tview = Some("user".into());
    let g = graph(
        vec![occ(1, "tb_post"), user],
        vec![eq(col(1, "pk_user"), col(0, "fk_user"), true, false)],
        col(0, "pk_post"),
    );
    assert_eq!(
        g.classify(1, &|e, _| e == "user"),
        Kind::Propagated("user".into())
    );
    assert!(matches!(g.classify(1, NONE), Kind::Mapped(_)));
}

#[test]
fn an_equality_wins_over_another_link() {
    // EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos AND l.fk_order = o.pk_order)
    let mut other = eq(col(1, "pos"), col(0, "min_pos"), true, false);
    other.equality = None;
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![
            other,
            eq(col(1, "fk_order"), col(0, "pk_order"), true, false),
        ],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(1, NONE), Kind::Local("fk_order".into()));
}

/// `a.pk_node = ANY (arr(n.path)) AND arr(n.path) @> ARRAY[a.pk_node]`, with the
/// array of `n` (occurrence `n`) looked up by a GIN index.
fn membership(a: usize, n: usize) -> Conjunct {
    let path = Column {
        occ: n,
        attnum: 3,
        name: "path".to_string(),
    };
    let mut array = Sql::text("(pg_catalog.string_to_array(");
    array.push_sql(path.sql());
    array.push_text(", '.'::pg_catalog.text))::bigint[]");
    let mut sql = Sql::text("((");
    sql.push_sql(col(a, "pk_node").sql());
    sql.push_text(" OPERATOR(pg_catalog.=) ANY (");
    sql.push_sql(array.clone());
    sql.push_text(")) AND (");
    sql.push_sql(array.clone());
    sql.push_text(") OPERATOR(pg_catalog.@>) ARRAY[");
    sql.push_sql(col(a, "pk_node").sql());
    sql.push_text("])");
    Conjunct {
        sql,
        a,
        b: n,
        a_to_b: Maps::Yes,
        b_to_a: Maps::Yes,
        equality: None,
        lookups: vec![Lookup {
            occ: n,
            expr: array,
            gin: true,
        }],
    }
}

#[test]
fn an_ancestor_read_through_array_membership_is_mapped() {
    // tb_node n JOIN tb_node a ON a.pk_node = ANY (string_to_array(n.path, '.')::bigint[])
    let g = graph(
        vec![occ(7, "tb_node"), occ(7, "tb_node")],
        vec![membership(1, 0)],
        col(0, "pk_node"),
    );
    assert_eq!(g.classify(0, NONE), Kind::Local("pk_node".into()));
    assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
    let tables = g.tables(NONE);
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].kind, TableKind::Mapped);
    assert_eq!(
        tables[0].sql.as_deref(),
        Some(
            "SELECT DISTINCT d.{c:7:1} FROM pg_tviews_delta d UNION \
             SELECT DISTINCT o1.{c:7:1} FROM pg_tviews_delta d, {r:7} o1 \
             WHERE ((d.{c:7:1} OPERATOR(pg_catalog.=) ANY ((pg_catalog.string_to_array(o1.{c:7:3}, \
             '.'::pg_catalog.text))::bigint[])) AND ((pg_catalog.string_to_array(o1.{c:7:3}, \
             '.'::pg_catalog.text))::bigint[]) OPERATOR(pg_catalog.@>) ARRAY[d.{c:7:1}])"
        )
    );
}

#[test]
fn a_lookup_by_expression_names_its_index() {
    let g = graph(
        vec![occ(7, "tb_node"), occ(7, "tb_node")],
        vec![membership(1, 0)],
        col(0, "pk_node"),
    );
    let tables = g.tables(NONE);
    assert_eq!(
        tables[0].index_hints,
        vec![IndexHint {
            table: "public.tb_node".into(),
            relname: "tb_node".into(),
            expr: "(pg_catalog.string_to_array({c:7:3}, '.'::pg_catalog.text))::bigint[]".into(),
            gin: true,
        }]
    );
}

#[test]
fn the_changed_side_of_a_lookup_needs_no_index() {
    // A write to n maps through its own key: its array is read off the change.
    let g = graph(
        vec![occ(7, "tb_node"), occ(8, "tb_ancestor")],
        vec![membership(1, 0)],
        col(0, "pk_node"),
    );
    let tables = g.tables(NONE);
    let node = tables.iter().find(|t| t.relname == "tb_node").unwrap();
    let ancestor = tables.iter().find(|t| t.relname == "tb_ancestor").unwrap();
    assert!(node.index_hints.is_empty());
    assert_eq!(ancestor.index_hints.len(), 1);
}

// ── embeds of other TVIEWs ─────────────────────────────────

/// `tb_user u` (0) joined to the backing view of `user_summary`, whose key is
/// `tb_order.fk_user` (1), projecting `outputs`.
fn summary_graph(join: Vec<Conjunct>, outputs: Vec<(&str, Column)>) -> QueryGraph {
    let mut g = graph(
        vec![occ(1, "tb_user"), occ(2, "tb_order")],
        join,
        col(0, "pk_user"),
    );
    g.tview_keys
        .insert("user_summary".into(), vec![col(1, "fk_user")]);
    g.outputs = outputs
        .into_iter()
        .map(|(name, c)| (name.to_string(), Some(c)))
        .collect();
    g
}

#[test]
fn an_embed_is_found_through_an_equality_with_its_key() {
    // tb_user u LEFT JOIN <summary view> s ON s.pk_user_summary = u.pk_user
    let g = summary_graph(
        vec![eq(col(1, "fk_user"), col(0, "pk_user"), true, false)],
        vec![("pk_user", col(0, "pk_user")), ("id", col(0, "id"))],
    );
    assert_eq!(
        g.embed_lookups(),
        BTreeMap::from([("user_summary".to_string(), vec!["pk_user".to_string()])])
    );
}

#[test]
fn an_embed_lookup_uses_the_output_name() {
    // tb_post p JOIN tv_tag_count c ON p.fk_author = c.pk_tag_count, `p.fk_author AS author`
    let mut g = graph(vec![occ(1, "tb_post")], vec![], col(0, "pk_post"));
    g.tview_keys
        .insert("tag_count".into(), vec![col(0, "fk_author")]);
    g.outputs = vec![
        ("pk_post".into(), Some(col(0, "pk_post"))),
        ("author".into(), Some(col(0, "fk_author"))),
    ];
    assert_eq!(
        g.embed_lookups(),
        BTreeMap::from([("tag_count".to_string(), vec!["author".to_string()])])
    );
}

#[test]
fn an_embed_read_twice_has_both_lookups() {
    // tb_doc d JOIN tv_user a ON a.pk_user = d.fk_author JOIN tv_user e ON e.pk_user = d.fk_editor
    let mut g = graph(vec![occ(1, "tb_doc")], vec![], col(0, "pk_doc"));
    g.tview_keys.insert(
        "user".into(),
        vec![col(0, "fk_author"), col(0, "fk_editor")],
    );
    g.outputs = vec![
        ("pk_doc".into(), Some(col(0, "pk_doc"))),
        ("fk_author".into(), Some(col(0, "fk_author"))),
        ("fk_editor".into(), Some(col(0, "fk_editor"))),
    ];
    assert_eq!(
        g.embed_lookups(),
        BTreeMap::from([(
            "user".to_string(),
            vec!["fk_author".to_string(), "fk_editor".to_string()]
        )])
    );
}

#[test]
fn an_embed_whose_key_no_output_carries_has_no_lookup() {
    // tb_post p JOIN <summary view> s ON s.pk_user_summary = p.fk_user, fk_user not projected
    let g = summary_graph(
        vec![eq(col(1, "fk_user"), col(0, "fk_user"), true, false)],
        vec![("pk_user", col(0, "pk_user"))],
    );
    assert_eq!(
        g.embed_lookups(),
        BTreeMap::from([("user_summary".to_string(), Vec::new())])
    );
}

#[test]
fn a_tview_not_read_has_no_lookup() {
    let g = graph(vec![occ(1, "tb_user")], vec![], col(0, "pk_user"));
    assert!(g.embed_lookups().is_empty());
}

// ── mapping queries (golden) ────────────────────────────────────────────

fn ne(a: Column, b: Column, op: &str) -> Conjunct {
    let mut c = eq(a, b, true, false);
    c.equality = None;
    let Piece::Text(t) = &mut c.sql.0[1] else {
        unreachable!()
    };
    *t = t.replace('=', op);
    c
}

#[test]
fn local_copy_of_the_key_skips_the_root() {
    // ARRAY(SELECT … FROM tb_line l WHERE l.fk_order = o.pk_order), as a
    // mapped table (with another occurrence) keeps the one-step simplification.
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
        col(0, "pk_order"),
    );
    assert_eq!(
        g.mapping_sql(&[(1, vec![0])]),
        "SELECT DISTINCT d.{c:2:1} FROM pg_tviews_delta d"
    );
}

#[test]
fn two_hops_join_the_intermediate_table() {
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line"), occ(3, "tb_sku")],
        vec![
            eq(col(1, "fk_order"), col(0, "pk_order"), true, true),
            eq(col(2, "pk_sku"), col(1, "fk_sku"), true, true),
        ],
        col(0, "pk_order"),
    );
    assert_eq!(
        g.mapping_sql(&[(2, vec![1, 0])]),
        "SELECT DISTINCT o1.{c:2:1} FROM pg_tviews_delta d, {r:2} o1 \
         WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:2:1}"
    );
}

#[test]
fn a_non_equality_keeps_the_root() {
    // EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos)
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![ne(col(1, "pos"), col(0, "min_pos"), ">")],
        col(0, "pk_order"),
    );
    assert_eq!(
        g.mapping_sql(&[(1, vec![0])]),
        "SELECT DISTINCT o1.{c:1:1} FROM pg_tviews_delta d, {r:1} o1 \
         WHERE d.{c:2:1} OPERATOR(pg_catalog.>) o1.{c:1:1}"
    );
}

#[test]
fn several_occurrences_union_their_queries() {
    // tb_node n LEFT JOIN tb_node p ON p.pk_node = n.fk_parent
    let g = graph(
        vec![occ(1, "tb_node"), occ(1, "tb_node")],
        vec![eq(col(1, "pk_node"), col(0, "fk_parent"), true, false)],
        col(0, "pk_node"),
    );
    assert_eq!(
        g.mapping_sql(&[(0, vec![]), (1, vec![0])]),
        "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d UNION SELECT DISTINCT o1.{c:1:1} \
         FROM pg_tviews_delta d, {r:1} o1 WHERE d.{c:1:1} OPERATOR(pg_catalog.=) o1.{c:1:1}"
    );
}

#[test]
#[allow(clippy::literal_string_with_formatting_args)] // Reason: `{r:7}` and `{c:7:2}` are template placeholders, not format arguments
fn templates_round_trip_braces_and_placeholders() {
    let template = format!(
        "SELECT d.{{c:7:2}} FROM {{r:7}} d WHERE d.{{c:7:3}} = {}",
        escape_template("'{r:1} }'")
    );
    let names = |p: Placeholder| match p {
        Placeholder::Relation(7) => Some("public.tb_x".to_string()),
        Placeholder::Column(7, 2) => Some("\"a\"".to_string()),
        Placeholder::Column(7, 3) => Some("b".to_string()),
        _ => None,
    };
    assert_eq!(
        fill_template(&template, &names).as_deref(),
        Some(r#"SELECT d."a" FROM public.tb_x d WHERE d.b = '{r:1} }'"#)
    );
    assert_eq!(
        template_placeholders(&template),
        vec![
            Placeholder::Column(7, 2),
            Placeholder::Relation(7),
            Placeholder::Column(7, 3)
        ]
    );
    assert_eq!(fill_template("{r:8}", &names), None);
    assert_eq!(fill_template("{x:1}", &names), None);
}

#[test]
fn a_tree_reads_the_key_of_each_occurrence() {
    // SELECT pk_tree, EXISTS (SELECT 1 FROM tb_tree c WHERE c.fk_parent = t.pk_tree)
    // FROM tb_tree t: the root occurrence and a local one on another column.
    let at = |occ: usize, name: &str, attnum: i16| Column {
        occ,
        attnum,
        name: name.to_string(),
    };
    let g = graph(
        vec![occ(1, "tb_tree"), occ(1, "tb_tree")],
        vec![eq(at(1, "fk_parent", 3), at(0, "pk_tree", 1), true, false)],
        at(0, "pk_tree", 1),
    );
    let tables = g.tables(NONE);
    assert_eq!(tables[0].kind, TableKind::Mapped);
    assert_eq!(
        tables[0].sql.as_deref(),
        Some(
            "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d \
             UNION SELECT DISTINCT d.{c:1:3} FROM pg_tviews_delta d"
        )
    );
}

#[test]
fn a_self_join_combines_occurrences() {
    // tb_node n LEFT JOIN tb_node p ON p.pk_node = n.fk_parent: the table is
    // the root (local) and reached through fk_parent (mapped).
    let g = graph(
        vec![occ(1, "tb_node"), occ(1, "tb_node")],
        vec![eq(col(1, "pk_node"), col(0, "fk_parent"), true, false)],
        col(0, "pk_node"),
    );
    let tables = g.tables(NONE);
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].kind, TableKind::Mapped);
    assert_eq!(tables[0].paths, vec![(0, vec![]), (1, vec![0])]);
}

#[test]
fn an_all_keys_table_keeps_the_mapping_of_its_traceable_reads() {
    // tb_order is the root, and read again in a subquery nothing links.
    let mut again = occ(1, "tb_order");
    again.in_sublink = true;
    let g = graph(vec![occ(1, "tb_order"), again], vec![], col(0, "pk_order"));
    let tables = g.tables(NONE);
    assert!(matches!(tables[0].kind, TableKind::AllKeys(_)));
    assert_eq!(tables[0].paths, vec![(0, vec![])]);
    assert!(tables[0].sql.is_some());
}

mod identity;
mod read_sets;
