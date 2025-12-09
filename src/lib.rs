use pgrx::prelude::*;
use pgrx::JsonB;

mod catalog;
mod trigger;
mod refresh;
mod propagate;
mod utils;
mod hooks;
pub mod error;
pub mod metadata;
pub mod schema;
pub mod parser;
pub mod ddl;
pub mod config;
pub mod dependency;

pub use error::{TViewError, TViewResult};

pg_module_magic!();

/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
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

/// Find all TVIEWs that have the given base table as a dependency
fn find_dependent_tviews(base_table_oid: pg_sys::Oid) -> spi::Result<Vec<catalog::TviewMeta>> {
    // Query pg_tview_meta using the pre-computed dependencies array
    // The dependencies column contains all base table OIDs that this TVIEW depends on

    let query = format!(
        "SELECT m.table_oid AS tview_oid, m.view_oid, m.entity, m.fk_columns, m.uuid_fk_columns
         FROM pg_tview_meta m
         WHERE {:?} = ANY(m.dependencies)",
        base_table_oid.as_u32()
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut result = Vec::new();

        for row in rows {
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            result.push(catalog::TviewMeta {
                tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                view_oid: row["view_oid"].value().unwrap().unwrap(),
                entity_name: row["entity"].value().unwrap().unwrap(),
                sync_mode: 's', // Default to synchronous
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
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
    ))?.ok_or_else(|| spi::Error::InvalidPosition)?;

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
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn sanity_check() {
        assert_eq!(1 + 1, 2);
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
}

