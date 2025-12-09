use pgrx::prelude::*;

mod catalog;
mod trigger;
mod refresh;
mod propagate;
mod util;
pub mod error;
pub mod metadata;

pub use error::{TViewError, TViewResult};

pg_module_magic!();

/// Get the version of the pg_tviews extension
#[pg_extern]
fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

/// Initialize the extension - create metadata tables
#[pg_guard]
extern "C" fn _PG_init() {
    // Create metadata tables on extension load
    if let Err(e) = metadata::create_metadata_tables() {
        pgrx::error!("Failed to initialize pg_tviews metadata: {}", e);
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
    #[should_panic(expected = "TVIEW metadata not found")]
    fn test_error_propagates_to_postgres() {
        // This should raise a PostgreSQL error
        Err::<(), _>(TViewError::MetadataNotFound {
            entity: "test".to_string(),
        }).unwrap();
    }
}

