//! DDL Operations: TVIEW Creation and Management
//!
//! This module handles Data Definition Language operations for TVIEWs:
//! - **CREATE TABLE tv_ AS SELECT**: Parses SQL, creates metadata, sets up triggers
//! - **DROP TABLE tv_***: Cleans up metadata, removes triggers and views
//! - **Validation**: Ensures TVIEW names and SQL are valid
//!
//! ## Architecture
//!
//! DDL operations follow this sequence:
//! 1. Parse and validate TVIEW name (`tv_*` format)
//! 2. Analyze SELECT statement for column types and dependencies
//! 3. Create metadata entries in `pg_tview_meta`
//! 4. Set up triggers on base tables for change tracking
//! 5. Create the actual view with refresh triggers

pub mod create;
pub mod drop;
pub mod convert;

pub use create::create_tview;
pub use drop::drop_tview;
pub use convert::convert_existing_table_to_tview;

use pgrx::prelude::*;

/// SQL function: Create a TVIEW
///
/// Usage: `SELECT pg_tviews_create('my_entity', 'SELECT id, name FROM users WHERE active = true')`;
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
    // Ensure `ProcessUtility` hook is installed for DDL syntax support
    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    match create_tview(tview_name, select_sql) {
        Ok(()) => Ok(format!("TVIEW '{}' created successfully", tview_name)),
        Err(e) => Err(format!("Failed to create TVIEW: {}", e)),
    }
}

/// SQL function: Drop a TVIEW
///
/// Usage: `SELECT pg_tviews_drop('my_entity', true)`;  -- true = IF EXISTS
#[pg_extern]
fn pg_tviews_drop(tview_name: &str, if_exists: default!(bool, false)) -> Result<String, String> {
    match drop_tview(tview_name, if_exists) {
        Ok(()) => Ok(format!("TVIEW '{}' dropped successfully", tview_name)),
        Err(e) => Err(format!("Failed to drop TVIEW: {}", e)),
    }
}

/// SQL function: Convert existing table to TVIEW (for benchmarking/testing)
///
/// Usage: `SELECT pg_tviews_convert_existing_table('tv_product')`;
///
/// This function converts a table that was created with standard `DDL`
/// into a proper TVIEW structure with triggers and metadata.
///
/// Note: Different from the internal `pg_tviews_convert_table()` which is called
/// by event triggers during CREATE TABLE interception.
#[pg_extern]
fn pg_tviews_convert_existing_table(table_name: &str) -> Result<String, String> {
    match convert_existing_table_to_tview(table_name) {
        Ok(()) => Ok(format!("Table '{}' converted to TVIEW successfully", table_name)),
        Err(e) => Err(format!("Failed to convert table to TVIEW: {}", e)),
    }
}

/// SQL function: Refresh TVIEW data (for benchmarking/testing)
///
/// Usage: `SELECT pg_tviews_refresh('tv_product')`;
///
/// This is primarily for benchmarking - in production, TVIEWs auto-refresh via triggers.
/// This function forces a full refresh by truncating and repopulating from the base view.
#[pg_extern]
fn pg_tviews_refresh(tview_name: &str) -> Result<String, String> {
    // For benchmarking, we can do a simple truncate + insert from view
    let view_name = tview_name.replace("tv_", "v_");
    let sql = format!(
        "TRUNCATE {}; INSERT INTO {} SELECT * FROM {}",
        tview_name, tview_name, view_name
    );

    match Spi::run(&sql) {
        Ok(_) => Ok(format!("TVIEW '{}' refreshed successfully", tview_name)),
        Err(e) => Err(format!("Failed to refresh TVIEW: {}", e)),
    }
}
