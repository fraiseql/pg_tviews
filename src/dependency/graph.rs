use crate::error::{TViewError, TViewResult};
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone)]
struct DependencyNode {
    oid: pg_sys::Oid,
    relkind: Option<String>,
}

/// Find all base tables that a view depends on (transitively).
///
/// `schema_hint` is the schema where the view was created.  When `Some`, the OID
/// lookup is constrained to that schema so the correct view is found even when
/// `current_schema()` resolves to a different schema (e.g. the database
/// `search_path` starts with `"app"` but the view lives in `"public"`).
/// When `None`, the lookup falls back to `current_schema()`.
///
/// ALGORITHM:
/// 1. Start from view OID
/// 2. Query `pg_depend` WHERE `objid` = `current_oid` (objects THIS depends on)
/// 3. For each dependency:
///    - If it's a table (`relkind='r'`), add to `base_tables`
///    - If it's a view (`relkind='v'`), recurse
/// 4. Track visited to detect cycles
/// 5. Enforce `pg_tviews.max_dependency_depth`
///
/// # Errors
/// Returns error if circular dependency detected, depth limit exceeded, or OID lookup fails
pub fn find_base_tables(
    view_name: &str,
    schema_hint: Option<&str>,
) -> TViewResult<Vec<pg_sys::Oid>> {
    let view_oid = get_view_oid(view_name, schema_hint)?;
    let dependencies = traverse_dependencies(view_oid)?;
    let tview_oids = load_tview_table_oids()?;
    Ok(filter_base_tables(&dependencies, &tview_oids))
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
                    pg_error: format!("{e:?}"),
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

    // SAFETY: DatumWithOid::new wraps PostgreSQL datum pointers for SPI parameter passing.
    // The view/schema names are validated before this call.
    let args = vec![
        unsafe { DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        unsafe {
            DatumWithOid::new(
                schema.as_str(),
                PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
            )
        },
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
        pg_error: format!("{e:?}"),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: view_name.to_string(),
        reason: format!("Object not found in schema '{schema}'"),
    })
}

fn traverse_dependencies(view_oid: pg_sys::Oid) -> TViewResult<Vec<DependencyNode>> {
    let mut all_dependencies = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back((view_oid, 0));

    let max_dependency_depth = crate::config::max_dependency_depth();
    while let Some((current_oid, depth)) = queue.pop_front() {
        // Check depth limit (tunable via pg_tviews.max_dependency_depth)
        if depth > max_dependency_depth {
            return Err(TViewError::DependencyDepthExceeded {
                depth,
                max_depth: max_dependency_depth,
            });
        }

        // Check for cycles
        if visiting.contains(&current_oid) {
            let cycle = reconstruct_cycle(&visiting, current_oid);
            return Err(TViewError::CircularDependency { cycle });
        }

        // Skip if already visited
        if visited.contains(&current_oid) {
            continue;
        }

        visiting.insert(current_oid);

        // Query dependencies
        let deps = query_dependencies(current_oid)?;

        // Process each dependency
        for (dep_oid, relkind_opt) in deps {
            // Add to all dependencies
            all_dependencies.push(DependencyNode {
                oid: dep_oid,
                relkind: relkind_opt.clone(),
            });

            if relkind_opt.as_deref() == Some("v") {
                // View - recurse
                queue.push_back((dep_oid, depth + 1));
            }
            // Base tables and others handled later
        }

        visiting.remove(&current_oid);
        visited.insert(current_oid);
    }

    Ok(all_dependencies)
}

fn query_dependencies(current_oid: pg_sys::Oid) -> TViewResult<Vec<(pg_sys::Oid, Option<String>)>> {
    let deps_query = format!(
        "SELECT DISTINCT d.refobjid, c.relkind
         FROM pg_rewrite r
         JOIN pg_depend d ON d.objid = r.oid AND d.classid = 'pg_rewrite'::regclass::oid
          LEFT JOIN pg_class c ON d.refobjid = c.oid AND d.refclassid = 'pg_class'::regclass::oid
          WHERE r.ev_class = {current_oid:?}
           AND d.refclassid = 'pg_class'::regclass::oid
           AND c.oid != {current_oid:?}"
    );

    let deps = Spi::connect(|client| {
        let rows = client.select(&deps_query, None, &[])?;
        let mut results = Vec::new();

        for row in rows {
            let refobjid = row["refobjid"]
                .value::<pg_sys::Oid>()
                .map_err(|e| TViewError::CatalogError {
                    operation: "Extract refobjid".to_string(),
                    pg_error: format!("{e:?}"),
                })?
                .ok_or_else(|| TViewError::CatalogError {
                    operation: "Extract refobjid".to_string(),
                    pg_error: "NULL OID in pg_depend".to_string(),
                })?;

            #[allow(clippy::cast_sign_loss)]
            // Reason: pg_class.relkind is stored as i8, cast to u8 for char
            let relkind = row["relkind"]
                .value::<i8>()
                .map_err(|e| TViewError::CatalogError {
                    operation: "Extract relkind".to_string(),
                    pg_error: format!("{e:?}"),
                })?
                .map(|c| (c as u8 as char).to_string());

            results.push((refobjid, relkind));
        }

        Ok(Some(results))
    })
    .map_err(|e: pgrx::spi::Error| TViewError::SpiError {
        query: deps_query.clone(),
        error: e.to_string(),
    })?
    .unwrap_or_default();

    Ok(deps)
}

/// Load OIDs of all TVIEW-managed tables from `pg_tview_meta`.
///
/// These are the `tv_*` tables that `pg_tviews` owns. They must NOT be treated
/// as base tables for trigger installation — cascade is metadata-driven via
/// `find_parents_for()`, not trigger-driven.
fn load_tview_table_oids() -> TViewResult<HashSet<pg_sys::Oid>> {
    Spi::connect(|client| {
        let rows = client.select(
            &format!(
                "SELECT table_oid::oid AS table_oid FROM {}",
                crate::utils::meta_table()
            ),
            None,
            &[],
        )?;
        let mut oids = HashSet::new();
        for row in rows {
            if let Some(oid) =
                row["table_oid"]
                    .value::<pg_sys::Oid>()
                    .map_err(|e| TViewError::CatalogError {
                        operation: "load_tview_table_oids".to_string(),
                        pg_error: format!("{e:?}"),
                    })?
            {
                oids.insert(oid);
            }
        }
        Ok(oids)
    })
    .map_err(|e: pgrx::spi::Error| TViewError::CatalogError {
        operation: "load_tview_table_oids".to_string(),
        pg_error: format!("{e:?}"),
    })
}

fn filter_base_tables(
    dependencies: &[DependencyNode],
    tview_oids: &HashSet<pg_sys::Oid>,
) -> Vec<pg_sys::Oid> {
    let mut base_tables = HashSet::new();

    for dep in dependencies {
        if let Some(relkind) = &dep.relkind {
            match relkind.as_str() {
                "r" | "p" => {
                    // Skip TVIEW-managed tables — cascade is metadata-driven
                    if tview_oids.contains(&dep.oid) {
                        continue;
                    }
                    base_tables.insert(dep.oid);
                }
                "m" => {
                    // A materialized view's rows change only by REFRESH MATERIALIZED
                    // VIEW, which fires no trigger: the lineage classifies it
                    // `all_keys`, and the TVIEW's uncascaded_policy decides (#189).
                    base_tables.insert(dep.oid);
                }
                _ => {
                    // Views and other object types: not base tables
                }
            }
        }
    }

    base_tables.into_iter().collect()
}

fn reconstruct_cycle(visiting: &HashSet<pg_sys::Oid>, current: pg_sys::Oid) -> Vec<String> {
    // Simple cycle representation: just list the OIDs in visiting set + current
    visiting
        .iter()
        .chain(std::iter::once(&current))
        .filter_map(|oid| get_object_name(*oid).ok())
        .collect()
}

fn get_object_name(oid: pg_sys::Oid) -> TViewResult<String> {
    crate::utils::spi_get_string(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {oid:?}"
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get name for OID {oid:?}"),
        pg_error: format!("{e:?}"),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {oid:?}"),
        reason: "Object not found".to_string(),
    })
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_find_base_tables_single() {
        // Create base table
        Spi::run("CREATE TABLE tb_test (pk INTEGER PRIMARY KEY, id UUID, data JSONB)").unwrap();

        // Create view
        Spi::run("CREATE VIEW v_test AS SELECT * FROM tb_test").unwrap();

        // Find dependencies
        let base_tables = find_base_tables("v_test", None).unwrap();

        assert_eq!(base_tables.len(), 1);

        let table_name = get_object_name(base_tables[0]).unwrap();
        assert_eq!(table_name, "tb_test");
    }

    #[pg_test]
    fn test_find_base_tables_transitive() {
        // Create base tables
        Spi::run("CREATE TABLE tb_user (pk INTEGER PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (pk INTEGER PRIMARY KEY, fk_user INTEGER, title TEXT)")
            .unwrap();

        // Create helper view
        Spi::run("CREATE VIEW v_user AS SELECT * FROM tb_user").unwrap();

        // Create composite view
        Spi::run(
            "CREATE VIEW v_post AS
            SELECT p.*, u.name FROM tb_post p JOIN v_user u ON u.pk = p.fk_user
        ",
        )
        .unwrap();

        // Find dependencies
        let base_tables = find_base_tables("v_post", None).unwrap();

        // Should find both tb_user and tb_post
        assert_eq!(base_tables.len(), 2);

        let names: Vec<String> = base_tables
            .iter()
            .map(|&oid| get_object_name(oid).unwrap())
            .collect();

        assert!(names.contains(&"tb_user".to_string()));
        assert!(names.contains(&"tb_post".to_string()));
    }
}
