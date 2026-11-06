//! A table read without a link to the key, through reads filtered by constants
//! (#284).

use super::{NONE, col, eq, graph, occ};
use crate::lineage::{Column, Sql, TableKind};

/// `(<column of occ> OPERATOR(pg_catalog.=) 'v'::text)`.
fn equals(occ: usize, attnum: i16, value: &str) -> Sql {
    let mut sql = Sql::text("(");
    sql.push_sql(
        Column {
            occ,
            attnum,
            name: String::new(),
        }
        .sql(),
    );
    sql.push_text(&format!(" OPERATOR(pg_catalog.=) '{value}'::text)"));
    sql
}

/// `tb_order o`; `tb_category` reached from the order's lines (occurrence 1, linked)
/// and in a subquery linked to nothing (occurrence 2).
fn orders_and_categories() -> crate::lineage::QueryGraph {
    let mut lookup = occ(3, "tb_category");
    lookup.in_sublink = true;
    graph(
        vec![occ(1, "tb_order"), occ(3, "tb_category"), lookup],
        vec![eq(col(1, "fk_order"), col(0, "pk_order"), true, false)],
        col(0, "pk_order"),
    )
}

#[test]
fn an_unlinked_read_filtered_by_constants_guards_its_table() {
    let mut g = orders_and_categories();
    g.guards
        .insert(2, vec![equals(2, 3, "default"), equals(2, 4, "even")]);
    let tables = g.tables(NONE);
    let category = tables.iter().find(|t| t.relid == 3).expect("tb_category");
    assert!(matches!(category.kind, TableKind::AllKeys(_)));
    assert_eq!(
        category.guard.as_deref(),
        Some(
            "SELECT 1::pg_catalog.int8 FROM pg_tviews_delta d WHERE \
             ((d.{c:3:3} OPERATOR(pg_catalog.=) 'default'::text) AND \
             (d.{c:3:4} OPERATOR(pg_catalog.=) 'even'::text))"
        )
    );
    // The linked read keeps its mapping.
    assert!(category.sql.is_some());
}

#[test]
fn an_unlinked_read_without_constant_filters_leaves_its_table_unguarded() {
    let tables = orders_and_categories().tables(NONE);
    let category = tables.iter().find(|t| t.relid == 3).expect("tb_category");
    assert!(matches!(category.kind, TableKind::AllKeys(_)));
    assert_eq!(category.guard, None);
}

#[test]
fn every_unlinked_read_needs_a_guard() {
    let mut g = orders_and_categories();
    let mut other = occ(3, "tb_category");
    other.in_sublink = true;
    g.occurrences.push(other);
    g.guards.insert(2, vec![equals(2, 3, "default")]);
    let tables = g.tables(NONE);
    let category = tables.iter().find(|t| t.relid == 3).expect("tb_category");
    assert_eq!(category.guard, None);
    // Both guarded: either read's conditions.
    g.guards.insert(3, vec![equals(3, 3, "other")]);
    let tables = g.tables(NONE);
    let category = tables.iter().find(|t| t.relid == 3).expect("tb_category");
    assert!(
        category
            .guard
            .as_deref()
            .is_some_and(|sql| sql.contains("'default'::text)) OR ((d.{c:3:3}"))
    );
}

#[test]
fn a_mapped_table_has_no_guard() {
    let mut g = orders_and_categories();
    g.occurrences.truncate(2);
    g.guards.insert(1, vec![equals(1, 3, "default")]);
    let tables = g.tables(NONE);
    let category = tables.iter().find(|t| t.relid == 3).expect("tb_category");
    assert!(!matches!(category.kind, TableKind::AllKeys(_)));
    assert_eq!(category.guard, None);
}
