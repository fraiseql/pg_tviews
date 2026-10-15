//! Identities, UNION branch keys and reads of another TVIEW's table.

use super::*;

// ── identity ────────────────────────────────────────────────────────────

fn out(name: &str, junk: bool, sortgroupref: u32, column: Option<Column>) -> OutputColumn {
    OutputColumn {
        name: name.to_string(),
        junk,
        sortgroupref,
        columns: column.into_iter().collect(),
        type_oid: 20,
    }
}

fn at(occ: usize, name: &str, attnum: i16) -> Column {
    Column {
        occ,
        attnum,
        name: name.to_string(),
    }
}

const UNEQUAL: &dyn Fn(&Column, &Column) -> bool = &|_, _| false;

#[test]
fn identity_without_distinct_on_is_pk_entity() {
    let outputs = [
        out("id", false, 0, Some(at(0, "id", 2))),
        out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
    ];
    assert_eq!(
        select_identity("order", &outputs, None, UNEQUAL),
        Ok(SelectedIdentity {
            position: 1,
            kind: IdentityKind::Pk
        })
    );
}

#[test]
fn identity_without_pk_entity_is_missing() {
    let outputs = [out("id", false, 0, Some(at(0, "id", 2)))];
    assert_eq!(
        select_identity("order", &outputs, None, UNEQUAL),
        Err(IdentityError::Missing)
    );
}

#[test]
fn identity_is_a_projected_distinct_on_root_column() {
    // DISTINCT ON (c.id_contract) c.id_contract AS pk_contract
    let outputs = [
        out("pk_contract", false, 1, Some(at(0, "id_contract", 3))),
        out("id", false, 0, Some(at(0, "id", 2))),
    ];
    assert_eq!(
        select_identity("contract", &outputs, Some(&[1]), UNEQUAL),
        Ok(SelectedIdentity {
            position: 0,
            kind: IdentityKind::DistinctOn
        })
    );
}

#[test]
fn identity_is_a_projected_distinct_on_column_other_than_pk() {
    // DISTINCT ON (o.id) o.pk_order, o.id
    let outputs = [
        out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
        out("id", false, 1, Some(at(0, "id", 2))),
    ];
    assert_eq!(
        select_identity("order", &outputs, Some(&[1]), UNEQUAL),
        Ok(SelectedIdentity {
            position: 1,
            kind: IdentityKind::DistinctOn
        })
    );
}

#[test]
fn identity_is_a_projected_joined_column() {
    // DISTINCT ON (l.fk_order) l.fk_order AS pk_lastline, o.id … FROM tb_line l JOIN tb_order o
    let outputs = [
        out("pk_lastline", false, 1, Some(at(0, "fk_order", 3))),
        out("id", false, 0, Some(at(1, "id", 2))),
    ];
    assert_eq!(
        select_identity("lastline", &outputs, Some(&[1]), UNEQUAL),
        Ok(SelectedIdentity {
            position: 0,
            kind: IdentityKind::DistinctOn
        })
    );
}

#[test]
fn identity_is_a_projected_column_equal_to_an_unprojected_key() {
    // DISTINCT ON (l.fk_order) o.pk_order … JOIN ON o.pk_order = l.fk_order
    let outputs = [
        out("pk_order", false, 0, Some(at(1, "pk_order", 1))),
        out("id", false, 0, Some(at(1, "id", 2))),
        out("fk_order", true, 1, Some(at(0, "fk_order", 3))),
    ];
    let equal = |a: &Column, b: &Column| {
        a.occ != b.occ
            && [a.name.as_str(), b.name.as_str()].contains(&"fk_order")
            && [a.name.as_str(), b.name.as_str()].contains(&"pk_order")
    };
    assert_eq!(
        select_identity("order", &outputs, Some(&[1]), &equal),
        Ok(SelectedIdentity {
            position: 0,
            kind: IdentityKind::DistinctOn
        })
    );
}

#[test]
fn identity_of_an_unprojected_expression_is_refused() {
    // DISTINCT ON (lower(o.ref)) o.pk_order …
    let outputs = [
        out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
        out("?column?", true, 1, None),
    ];
    assert_eq!(
        select_identity("order", &outputs, Some(&[1]), UNEQUAL),
        Err(IdentityError::Unprojected)
    );
}

#[test]
fn identity_of_an_unprojected_column_nothing_equals_is_refused() {
    // DISTINCT ON (o.ref) o.pk_order …
    let outputs = [
        out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
        out("ref", true, 1, Some(at(0, "ref", 3))),
    ];
    assert_eq!(
        select_identity("order", &outputs, Some(&[1]), UNEQUAL),
        Err(IdentityError::Unprojected)
    );
}

#[test]
fn identity_of_a_projected_expression_is_refused() {
    // DISTINCT ON (lower(o.ref)) lower(o.ref) AS code, o.pk_order …
    let outputs = [
        out("code", false, 1, None),
        out("pk_order", false, 0, Some(at(0, "pk_order", 1))),
    ];
    assert_eq!(
        select_identity("order", &outputs, Some(&[1]), UNEQUAL),
        Err(IdentityError::NotAColumn)
    );
}

#[test]
fn identity_is_a_union_subquery_column_of_every_branch() {
    // DISTINCT ON (u.pk_task) u.pk_task … FROM (… tb_task UNION ALL … tb_task_copy) u
    let key = OutputColumn {
        name: "pk_task".to_string(),
        junk: false,
        sortgroupref: 1,
        columns: vec![at(0, "pk_task", 1), at(1, "pk_task", 1)],
        type_oid: 23,
    };
    let outputs = [key.clone(), out("id", false, 0, None)];
    assert_eq!(
        select_identity("task", &outputs, Some(&[1]), UNEQUAL),
        Ok(SelectedIdentity {
            position: 0,
            kind: IdentityKind::DistinctOn
        })
    );
    // Left unprojected, it is matched by the projected column standing for the
    // same branch columns.
    let junk = OutputColumn {
        junk: true,
        ..key.clone()
    };
    let projected = OutputColumn {
        sortgroupref: 0,
        name: "pk".to_string(),
        ..key
    };
    assert_eq!(
        select_identity("task", &[projected, junk], Some(&[1]), UNEQUAL),
        Ok(SelectedIdentity {
            position: 0,
            kind: IdentityKind::DistinctOn
        })
    );
}

#[test]
fn composite_identity_is_refused() {
    // DISTINCT ON (s.sku, s.warehouse)
    let outputs = [
        out("pk_stock", false, 0, Some(at(0, "pk_stock", 1))),
        out("sku", false, 1, Some(at(0, "sku", 3))),
        out("warehouse", false, 2, Some(at(0, "warehouse", 4))),
    ];
    assert_eq!(
        select_identity("stock", &outputs, Some(&[1, 2]), UNEQUAL),
        Err(IdentityError::Composite)
    );
}

#[test]
fn distinct_on_list_splits_top_level_commas() {
    assert_eq!(
        distinct_on_list(
            " SELECT DISTINCT ON (s.sku, lower((s.warehouse)::text), f(a, ')')) s.pk_stock"
        ),
        vec!["s.sku", "lower((s.warehouse)::text)", "f(a, ')')"]
    );
    assert_eq!(
        distinct_on_list(" SELECT DISTINCT ON (o.id) o.id"),
        vec!["o.id"]
    );
    assert!(distinct_on_list(" SELECT o.id FROM t").is_empty());
}

#[test]
fn a_root_keyed_on_a_virtual_column_is_mapped() {
    // DISTINCT ON (v.code) with code virtual: the row trigger cannot read it.
    let mut g = graph(vec![occ(1, "tb_ver")], vec![], col(0, "code"));
    g.virtual_columns.insert((0, 1));
    assert_eq!(g.classify(0, NONE), Kind::Mapped(vec![]));
    assert_eq!(
        g.mapping_sql(&[(0, vec![])]),
        "SELECT DISTINCT d.{c:1:1} FROM pg_tviews_delta d"
    );
}

#[test]
fn a_table_joined_on_a_virtual_column_is_mapped() {
    // tb_order o LEFT JOIN tb_ref r ON r.ord = o.pk_order, ord virtual
    let ord = Column {
        occ: 1,
        attnum: 4,
        name: "ord".into(),
    };
    let mut g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_ref")],
        vec![eq(ord, col(0, "pk_order"), true, false)],
        col(0, "pk_order"),
    );
    assert_eq!(g.classify(1, NONE), Kind::Local("ord".into()));
    g.virtual_columns.insert((1, 4));
    assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
    assert_eq!(
        g.mapping_sql(&[(1, vec![0])]),
        "SELECT DISTINCT d.{c:2:4} FROM pg_tviews_delta d"
    );
}

#[test]
fn a_virtual_column_read_adds_its_inputs() {
    let read = vec![("pk_shop".to_string(), 1), ("code".to_string(), 3)];
    let virtual_inputs = vec![
        (3, vec![("name".to_string(), 2)]),
        (5, vec![("other".to_string(), 4)]),
    ];
    assert_eq!(
        expand_read_columns(read, &virtual_inputs),
        vec![
            ("pk_shop".to_string(), 1),
            ("name".to_string(), 2),
            ("code".to_string(), 3)
        ]
    );
}

#[test]
fn virtual_reads_name_the_virtual_columns_read_and_their_inputs() {
    let read = vec![("name".to_string(), 2), ("code".to_string(), 3)];
    let virtual_inputs = vec![
        (3, vec![("name".to_string(), 2)]),
        (5, vec![("other".to_string(), 4)]),
    ];
    assert_eq!(virtual_reads(&read, &virtual_inputs), vec!["code", "name"]);
    assert!(virtual_reads(&read, &[]).is_empty());
}

#[test]
fn inputs_already_read_and_no_virtual_columns_change_nothing() {
    let read = vec![("price".to_string(), 2), ("taxed".to_string(), 3)];
    assert_eq!(
        expand_read_columns(read.clone(), &[(3, vec![("price".to_string(), 2)])]),
        read
    );
    assert_eq!(expand_read_columns(read.clone(), &[]), read);
}

#[test]
fn all_keys_wins_for_the_table() {
    let mut sub = occ(2, "tb_line");
    sub.in_sublink = true;
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line"), sub],
        vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
        col(0, "pk_order"),
    );
    let tables = g.tables(NONE);
    assert!(matches!(tables[1].kind, TableKind::AllKeys(_)));
}

// ── UNION branch keys, materialized views ─────────────────

/// `-<column>`: a UNION branch's key computed from one column.
fn negated(c: &Column) -> Sql {
    let mut sql = Sql::text("(OPERATOR(pg_catalog.-) ");
    sql.push_sql(c.sql());
    sql.push_text(")");
    sql
}

/// `SELECT p.pk_product … FROM tb_product p UNION ALL SELECT -l.pk_order_line …
/// FROM tb_order_line l`, read by a definition that joins `tb_note n` to the
/// union's id: occurrence 0 is p (leaf 0), 1 is l (leaf 1), 2 is n (outside).
fn two_branches() -> QueryGraph {
    let mut p = occ(1, "tb_product");
    p.unions = vec![(1, 0)];
    let mut l = occ(2, "tb_order_line");
    l.unions = vec![(1, 1)];
    let mut g = graph(
        vec![p, l, occ(3, "tb_note")],
        vec![
            eq(col(2, "target"), col(0, "id"), true, true),
            eq(col(2, "target"), col(1, "id"), true, true),
        ],
        col(0, "pk_product"),
    );
    g.roots = vec![
        Root {
            key: col(0, "pk_product"),
            expr: None,
            scope: vec![(1, 0)],
        },
        Root {
            key: col(1, "pk_order_line"),
            expr: Some(negated(&col(1, "pk_order_line"))),
            scope: vec![(1, 1)],
        },
    ];
    g
}

#[test]
fn scopes_meet_unless_they_take_different_leaves_of_one_union() {
    assert!(compatible(&vec![], &vec![(1, 0)]));
    assert!(compatible(&vec![(1, 0)], &vec![(1, 0), (2, 1)]));
    assert!(compatible(&vec![(1, 0)], &vec![(2, 1)]));
    assert!(!compatible(&vec![(1, 0)], &vec![(1, 1)]));
    assert!(!compatible(&vec![(2, 1), (1, 0)], &vec![(1, 1)]));
}

#[test]
fn each_branch_root_maps_its_own_rows() {
    let g = two_branches();
    assert_eq!(g.classify(0, NONE), Kind::Local("pk_product".into()));
    // A computed key is computed by the mapping query.
    assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![]));
    assert_eq!(
        g.mapping_sql(&[(1, vec![])]),
        "SELECT DISTINCT (OPERATOR(pg_catalog.-) d.{c:2:1}) FROM pg_tviews_delta d"
    );
}

#[test]
fn a_read_outside_the_union_maps_to_every_branch() {
    let g = two_branches();
    assert_eq!(g.classify(2, NONE), Kind::Branches(vec![vec![0], vec![1]]));
    let tables = g.tables(NONE);
    assert_eq!(tables[2].kind, TableKind::Mapped);
    assert_eq!(
        tables[2].sql.as_deref(),
        Some(
            "SELECT DISTINCT o1.{c:1:1} FROM pg_tviews_delta d, {r:1} o1 \
             WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:1:1} UNION \
             SELECT DISTINCT (OPERATOR(pg_catalog.-) o1.{c:2:1}) FROM pg_tviews_delta d, {r:2} o1 \
             WHERE d.{c:3:1} OPERATOR(pg_catalog.=) o1.{c:2:1}"
        )
    );
    // Two roots: rows are recomputed, never patched through a hop.
    assert_eq!(tables[2].hop, None);
}

#[test]
fn an_equality_with_a_computed_key_column_stands_for_the_root() {
    // tb_line.fk_order_line = l.pk_order_line, in the line branch.
    let mut g = two_branches();
    let mut line = occ(4, "tb_line");
    line.unions = vec![(1, 1)];
    g.occurrences.push(line);
    g.conjuncts.push(eq(
        col(3, "fk_order_line"),
        col(1, "pk_order_line"),
        true,
        true,
    ));
    assert_eq!(g.classify(3, NONE), Kind::Mapped(vec![2]));
    assert_eq!(
        g.mapping_sql(&[(3, vec![2])]),
        "SELECT DISTINCT (OPERATOR(pg_catalog.-) d.{c:4:1}) FROM pg_tviews_delta d"
    );
}

#[test]
fn a_branch_without_a_key_leaves_what_reaches_it_all_keys() {
    let mut g = two_branches();
    g.roots.pop();
    g.holes = vec![vec![(1, 1)]];
    assert_eq!(g.classify(0, NONE), Kind::Local("pk_product".into()));
    assert!(matches!(g.classify(1, NONE), Kind::AllKeys(r) if r.contains("every UNION branch")));
    assert!(matches!(g.classify(2, NONE), Kind::AllKeys(_)));
}

#[test]
fn a_materialized_view_is_all_keys_even_when_linked() {
    let mut g = graph(
        vec![occ(1, "tb_customer"), occ(2, "mv_order_count")],
        vec![eq(col(1, "fk_customer"), col(0, "pk_customer"), true, true)],
        col(0, "pk_customer"),
    );
    g.occurrences[1].matview = true;
    assert!(matches!(g.classify(1, NONE), Kind::AllKeys(r) if r.contains("materialized view")));
    assert!(g.tables(NONE)[1].matview);
}

// ── reads of another TVIEW's table ───────────────────────────────

/// `tb_note n` (root) and `tv_line l`, joined on `cond`.
fn note_and_line(cond: Conjunct) -> QueryGraph {
    let mut line = occ(2, "tv_line");
    line.tview_table = Some("line".into());
    graph(vec![occ(1, "tb_note"), line], vec![cond], col(0, "pk_note"))
}

#[test]
fn a_tview_table_joined_on_its_key_by_an_embed_is_propagated() {
    let g = note_and_line(eq(col(1, "pk_line"), col(0, "fk_line"), true, true));
    let embeds = |child: &str, _relid: u32| child == "line";
    assert_eq!(g.classify(1, &embeds), Kind::Propagated("line".into()));
    // Without the embed, its refreshes are mapped.
    assert_eq!(g.classify(1, NONE), Kind::Mapped(vec![0]));
}

#[test]
fn a_tview_table_linked_otherwise_is_mapped_never_local() {
    // l.order_id = n.pk_note: an equality with the key, which a base table
    // would read off its row (local).
    let g = note_and_line(eq(col(1, "order_id"), col(0, "pk_note"), true, true));
    let embeds = |child: &str, _relid: u32| child == "line";
    assert_eq!(g.classify(1, &embeds), Kind::Mapped(vec![0]));
    let tables = g.tables(&embeds);
    assert_eq!(tables[1].kind, TableKind::Mapped);
    assert_eq!(tables[1].tview.as_deref(), Some("line"));
    assert_eq!(
        g.tables(&embeds)[1].sql.as_deref(),
        Some("SELECT DISTINCT d.{c:2:1} FROM pg_tviews_delta d")
    );
}

/// `p.pk_product = f.fk_product` with `f.fk_product` an inbound column of a
/// first-row level: it maps a product toward the orders carrying it,
/// never an order away from it.
fn inbound(p: usize, f: usize) -> Conjunct {
    let mut c = eq(col(p, "pk_product"), col(f, "fk_product"), true, false);
    c.equality = None;
    c
}

#[test]
fn a_table_joined_to_an_inbound_column_maps_through_the_first_row_key() {
    // tb_customer c LEFT JOIN (first order per customer) f ON f.fk_customer = c.pk_customer
    // LEFT JOIN tb_product p ON p.pk_product = f.fk_product
    let g = graph(
        vec![
            occ(1, "tb_customer"),
            occ(2, "tb_order"),
            occ(3, "tb_product"),
        ],
        vec![
            eq(col(1, "fk_customer"), col(0, "pk_customer"), true, false),
            inbound(2, 1),
        ],
        col(0, "pk_customer"),
    );
    assert_eq!(g.classify(2, NONE), Kind::Mapped(vec![1, 0]));
    assert_eq!(g.classify(1, NONE), Kind::Local("fk_customer".into()));
}

#[test]
fn a_first_row_level_does_not_map_away_through_an_inbound_column() {
    // tb_product p, (SELECT count(*) FROM first orders f WHERE f.fk_product = p.pk_product)
    let g = graph(
        vec![occ(3, "tb_product"), occ(2, "tb_order")],
        vec![inbound(0, 1)],
        col(0, "pk_product"),
    );
    assert!(matches!(g.classify(1, NONE), Kind::AllKeys(_)));
}

fn table(relid: u32, kind: TableKind, sql: Option<&str>) -> TableLineage {
    TableLineage {
        relid,
        relname: format!("t{relid}"),
        qualified: format!("public.t{relid}"),
        kind,
        paths: vec![],
        sql: sql.map(str::to_string),
        reads: vec![],
        columns: vec![],
        lookups: vec![],
        index_hints: vec![],
        hop: None,
        fanout: None,
        root: false,
        virtual_reads: vec![],
        matview: false,
        tview: None,
    }
}

#[test]
fn a_function_read_is_all_keys_and_keeps_the_traced_reads() {
    let mut lineage = Lineage {
        tables: vec![
            table(1, TableKind::Local("pk_a".into()), None),
            table(2, TableKind::Mapped, Some("SELECT 1")),
        ],
        unread: vec![],
        identity: Identity {
            name: "pk_a".into(),
            type_oid: 20,
            kind: IdentityKind::Pk,
            columns: vec![],
        },
        set_operation: false,
        aggregate_embeds: vec![],
        functions: vec![],
        time_reads: vec![],
        tview_reads: BTreeMap::new(),
        keyed_otherwise: BTreeSet::new(),
        data: None,
    };
    let read = |relid: u32| FunctionRead {
        function: "public.f()".into(),
        relid,
        relname: format!("t{relid}"),
        qualified: format!("public.t{relid}"),
        matview: false,
        tview: None,
    };
    lineage.add_function_reads(&[read(1), read(2), read(3)]);
    let reason = TableKind::AllKeys("read inside public.f()".into());
    assert_eq!(lineage.tables[0].kind, reason);
    assert_eq!(
        lineage.tables[0].sql.as_deref(),
        Some("SELECT DISTINCT \"pk_a\" FROM pg_tviews_delta")
    );
    assert_eq!(lineage.tables[1].kind, reason);
    assert_eq!(lineage.tables[1].sql.as_deref(), Some("SELECT 1"));
    assert_eq!(lineage.tables[2].kind, reason);
    assert_eq!(lineage.tables[2].sql, None);
}
