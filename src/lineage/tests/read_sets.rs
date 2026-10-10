//! Read sets (ADR 0207): from a TVIEW's keys to the values a mapped table's
//! joined column is compared with, so a refresh locks what a writer of that
//! table locks.

use super::*;

const POST: u32 = 10;
const USER: u32 = 20;
const ORG: u32 = 30;

fn column(occ: usize, attnum: i16, name: &str) -> Column {
    Column {
        occ,
        attnum,
        name: name.to_string(),
    }
}

/// `tb_post p JOIN tb_user u ON u.pk_user = p.fk_user JOIN tb_org o ON o.pk_org =
/// u.fk_org`, keyed on `p.pk_post`. Attnums: post (pk 1, `fk_user` 3, `fk_editor` 4),
/// user (pk 1, `fk_org` 4), org (pk 1).
fn post_user_org() -> QueryGraph {
    graph(
        vec![
            occ(POST, "tb_post"),
            occ(USER, "tb_user"),
            occ(ORG, "tb_org"),
        ],
        vec![
            eq(column(1, 1, "pk_user"), column(0, 3, "fk_user"), true, true),
            eq(column(2, 1, "pk_org"), column(1, 4, "fk_org"), true, true),
        ],
        column(0, 1, "pk_post"),
    )
}

#[test]
fn one_hop_reads_the_join_value_from_the_root() {
    assert_eq!(
        post_user_org().read_sets(&[(1, vec![0])]),
        vec![ReadSet {
            attnum: 1,
            sql: Some(
                "SELECT DISTINCT o1.{c:10:3} FROM {r:10} o1 \
                 WHERE o1.{c:10:1} OPERATOR(pg_catalog.=) ANY ($1)"
                    .into()
            ),
        }]
    );
}

#[test]
fn two_hops_walk_back_from_the_root() {
    assert_eq!(
        post_user_org().read_sets(&[(2, vec![1, 0])]),
        vec![ReadSet {
            attnum: 1,
            sql: Some(
                "SELECT DISTINCT o1.{c:20:4} FROM {r:10} o2, {r:20} o1 \
                 WHERE o2.{c:10:1} OPERATOR(pg_catalog.=) ANY ($1) \
                 AND o1.{c:20:1} OPERATOR(pg_catalog.=) o2.{c:10:3}"
                    .into()
            ),
        }]
    );
}

#[test]
fn reads_of_the_same_column_are_one_union() {
    // ... JOIN tb_user e ON e.pk_user = p.fk_editor: two reads of tb_user.pk_user.
    let mut g = post_user_org();
    g.occurrences.push(occ(USER, "tb_user"));
    g.conjuncts.push(eq(
        column(3, 1, "pk_user"),
        column(0, 4, "fk_editor"),
        true,
        true,
    ));
    let sets = g.read_sets(&[(1, vec![0]), (3, vec![2])]);
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].attnum, 1);
    assert_eq!(
        sets[0].sql.as_deref(),
        Some(
            "SELECT DISTINCT o1.{c:10:3} FROM {r:10} o1 \
             WHERE o1.{c:10:1} OPERATOR(pg_catalog.=) ANY ($1) \
             UNION SELECT DISTINCT o1.{c:10:4} FROM {r:10} o1 \
             WHERE o1.{c:10:1} OPERATOR(pg_catalog.=) ANY ($1)"
        )
    );
}

#[test]
fn an_outer_join_reads_the_value_even_without_a_match() {
    // tb_post p LEFT JOIN tb_user u ON u.pk_user = p.fk_user: a post naming a
    // user that doesn't exist yet still yields its fk_user, locked by value.
    let mut g = post_user_org();
    g.conjuncts[0].a_to_b = Maps::IfMatched;
    assert_eq!(
        g.read_sets(&[(1, vec![0])]),
        post_user_org().read_sets(&[(1, vec![0])])
    );
}

#[test]
fn a_join_on_the_key_reads_the_keys() {
    // tb_order o JOIN tb_line l ON l.fk_order = o.pk_order (mapped beside another
    // occurrence): the value is the key itself.
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![eq(
            column(1, 2, "fk_order"),
            column(0, 1, "pk_order"),
            true,
            false,
        )],
        column(0, 1, "pk_order"),
    );
    assert_eq!(
        g.read_sets(&[(1, vec![0])]),
        vec![ReadSet {
            attnum: 2,
            sql: Some(
                "SELECT DISTINCT o1.{c:1:1} FROM {r:1} o1 \
                 WHERE o1.{c:1:1} OPERATOR(pg_catalog.=) ANY ($1)"
                    .into()
            ),
        }]
    );
}

#[test]
fn a_join_by_no_equality_locks_the_table() {
    // EXISTS (SELECT 1 FROM tb_line l WHERE l.pos > o.min_pos)
    let g = graph(
        vec![occ(1, "tb_order"), occ(2, "tb_line")],
        vec![ne(col(1, "pos"), col(0, "min_pos"), ">")],
        col(0, "pk_order"),
    );
    assert_eq!(
        g.read_sets(&[(1, vec![0])]),
        vec![ReadSet {
            attnum: 0,
            sql: None
        }]
    );
}

#[test]
fn the_table_holding_the_key_needs_no_read_set() {
    // A self-join: the root occurrence's own rows are the TVIEW's rows.
    let g = graph(
        vec![occ(1, "tb_node"), occ(1, "tb_node")],
        vec![eq(col(1, "pk_node"), col(0, "fk_parent"), true, false)],
        col(0, "pk_node"),
    );
    assert_eq!(g.read_sets(&[(0, vec![])]), vec![]);
}
