use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;
use std::collections::HashSet;

/// Find all base tables that a view depends on (transitively).
///
/// `schema_hint` is the schema where the view was created.  When `Some`, the OID
/// lookup is constrained to that schema so the correct view is found even when
/// `current_schema()` resolves to a different schema (e.g. the database
/// `search_path` starts with `"app"` but the view lives in `"public"`).
/// When `None`, the lookup falls back to `current_schema()`.
///
/// The view's rewrite rule is followed through `pg_depend`, through views, at most
/// `pg_tviews.max_dependency_depth` views deep.
///
/// # Errors
/// Returns error if circular dependency detected, depth limit exceeded, or OID lookup fails
pub fn find_base_tables(
    view_name: &str,
    schema_hint: Option<&str>,
) -> TViewResult<Vec<pg_sys::Oid>> {
    let view_oid = get_view_oid(view_name, schema_hint)?;
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
            // which fires no trigger: the lineage classifies it `all_keys`, and the
            // TVIEW's uncascaded_policy decides.
            b'm' => Some(oid),
            _ => None,
        })
        .collect())
}

// Helper functions for find_base_tables()

fn get_view_oid(view_name: &str, schema_hint: Option<&str>) -> TViewResult<pg_sys::Oid> {
    // Use pg_class lookup rather than ::regclass cast: the cast raises a PostgreSQL
    // ERROR (aborting the transaction) when the view doesn't exist, whereas a catalog
    // query returns NULL which we can handle as a Rust error.
    //
    // When a schema_hint is provided (e.g. "public" from a schema-qualified CREATE TABLE)
    // we constrain the lookup to that schema.  Otherwise we fall back to current_schema()
    // so existing callers that don't know the schema still work.
    let schema = schema_hint.map_or_else(
        || {
            Spi::get_one::<String>("SELECT current_schema()::text")
                .map_err(|e| TViewError::CatalogError {
                    operation: "Resolve current_schema()".to_string(),
                    pg_error: e.to_string(),
                })
                .and_then(|opt| {
                    opt.ok_or_else(|| TViewError::CatalogError {
                        operation: "Resolve current_schema()".to_string(),
                        pg_error: "current_schema() returned NULL".to_string(),
                    })
                })
        },
        |s| Ok(s.to_string()),
    )?;

    let args = vec![
        crate::utils::spi::text(view_name),
        crate::utils::spi::text(schema.as_str()),
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
