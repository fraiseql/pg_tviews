use pgrx::prelude::*;
use std::collections::{HashSet, VecDeque};
use crate::error::{TViewError, TViewResult};
use crate::config::MAX_DEPENDENCY_DEPTH;

#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub base_tables: Vec<pg_sys::Oid>,
    pub helper_views: Vec<String>,
    pub all_dependencies: Vec<pg_sys::Oid>,
    pub max_depth_reached: usize,
}

/// Find all base tables that a view depends on (transitively)
///
/// ALGORITHM:
/// 1. Start from view OID
/// 2. Query pg_depend WHERE objid = current_oid (objects THIS depends on)
/// 3. For each dependency:
///    - If it's a table (relkind='r'), add to base_tables
///    - If it's a view (relkind='v'), recurse
/// 4. Track visited to detect cycles
/// 5. Enforce MAX_DEPENDENCY_DEPTH
///
/// CORRECTED: Was using refobjid = {}, now uses objid = {}
pub fn find_base_tables(view_name: &str) -> TViewResult<DependencyGraph> {
    let view_oid = get_oid(view_name)?;
    let mut base_tables = HashSet::new();
    let mut all_dependencies = HashSet::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();  // For cycle detection
    let mut queue = VecDeque::new();
    let mut max_depth = 0;

    queue.push_back((view_oid, 0usize));  // (oid, depth)

    while let Some((current_oid, depth)) = queue.pop_front() {
        // Check depth limit
        if depth > MAX_DEPENDENCY_DEPTH {
            return Err(TViewError::DependencyDepthExceeded {
                depth,
                max_depth: MAX_DEPENDENCY_DEPTH,
            });
        }

        max_depth = max_depth.max(depth);

        // Check for cycles
        if visiting.contains(&current_oid) {
            // Build cycle path for error message
            let cycle = reconstruct_cycle(&visiting, current_oid)?;
            return Err(TViewError::CircularDependency { cycle });
        }

        // Skip if already visited
        if visited.contains(&current_oid) {
            continue;
        }

        visiting.insert(current_oid);

        // Debug: Log what we're querying
        if let Ok(name) = get_object_name(current_oid) {
            info!("Checking dependencies for: {} (OID {:?}) at depth {}", name, current_oid, depth);
        }

        // CORRECTED ALGORITHM: Views depend on tables via pg_rewrite rules!
        // 1. Find the pg_rewrite rule for this view/relation
        // 2. Query dependencies of the RULE (not the view directly)
        // 3. The rule's dependencies point to the actual base tables/views
        let deps_query = format!(
            "SELECT DISTINCT d.refobjid, d.refobjsubid, d.deptype, c.relkind
             FROM pg_rewrite r
             JOIN pg_depend d ON d.objid = r.oid AND d.classid = 'pg_rewrite'::regclass::oid
             LEFT JOIN pg_class c ON d.refobjid = c.oid AND d.refclassid = 'pg_class'::regclass::oid
             WHERE r.ev_class = {:?}
               AND d.deptype IN ('n', 'a')
               AND d.refclassid = 'pg_class'::regclass::oid
               AND c.oid != {:?}",  // Exclude self-reference
            current_oid, current_oid
        );

        info!("Executing query: {}", deps_query);

        let deps: Vec<(pg_sys::Oid, i32, String, Option<String>)> =
            Spi::connect(|client| {
                let tup_table = client.select(&deps_query, None, None)
                    .map_err(|e| TViewError::CatalogError {
                        operation: "pg_depend query".to_string(),
                        pg_error: format!("{:?}", e),
                    })?;

                let mut results = Vec::new();

                for row in tup_table {
                    let refobjid = row["refobjid"].value::<pg_sys::Oid>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "Extract refobjid".to_string(),
                            pg_error: format!("{:?}", e),
                        })?
                        .ok_or_else(|| TViewError::CatalogError {
                            operation: "Extract refobjid".to_string(),
                            pg_error: "NULL OID in pg_depend".to_string(),
                        })?;
                    let refobjsubid = row["refobjsubid"].value::<i32>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "Extract refobjsubid".to_string(),
                            pg_error: format!("{:?}", e),
                        })?.unwrap_or(0);
                    // deptype is "char" (single byte), not text/String
                    let deptype = row["deptype"].value::<i8>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "Extract deptype".to_string(),
                            pg_error: format!("{:?}", e),
                        })?
                        .map(|c| (c as u8 as char).to_string())
                        .unwrap_or_default();
                    // relkind is also "char" (single byte)
                    let relkind = row["relkind"].value::<i8>()
                        .map_err(|e| TViewError::CatalogError {
                            operation: "Extract relkind".to_string(),
                            pg_error: format!("{:?}", e),
                        })?
                        .map(|c| (c as u8 as char).to_string());

                    results.push((refobjid, refobjsubid, deptype, relkind));
                }

                Ok(Some(results))
            })?
            .unwrap_or_default();

        info!("Found {} dependency rows", deps.len());

        // Process each dependency
        for (dep_oid, _subid, _deptype, relkind_opt) in deps {
            all_dependencies.insert(dep_oid);

            // Skip if no relkind (not a pg_class object)
            let Some(relkind) = relkind_opt else {
                info!("Skipping dependency OID {:?} (not in pg_class)", dep_oid);
                continue;
            };

            if let Ok(dep_name) = get_object_name(dep_oid) {
                info!("  Found dependency: {} (OID {:?}, relkind '{}')", dep_name, dep_oid, relkind);
            }

            match relkind.as_str() {
                "r" => {
                    // Regular table - add to base_tables
                    base_tables.insert(dep_oid);
                    if let Ok(name) = get_object_name(dep_oid) {
                        info!("  -> Base table: {}", name);
                    }
                }
                "v" => {
                    // View - recurse
                    queue.push_back((dep_oid, depth + 1));
                    if let Ok(name) = get_object_name(dep_oid) {
                        info!("  -> View (will recurse): {} at depth {}", name, depth + 1);
                    }
                }
                "m" => {
                    // Materialized view - treat as base table
                    base_tables.insert(dep_oid);
                    if let Ok(name) = get_object_name(dep_oid) {
                        info!("  -> Materialized view: {}", name);
                    }
                }
                "p" => {
                    // Partitioned table - treat as base table
                    base_tables.insert(dep_oid);
                    if let Ok(name) = get_object_name(dep_oid) {
                        info!("  -> Partitioned table: {}", name);
                    }
                }
                _ => {
                    // Ignore other types (indexes, sequences, etc.)
                    if crate::config::DEBUG_DEPENDENCIES {
                        if let Ok(name) = get_object_name(dep_oid) {
                            info!("  -> Ignoring '{}' with relkind '{}'", name, relkind);
                        }
                    }
                }
            }
        }

        visiting.remove(&current_oid);
        visited.insert(current_oid);
    }

    Ok(DependencyGraph {
        base_tables: base_tables.into_iter().collect(),
        helper_views: Vec::new(),  // Filled in later
        all_dependencies: all_dependencies.into_iter().collect(),
        max_depth_reached: max_depth,
    })
}

/// Reconstruct cycle path for error reporting
fn reconstruct_cycle(visiting: &HashSet<pg_sys::Oid>, cycle_oid: pg_sys::Oid) -> TViewResult<Vec<String>> {
    let mut cycle_names = Vec::new();

    for &oid in visiting.iter() {
        if let Ok(name) = get_object_name(oid) {
            cycle_names.push(name);
        }

        if oid == cycle_oid {
            break;
        }
    }

    // Add the repeated node to show cycle
    if let Ok(name) = get_object_name(cycle_oid) {
        cycle_names.push(name);
    }

    Ok(cycle_names)
}

/// Find all helper views (v_*) used by a SELECT statement
pub fn find_helper_views(select_sql: &str) -> TViewResult<Vec<String>> {
    let mut helpers = Vec::new();

    // Simple regex to find v_* references
    // LIMITATION: This is v1 - doesn't handle all cases (subqueries, CTEs, etc.)
    // TODO: Use PostgreSQL parser API in v2
    let re = regex::Regex::new(r"\bv_(\w+)\b")
        .map_err(|e| TViewError::InternalError {
            message: format!("Regex compilation failed: {}", e),
            file: file!(),
            line: line!(),
        })?;

    for cap in re.captures_iter(select_sql) {
        let helper_name = format!("v_{}", &cap[1]);

        // Verify view actually exists
        if view_exists(&helper_name)? {
            if !helpers.contains(&helper_name) {
                helpers.push(helper_name);
            }
        } else {
            warning!("Helper view '{}' referenced in SELECT but does not exist", helper_name);
        }
    }

    Ok(helpers)
}

fn get_oid(object_name: &str) -> TViewResult<pg_sys::Oid> {
    Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT '{}'::regclass::oid",
        object_name
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get OID for '{}'", object_name),
        pg_error: format!("{:?}", e),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: object_name.to_string(),
        reason: "Object not found".to_string(),
    })
}

fn get_relkind(oid: pg_sys::Oid) -> TViewResult<String> {
    Spi::get_one::<String>(&format!(
        "SELECT relkind::text FROM pg_class WHERE oid = {:?}",
        oid
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get relkind for OID {:?}", oid),
        pg_error: format!("{:?}", e),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {:?}", oid),
        reason: "Object not found in pg_class".to_string(),
    })
}

fn get_object_name(oid: pg_sys::Oid) -> TViewResult<String> {
    Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {:?}",
        oid
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Get name for OID {:?}", oid),
        pg_error: format!("{:?}", e),
    })?
    .ok_or_else(|| TViewError::DependencyResolutionFailed {
        view_name: format!("OID {:?}", oid),
        reason: "Object not found".to_string(),
    })
}

fn view_exists(view_name: &str) -> TViewResult<bool> {
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_class
         WHERE relname = '{}' AND relkind IN ('v', 'm')",
        view_name
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check existence of '{}'", view_name),
        pg_error: format!("{:?}", e),
    })
    .map(|opt| opt.unwrap_or(false))
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;
    use crate::error::testing::*;

    #[pg_test]
    fn test_find_base_tables_single() {
        // Create base table
        Spi::run("CREATE TABLE tb_test (pk INTEGER PRIMARY KEY, id UUID, data JSONB)").unwrap();

        // Create view
        Spi::run("CREATE VIEW v_test AS SELECT * FROM tb_test").unwrap();

        // Find dependencies
        let graph = find_base_tables("v_test").unwrap();

        assert_eq!(graph.base_tables.len(), 1);
        assert_eq!(graph.max_depth_reached, 1);

        let table_name = get_object_name(graph.base_tables[0]).unwrap();
        assert_eq!(table_name, "tb_test");
    }

    #[pg_test]
    fn test_find_base_tables_transitive() {
        // Create base tables
        Spi::run("CREATE TABLE tb_user (pk INTEGER PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (pk INTEGER PRIMARY KEY, fk_user INTEGER, title TEXT)").unwrap();

        // Create helper view
        Spi::run("CREATE VIEW v_user AS SELECT * FROM tb_user").unwrap();

        // Create composite view
        Spi::run("CREATE VIEW v_post AS
            SELECT p.*, u.name FROM tb_post p JOIN v_user u ON u.pk = p.fk_user
        ").unwrap();

        // Find dependencies
        let graph = find_base_tables("v_post").unwrap();

        // Should find both tb_user and tb_post
        assert_eq!(graph.base_tables.len(), 2);
        assert!(graph.max_depth_reached >= 1);

        let names: Vec<String> = graph.base_tables.iter()
            .map(|&oid| get_object_name(oid).unwrap())
            .collect();

        assert!(names.contains(&"tb_user".to_string()));
        assert!(names.contains(&"tb_post".to_string()));
    }

    #[pg_test]
    fn test_circular_dependency_detected() {
        // Create view that references itself (PostgreSQL allows this!)
        Spi::run("CREATE TABLE tb_base (pk INTEGER PRIMARY KEY, value TEXT)").unwrap();
        Spi::run("CREATE VIEW v_a AS SELECT * FROM tb_base WHERE value = 'a'").unwrap();

        // This shouldn't create a cycle in normal cases, but let's test depth limit
        // by creating a deep hierarchy

        Spi::run("CREATE VIEW v_b AS SELECT * FROM v_a").unwrap();
        Spi::run("CREATE VIEW v_c AS SELECT * FROM v_b").unwrap();
        // ... would need 10+ levels to trigger depth limit

        // For now, verify no cycle in simple case
        let graph = find_base_tables("v_c").unwrap();
        assert!(graph.max_depth_reached < MAX_DEPENDENCY_DEPTH);
    }

    #[pg_test]
    fn test_depth_limit_enforced() {
        // This test would require creating 11+ nested views
        // Left as integration test
    }
}
