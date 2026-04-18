/*!
# `pg_tviews` - `PostgreSQL` Transactional Views

A `PostgreSQL` extension that provides transactional materialized views with
incremental refresh capabilities. `TVIEW`s automatically maintain consistency
between base tables and derived views through trigger-based change tracking.

## Architecture

`pg_tviews` implements a sophisticated refresh system:

1. **Change Tracking**: Triggers on base tables enqueue changes to a transaction-scoped queue
2. **Dependency Analysis**: Resolves view dependencies using topological sorting
3. **Incremental Refresh**: Updates only affected rows in dependent views
4. **Transaction Safety**: All refreshes occur within the same transaction as the original changes

## Key Features

- **Transactional Consistency**: View refreshes are atomic with base table changes
- **Dependency Resolution**: Handles complex multi-level view dependencies
- **Performance Optimized**: Incremental updates avoid full view rebuilds
- **`PostgreSQL` Native**: Written as a C extension using `pgrx` framework

## Safety

- No panics in FFI callbacks (all wrapped in `catch_unwind`)
- Transaction rollback on refresh failures
- Memory safety through Rust's ownership system
*/

use pgrx::prelude::*;

// Core modules
mod audit;
mod cascade_path;
mod catalog;
mod event_trigger;
mod hooks;
mod metrics;
mod propagate;
mod queue;
mod refresh;
mod sql_parser;
mod trigger;
mod utils;

// Feature modules
mod admin;
mod cascade;
mod health;
mod lifecycle;

// Public API modules
pub mod config;
pub mod ddl;
pub mod dependency;
pub mod error;
pub mod metadata;
pub mod parser;
pub mod schema;
pub mod validation;

// Public re-exports
pub use catalog::entity_for_table;
pub use error::{TViewError, TViewResult};
pub use lifecycle::check_jsonb_delta_available;
pub use queue::RefreshKey;

pg_module_magic!();

#[cfg(any(test, feature = "pg_test"))]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Vec allocation is not const-stable
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec![]
    }
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use crate::error::TViewError;
    use pgrx::prelude::*;

    #[pg_test]
    fn sanity_check() {
        let two: i32 = 2;
        assert_eq!(two, 1 + 1);
    }

    #[pg_test]
    fn test_version_function() {
        let version = Spi::get_one::<String>("SELECT pg_tviews_version()")
            .unwrap()
            .unwrap();
        assert!(version.starts_with("0.1.0"));
    }

    #[pg_test]
    fn test_version_callable_from_sql() {
        let result = crate::utils::spi_get_string("SELECT pg_tviews_version()");
        assert!(result.is_ok());
        let version = result.unwrap();
        assert!(version.is_some());
        assert!(version.unwrap().starts_with("0.1.0"));
    }

    #[pg_test]
    #[should_panic(expected = "TVIEW metadata not found")]
    fn test_error_propagates_to_postgres() {
        panic!(
            "{:?}",
            TViewError::MetadataNotFound {
                entity: "test".to_string(),
            }
        );
    }

    #[pg_test]
    fn test_jsonb_delta_check_function_exists() {
        let result = Spi::get_one::<bool>("SELECT pg_tviews_check_jsonb_delta()");
        assert!(
            result.is_ok(),
            "pg_tviews_check_jsonb_delta() function should exist"
        );
    }

    #[pg_test]
    fn test_check_jsonb_delta_available_function() {
        let _result = crate::check_jsonb_delta_available();
    }

    #[pg_test]
    fn test_pg_tviews_works_without_jsonb_delta() {
        Spi::run("DROP EXTENSION IF EXISTS jsonb_delta CASCADE").ok();

        Spi::run("CREATE TABLE tb_demo (pk_demo INT PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_demo VALUES (1, 'Demo')").unwrap();

        let result = Spi::get_one::<bool>(
            "SELECT pg_tviews_create('demo', 'SELECT pk_demo, jsonb_build_object(''name'', name) AS data FROM tb_demo') IS NOT NULL",
        );

        assert!(
            result.unwrap().unwrap_or(false),
            "pg_tviews should work without jsonb_delta"
        );
    }

    #[pg_test]
    fn test_pg_tviews_refresh_no_column_mismatch() {
        Spi::run("CREATE TABLE tb_note (pk_note BIGSERIAL PRIMARY KEY, body TEXT)").unwrap();
        Spi::run("INSERT INTO tb_note VALUES (1, 'hello'), (2, 'world')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('note', $$
            SELECT pk_note, jsonb_build_object('body', body) AS data
            FROM tb_note
        $$)",
        )
        .unwrap();

        let result = Spi::run("SELECT pg_tviews_refresh('note')");
        assert!(
            result.is_ok(),
            "pg_tviews_refresh failed: {:?}",
            result.err()
        );

        let count = Spi::get_one::<i64>("SELECT COUNT(*) FROM tv_note")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, 2, "all rows should survive the full refresh");
    }

    #[pg_test]
    fn test_pg_tviews_refresh_repopulates_data() {
        Spi::run("CREATE TABLE tb_tag (pk_tag BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_tag VALUES (1, 'rust')").unwrap();

        Spi::run(
            "SELECT pg_tviews_create('tag', $$
            SELECT pk_tag, jsonb_build_object('name', name) AS data
            FROM tb_tag
        $$)",
        )
        .unwrap();

        Spi::run("UPDATE tv_tag SET data = '{}'::jsonb WHERE pk_tag = 1").unwrap();

        let stale = Spi::get_one::<pgrx::JsonB>("SELECT data FROM tv_tag WHERE pk_tag = 1")
            .unwrap()
            .unwrap();
        assert_eq!(
            stale.0,
            serde_json::json!({}),
            "data should be corrupted before refresh"
        );

        Spi::run("SELECT pg_tviews_refresh('tag')").unwrap();

        let restored = Spi::get_one::<pgrx::JsonB>("SELECT data FROM tv_tag WHERE pk_tag = 1")
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.0["name"], "rust",
            "refresh should restore data from the backing view"
        );
    }
}
