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

pub mod aggregate;
pub mod create;
pub mod drop;
pub mod rename;

pub use create::create_tview;
pub use drop::drop_tview;

use pgrx::prelude::*;

/// SQL function: Create a TVIEW
///
/// Usage: SELECT `pg_tviews_create`('`my_entity`', 'SELECT id, name FROM users WHERE active = true');
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;

    // Ensure ProcessUtility hook is installed for DDL syntax support.
    // SAFETY: Called from PostgreSQL backend context, hook installation is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    match create_tview(tview_name, select_sql, None, false) {
        Ok(()) => Ok(format!("TVIEW '{tview_name}' created successfully")),
        Err(e) => Err(format!("Failed to create TVIEW: {e}")),
    }
}

/// SQL function: create an aggregate TVIEW (issue #58).
///
/// Usage:
/// `SELECT pg_tviews_create_aggregate('tv_user_summary', $$ SELECT o.fk_user AS
///  pk_user_summary, u.id, jsonb_build_object('orders', count(*)) AS data FROM tb_order o
///  JOIN tb_user u ON u.pk_user = o.fk_user GROUP BY o.fk_user, u.id $$,
///  '{"tb_order": "fk_user", "tb_user": "pk_user"}');`
///
/// `group_keys` maps each source table to the column whose value is the group key.
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires JsonB by value
fn pg_tviews_create_aggregate(
    tview_name: &str,
    select_sql: &str,
    group_keys: pgrx::JsonB,
) -> Result<String, String> {
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;
    let keys: aggregate::GroupKeys = serde_json::from_value(group_keys.0).map_err(|_| {
        "group_keys must be a JSON object mapping source table names to column names".to_string()
    })?;
    // SAFETY: Called from PostgreSQL backend context, hook installation is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }
    create::create_aggregate_tview(tview_name, select_sql, &keys)
        .map(|()| format!("TVIEW '{tview_name}' created successfully"))
        .map_err(|e| format!("Failed to create aggregate TVIEW: {e}"))
}

/// Internal: called by the `sql_drop` event trigger for a TVIEW whose backing view
/// or table was dropped as a dependent (see [`drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), String> {
    drop::handle_dropped(entity).map_err(|e| format!("Failed to deregister TVIEW '{entity}': {e}"))
}

/// SQL function: Drop a TVIEW
///
/// Usage: SELECT `pg_tviews_drop`('`my_entity`', true);        -- true = IF EXISTS
///        SELECT `pg_tviews_drop`('`my_entity`', true, true);  -- IF EXISTS + CASCADE
#[pg_extern]
fn pg_tviews_drop(
    tview_name: &str,
    if_exists: default!(bool, false),
    cascade: default!(bool, false),
) -> Result<String, String> {
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;

    match drop_tview(tview_name, if_exists, cascade) {
        Ok(()) => Ok(format!("TVIEW '{tview_name}' dropped successfully")),
        Err(e) => Err(format!("Failed to drop TVIEW: {e}")),
    }
}

/// SQL function: rebind the relation OIDs inside `cascade_paths` to the current
/// catalog. Called by the `pg_tview_meta` insert trigger so that rows loaded by
/// `pg_restore` point at the restored relations; not meant to be called directly.
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires Vec by value
fn pg_tviews_rebind_cascade_paths(
    view_oid: pg_sys::Oid,
    cascade_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    create::rebind_cascade_paths(view_oid, &cascade_paths)
        .map_err(|e| format!("Failed to rebind cascade paths: {e}"))
}

/// SQL function: deprecated, always raises an error.
///
/// It replaced `tv_x` with a view over a literal `VALUES` snapshot (no triggers,
/// no refresh), and could not run on PG18. Use [`pg_tviews_create`] or
/// `CREATE TABLE tv_x AS SELECT ...` instead. The function is kept only so that
/// callers get this message; it is removed in the next breaking release.
#[pg_extern]
fn pg_tviews_convert_existing_table(table_name: &str) -> Result<String, String> {
    crate::validation::validate_sql_identifier(table_name, "table_name")
        .map_err(|e| format!("Invalid table name: {e}"))?;

    Err(format!(
        "pg_tviews_convert_existing_table() is deprecated and no longer converts '{table_name}'; \
         use pg_tviews_create() or CREATE TABLE tv_<entity> AS SELECT ... instead"
    ))
}
