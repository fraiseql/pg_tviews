pub mod create;
pub mod drop;

pub use create::create_tview;
pub use drop::drop_tview;

use pgrx::prelude::*;

/// SQL function: Create a TVIEW
///
/// Usage: SELECT pg_tviews_create('my_entity', 'SELECT id, name FROM users WHERE active = true');
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
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
