use pgrx::prelude::*;
use pgrx::JsonB;

mod catalog;
mod trigger;
mod refresh;
mod propagate;
mod utils;
pub mod error;
pub mod metadata;
pub mod schema;
pub mod parser;
pub mod hooks;
pub mod ddl;

pub use error::{TViewError, TViewResult};

pg_module_magic!();

/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

/// Initialize the extension - create metadata tables and install hooks
#[pg_guard]
extern "C" fn _PG_init() {
    // Create metadata tables on extension load
    if let Err(e) = metadata::create_metadata_tables() {
        pgrx::error!("Failed to initialize pg_tviews metadata: {}", e);
    }

    // Install ProcessUtility hook for CREATE/DROP TVIEW
    hooks::install_hooks();
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

