use pgrx::prelude::*;
use pgrx::JsonB;
use std::sync::atomic::{AtomicBool, Ordering};

mod catalog;
mod refresh;
mod propagate;
mod utils;
mod hooks;
mod trigger;
pub mod error;
pub mod metadata;
pub mod schema;
pub mod parser;
pub mod ddl;
pub mod config;
pub mod dependency;

pub use error::{TViewError, TViewResult};

pg_module_magic!();

// Static cache for jsonb_ivm availability (performance optimization)
static JSONB_IVM_AVAILABLE: AtomicBool = AtomicBool::new(false);
static JSONB_IVM_CHECKED: AtomicBool = AtomicBool::new(false);

/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

/// Check if jsonb_ivm extension is available at runtime (cached)
/// Returns true if extension is installed, false otherwise
///
/// This function caches the result after the first check to avoid
/// repeated queries to pg_extension on every cascade operation.
pub fn check_jsonb_ivm_available() -> bool {
    // Return cached result if already checked
    if JSONB_IVM_CHECKED.load(Ordering::Relaxed) {
        return JSONB_IVM_AVAILABLE.load(Ordering::Relaxed);
    }

    // First time: query database
    let result: Result<bool, spi::Error> = Spi::connect(|client| {
        let rows = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_ivm')",
            None,
            None,
        )?;

        for row in rows {
            if let Some(exists) = row[1].value::<bool>()? {
                return Ok(exists);
            }
        }
        Ok(false)
    });

    let is_available = result.unwrap_or(false);

    // Cache result
    JSONB_IVM_AVAILABLE.store(is_available, Ordering::Relaxed);
    JSONB_IVM_CHECKED.store(true, Ordering::Relaxed);

    is_available
}

/// Export as SQL function for testing
#[pg_extern]
fn pg_tviews_check_jsonb_ivm() -> bool {
    check_jsonb_ivm_available()
}

/// Initialize the extension
/// Installs the ProcessUtility hook to intercept CREATE TABLE tv_* commands
#[pg_guard]
extern "C" fn _PG_init() {
    pgrx::log!("pg_tviews: _PG_init() called, installing ProcessUtility hook");

    // Install ProcessUtility hook
    unsafe {
        hooks::install_hook();
    }

    pgrx::log!("pg_tviews: ProcessUtility hook installed");

    // Check for jsonb_ivm extension
    if !check_jsonb_ivm_available() {
        warning!(
            "pg_tviews: jsonb_ivm extension not detected\n\
             → Performance: Basic (full document replacement)\n\
             → To enable 1.5-3× faster cascades, install jsonb_ivm:\n\
             → https://github.com/fraiseql/jsonb_ivm"
        );
    } else {
        info!("pg_tviews: jsonb_ivm detected - surgical JSONB updates enabled (1.5-3× faster)");
    }
}

/// Analyze a SELECT statement and return inferred TVIEW schema as JSONB
#[pg_extern]
fn pg_tviews_analyze_select(sql: &str) -> JsonB {
    match schema::inference::infer_schema(sql) {
        Ok(schema) => {
            match schema.to_jsonb() {
                Ok(jsonb) => jsonb,
                Err(e) => {
                    error!("Failed to serialize schema to JSONB: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Schema inference failed: {}", e);
        }
    }
}

/// Infer column types from PostgreSQL catalog
#[pg_extern]
fn pg_tviews_infer_types(
    table_name: &str,
    columns: Vec<String>,
) -> JsonB {
    match schema::types::infer_column_types(table_name, &columns) {
        Ok(types) => {
            match serde_json::to_value(&types) {
                Ok(json_value) => JsonB(json_value),
                Err(e) => {
                    error!("Failed to serialize types to JSONB: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Type inference failed: {}", e);
        }
    }
}

/// Cascade refresh when a base table row changes
/// Called by trigger handler when INSERT/UPDATE/DELETE occurs on base tables
///
/// Arguments:
/// - base_table_oid: OID of the base table that changed
/// - pk_value: Primary key value of the changed row
#[pg_extern]
fn pg_tviews_cascade(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // Find all TVIEWs that depend on this base table
    let dependent_tviews = match find_dependent_tviews(base_table_oid) {
        Ok(tv) => tv,
        Err(e) => error!("Failed to find dependent TVIEWs: {:?}", e),
    };

    if dependent_tviews.is_empty() {
        // No TVIEWs depend on this table
        info!("No dependent TVIEWs found for base table OID {:?}", base_table_oid);
        return;
    }

    info!("Base table OID {:?} changed (pk={}), refreshing {} dependent TVIEWs",
          base_table_oid, pk_value, dependent_tviews.len());

    // Refresh each dependent TVIEW
    for tview_meta in dependent_tviews {
        // Find rows in this TVIEW that reference the changed base table row
        let affected_rows = match find_affected_tview_rows(&tview_meta, base_table_oid, pk_value) {
            Ok(rows) => rows,
            Err(e) => {
                warning!("Failed to find affected rows in {}: {:?}", tview_meta.entity_name, e);
                continue;
            }
        };

        if affected_rows.is_empty() {
            continue;
        }

        info!("  Refreshing {} rows in TVIEW {}", affected_rows.len(), tview_meta.entity_name);

        // Refresh each affected row (this will cascade via propagate_from_row)
        for affected_pk in affected_rows {
            if let Err(e) = refresh::refresh_pk(tview_meta.view_oid, affected_pk) {
                warning!("Failed to refresh {}[{}]: {:?}", tview_meta.entity_name, affected_pk, e);
            }
        }
    }
}

/// Handle INSERT operations on base tables
/// Called by trigger handler when rows are inserted
///
/// Arguments:
/// - base_table_oid: OID of the base table that changed
/// - pk_value: Primary key value of the inserted row
#[pg_extern]
fn pg_tviews_insert(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // For INSERT operations, we need to check if this affects array relationships
    // For now, delegate to the cascade function (which handles recomputation)
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Handle DELETE operations on base tables
/// Called by trigger handler when rows are deleted
///
/// Arguments:
/// - base_table_oid: OID of the base table that changed
/// - pk_value: Primary key value of the deleted row
#[pg_extern]
fn pg_tviews_delete(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // For DELETE operations, we need to check if this affects array relationships
    // For now, delegate to the cascade function (which handles recomputation)
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Find all TVIEWs that have the given base table as a dependency
fn find_dependent_tviews(base_table_oid: pg_sys::Oid) -> spi::Result<Vec<catalog::TviewMeta>> {
    // Query pg_tview_meta using the pre-computed dependencies array
    // The dependencies column contains all base table OIDs that this TVIEW depends on

    let query = format!(
        "SELECT m.table_oid AS tview_oid, m.view_oid, m.entity, \
                m.fk_columns, m.uuid_fk_columns, \
                m.dependency_types, m.dependency_paths, m.array_match_keys \
         FROM pg_tview_meta m \
         WHERE {:?} = ANY(m.dependencies)",
        base_table_oid.as_u32()
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut result = Vec::new();

        for row in rows {
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            // Extract NEW arrays - dependency_types (TEXT[])
            let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
            let dep_types: Vec<catalog::DependencyType> = dep_types_raw
                .unwrap_or_default()
                .into_iter()
                .map(|s| catalog::DependencyType::from_str(&s))
                .collect();

            // dependency_paths (TEXT[][]) - array of arrays
            // TODO: pgrx doesn't support TEXT[][] extraction yet
            // For now, use empty default (Task 3 will populate these)
            let dep_paths: Vec<Option<Vec<String>>> = vec![];

            // array_match_keys (TEXT[]) with NULL values
            let array_keys: Option<Vec<Option<String>>> =
                row["array_match_keys"].value().unwrap_or(None);

            result.push(catalog::TviewMeta {
                tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                view_oid: row["view_oid"].value().unwrap().unwrap(),
                entity_name: row["entity"].value().unwrap().unwrap(),
                sync_mode: 's',
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                dependency_types: dep_types,
                dependency_paths: dep_paths,
                array_match_keys: array_keys.unwrap_or_default(),
            });
        }

        Ok(result)
    })
}

/// Find rows in a TVIEW that reference a specific base table row
fn find_affected_tview_rows(
    tview_meta: &catalog::TviewMeta,
    base_table_oid: pg_sys::Oid,
    base_pk: i64,
) -> spi::Result<Vec<i64>> {
    // Get the base table name to figure out which FK column to check
    let base_table_name = Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {:?}",
        base_table_oid
    ))?.ok_or(spi::Error::InvalidPosition)?;

    // Extract entity name from table name (e.g., "tb_user" -> "user")
    let base_entity = base_table_name.trim_start_matches("tb_");

    // Query the TVIEW's backing view to find rows where the PK matches
    let view_name = utils::lookup_view_for_source(tview_meta.view_oid)?;
    let tview_pk_col = format!("pk_{}", tview_meta.entity_name);

    // Determine if this is a direct match (tview entity = base entity) or FK relationship
    let where_clause = if tview_meta.entity_name == base_entity {
        // Direct match: the TVIEW is for this entity (e.g., tv_user depends on tb_user)
        // Match on the primary key column directly
        format!("{} = {}", tview_pk_col, base_pk)
    } else {
        // FK relationship: the TVIEW depends on this entity via FK
        // (e.g., tv_post depends on tb_user via fk_user)
        let fk_col = format!("fk_{}", base_entity);
        format!("{} = {}", fk_col, base_pk)
    };

    let query = format!(
        "SELECT {} FROM {} WHERE {}",
        tview_pk_col, view_name, where_clause
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[tview_pk_col.as_str()].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

/// This is where you could expose helper functions for debugging.
/// e.g., listing registered TVIEWs, dependencies, etc.

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use crate::TViewError;

    #[pg_test]
    fn sanity_check() {
        assert_eq!(2, 1 + 1);
    }

    #[pg_test]
    fn test_version_function() {
        let version = crate::pg_tviews_version();
        assert!(version.starts_with("0.1.0"));
    }

    #[pg_test]
    fn test_version_callable_from_sql() {
        let result = Spi::get_one::<String>(
            "SELECT pg_tviews_version()"
        );
        assert!(result.is_ok());
        let version = result.unwrap();
        assert!(version.is_some());
        assert!(version.unwrap().starts_with("0.1.0"));
    }

    #[pg_test]
    #[should_panic(expected = "TVIEW metadata not found")]
    fn test_error_propagates_to_postgres() {
        // This should raise a PostgreSQL error
        Err::<(), _>(TViewError::MetadataNotFound {
            entity: "test".to_string(),
        }).unwrap();
    }

    // Phase 5 Task 1 RED: Tests for jsonb_ivm detection
    #[pg_test]
    fn test_jsonb_ivm_check_function_exists() {
        // This test will fail because pg_tviews_check_jsonb_ivm doesn't exist yet
        let result = Spi::get_one::<bool>("SELECT pg_tviews_check_jsonb_ivm()");
        assert!(result.is_ok(), "pg_tviews_check_jsonb_ivm() function should exist");
    }

    #[pg_test]
    fn test_check_jsonb_ivm_available_function() {
        // This test will fail because check_jsonb_ivm_available() doesn't exist yet
        let _result = crate::check_jsonb_ivm_available();
        // Just calling it is enough - function must exist
    }

    #[pg_test]
    fn test_pg_tviews_works_without_jsonb_ivm() {
        // Setup: Ensure jsonb_ivm is NOT installed
        Spi::run("DROP EXTENSION IF EXISTS jsonb_ivm CASCADE").ok();

        // Test: pg_tviews should still function
        Spi::run("CREATE TABLE tb_demo (pk_demo INT PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_demo VALUES (1, 'Demo')").unwrap();

        // This should work even without jsonb_ivm
        let result = Spi::get_one::<bool>(
            "SELECT pg_tviews_create('demo', 'SELECT pk_demo, jsonb_build_object(''name'', name) AS data FROM tb_demo') IS NOT NULL"
        );

        assert!(result.unwrap().unwrap_or(false), "pg_tviews should work without jsonb_ivm");
    }
}

