//! DDL Operations: TVIEW Creation and Management
//!
//! This module handles Data Definition Language operations for TVIEWs:
//! - **CREATE TVIEW**: Parses SQL, creates metadata, sets up triggers
//! - **DROP TVIEW**: Cleans up metadata, removes triggers and views
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
/// Usage: SELECT pg_tviews_create('my_entity', 'SELECT id, name FROM users WHERE active = true');
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
    // Ensure ProcessUtility hook is installed for DDL syntax support
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
/// Usage: SELECT pg_tviews_drop('my_entity', true);  -- true = IF EXISTS
#[pg_extern]
fn pg_tviews_drop(tview_name: &str, if_exists: default!(bool, false)) -> Result<String, String> {
    match drop_tview(tview_name, if_exists) {
        Ok(()) => Ok(format!("TVIEW '{}' dropped successfully", tview_name)),
        Err(e) => Err(format!("Failed to drop TVIEW: {}", e)),
    }
}
