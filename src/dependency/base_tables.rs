use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;
use std::collections::HashSet;

/// Find all base tables that a view depends on (transitively).
///
/// `schema` is the schema where the view was created: the lookup is constrained
/// to it, so the right view is found whatever `search_path` says.
///
/// The view's rewrite rule is followed through `pg_depend`, through views, at most
/// `pg_tviews.max_dependency_depth` views deep.
///
/// # Errors
/// Returns error if circular dependency detected, depth limit exceeded, or OID lookup fails
pub fn find_base_tables(view_name: &str, schema: &str) -> TViewResult<Vec<pg_sys::Oid>> {
    let view_oid = get_view_oid(view_name, schema)?;
    let max_depth = crate::config::max_dependency_depth();
    let depth = crate::catalog::reads::view_nesting(view_oid)?;
    if depth > max_depth {
        return Err(TViewError::DepthExceeded {
            what: "dependency",
            depth,
            max_depth,
        });
    }
    let tview_tables = load_tview_table_oids()?;
    Ok(crate::catalog::reads::view_relations_read(view_oid)?
        .into_iter()
        .filter_map(|(oid, relkind)| match relkind {
            // A TVIEW's table is followed through the TVIEW's own refresh.
            b'r' | b'p' if !tview_tables.contains(&oid) => Some(oid),
            // A materialized view's rows change only by REFRESH MATERIALIZED VIEW,
            // a foreign table's on its server, a sequence's by nextval(): no trigger
            // sees them, the lineage classifies them `all_keys`, and the TVIEW's
            // uncascaded_policy decides.
            b'm' | b'f' | b'S' => Some(oid),
            _ => None,
        })
        .collect())
}

// Helper functions for find_base_tables()

fn get_view_oid(view_name: &str, schema: &str) -> TViewResult<pg_sys::Oid> {
    // Use pg_class lookup rather than ::regclass cast: the cast raises a PostgreSQL
    // ERROR (aborting the transaction) when the view doesn't exist, whereas a catalog
    // query returns NULL which we can handle as a Rust error.
    let args = vec![
        crate::utils::spi::text(view_name),
        crate::utils::spi::text(schema),
    ];
    Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT c.oid FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 \
           AND n.nspname = $2 \
           AND c.relkind IN ('v', 'm')",
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for '{view_name}'"),
        pg_error: e.to_string(),
    })?
    .ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for '{view_name}'"),
        pg_error: format!("not found in schema '{schema}'"),
    })
}

/// Load OIDs of all TVIEW-managed tables from `pg_tview_meta`.
///
/// These are the `tv_*` tables that `pg_tviews` owns. They must NOT be treated
/// as base tables for trigger installation — cascade is metadata-driven via
/// `find_parents_for()`, not trigger-driven.
fn load_tview_table_oids() -> TViewResult<HashSet<pg_sys::Oid>> {
    crate::catalog::row::table_oids()
}
