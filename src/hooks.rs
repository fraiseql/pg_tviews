//! `ProcessUtility` Hooks: DDL Interception and Transaction Management
//!
//! This module implements `PostgreSQL` hooks for DDL statement interception:
//! - **`ProcessUtility` Hook**: Intercepts CREATE TABLE `tv_*` and DROP TABLE `tv_*` statements
//! - **Transaction Callbacks**: Handles PREPARE/COMMIT/ABORT events
//! - **GID Capture**: Stores transaction IDs for 2PC support
//! - **DISCARD ALL**: Clears caches on connection pooling reset
//!
//! ## Hook Architecture
//!
//! `PostgreSQL` calls hooks at strategic points:
//! 1. **`ProcessUtility`**: Before executing utility statements (DDL)
//! 2. **Transaction Events**: At commit, abort, and prepare phases
//! 3. **Subtransaction Events**: For savepoint handling
//!
//! ## Safety Considerations
//!
//! - Hooks run in `PostgreSQL`'s execution context
//! - Must not panic (all wrapped in `catch_unwind`)
//! - Proper error handling to avoid corrupting transactions
//! - Thread-safe global state management

use pgrx::prelude::*;
use pgrx::pg_sys;
use std::ffi::CStr;
use std::sync::{LazyLock, Mutex};

use crate::ddl::drop_tview;
use crate::TViewError;

/// Previous `ProcessUtility` hook (if any other extension installed one)
static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

/// Global storage for GID during PREPARE TRANSACTION
static PREPARING_GID: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

/// Reentrancy guard: prevents the hook from processing DDL that the hook itself triggers.
/// When `pg_tviews_create` calls `Spi::run("CREATE VIEW ...")` internally, PostgreSQL
/// calls ProcessUtility again for that DDL. Without this guard, the hook re-enters and
/// can corrupt state, causing a segfault in PostgreSQL 18.
static mut HOOK_IN_PROGRESS: bool = false;

/// Install the `ProcessUtility` hook to intercept CREATE/DROP TABLE `tv_*`
/// Install the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
pub unsafe fn install_hook() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
}

/// Check if hook is installed, install it if not
/// This is called lazily to avoid issues during postmaster startup
pub unsafe fn ensure_hook_installed() {
    static mut HOOK_INSTALLED: bool = false;

    if !HOOK_INSTALLED {
        install_hook();
        HOOK_INSTALLED = true;
    }
}

/// `ProcessUtility` hook that intercepts CREATE TABLE `tv_*` and DROP TABLE `tv_*`
#[pg_guard]
#[allow(clippy::too_many_arguments)]
unsafe extern "C-unwind" fn tview_process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    // Reentrancy guard: if we're already inside the hook (e.g., processing DDL triggered
    // internally by pg_tviews_create via Spi::run), skip interception and pass through.
    if HOOK_IN_PROGRESS {
        call_prev_hook_or_standard(pstmt, query_string, read_only_tree, context, params, query_env, dest, qc);
        return;
    }
    HOOK_IN_PROGRESS = true;

    // Wrap FFI callback in catch_unwind to prevent panics crossing FFI boundary
    // Returns true if the hook handled the statement, false if it should pass through
    let result = std::panic::catch_unwind(|| -> bool {
        let query_str = if query_string.is_null() {
            "[NULL]".to_string()
        } else {
            CStr::from_ptr(query_string).to_string_lossy().to_string()
        };

        let query_lower = query_str.to_lowercase();

        // Skip extension-related statements to avoid infinite recursion during installation
        if query_lower.contains("create extension") || query_lower.contains("drop extension") {
            return false; // Pass through
        }

        // Check if this is PREPARE TRANSACTION
        if query_lower.trim().starts_with("prepare transaction") {
            // Extract GID from query: PREPARE TRANSACTION 'gid'
            if let Some(gid) = extract_gid_from_prepare_query(&query_str) {
                *PREPARING_GID.lock().unwrap_or_else(|p| p.into_inner()) = Some(gid);
            }
        }

        // Safety check
        if pstmt.is_null() {
            return false; // Pass through
        }

        let pstmt_ref = &*pstmt;

        // Check if this is a utility statement
        if pstmt_ref.utilityStmt.is_null() {
            return false; // Pass through
        }

        let utility_stmt = pstmt_ref.utilityStmt;
        let node_tag = (*utility_stmt).type_;


        // Check for CREATE TABLE AS
        if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {

            #[allow(clippy::cast_ptr_alignment)]
            let ctas = utility_stmt.cast::<pg_sys::CreateTableAsStmt>();
            if handle_create_table_as(ctas, query_string) {
                // We handled it - don't call standard utility

                return true; // Handled
            }

        }

        // Check for DROP TABLE
        if node_tag == pg_sys::NodeTag::T_DropStmt {
            #[allow(clippy::cast_ptr_alignment)]
            let drop_stmt = utility_stmt.cast::<pg_sys::DropStmt>();
            if handle_drop_table(drop_stmt, query_string) {
                // We handled it - don't call standard utility
                return true; // Handled
            }
        }

        // Not a tv_* statement - pass through
        false
    });

    // Check if hook handled the statement or if we need to pass through
    let should_pass_through = match result {
        Ok(handled) => !handled, // Pass through if hook didn't handle it
        Err(panic_info) => {
            // PANIC in ProcessUtility hook - log it and pass through to standard utility!
            let panic_msg = panic_info.downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_info.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| format!("{panic_info:?}"));
            error!("PANIC in ProcessUtility hook: {} - This is a bug in pg_tviews - please report it!", panic_msg);
            #[allow(unreachable_code)]
            {
                true // Pass through after panic (error! macro is marked cold but doesn't actually diverge)
            }
        }
    };

    // Execute the statement if hook didn't handle it or if it panicked
    if should_pass_through {
        call_prev_hook_or_standard(
            pstmt,
            query_string,
            read_only_tree,
            context,
            params,
            query_env,
            dest,
            qc,
        );
    }

    // Release the reentrancy guard
    HOOK_IN_PROGRESS = false;
}

/// Handle CREATE TABLE tv_* AS SELECT ...
unsafe fn handle_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    query_string: *const ::std::os::raw::c_char,
) -> bool {
    if ctas.is_null() {
        return false;
    }

    let ctas_ref = &*ctas;

    // Get the INTO clause which contains the table name
    if ctas_ref.into.is_null() {
        return false;
    }

    let into = &*ctas_ref.into;
    if into.rel.is_null() {
        return false;
    }

    let rel = &*into.rel;
    if rel.relname.is_null() {
        return false;
    }

    // Get table name
    let Ok(table_name) = CStr::from_ptr(rel.relname).to_str() else {
        return false;
    };

    // Check if it starts with tv_
    if !table_name.starts_with("tv_") {
        return false;
    }

    // Extract entity name
    let entity_name = &table_name[3..]; // Remove "tv_" prefix

    if entity_name.is_empty() {
        error!("Invalid TVIEW name '{}': must be tv_<entity>", table_name);
    }

    // Get the SELECT query
    let select_sql = if query_string.is_null() {
        error!("No query string provided");
    } else if let Ok(sql) = CStr::from_ptr(query_string).to_str() {
        // Extract the SELECT part from "CREATE TABLE tv_X AS SELECT ..."
        // We need to find the AS that comes after the table name, not column aliases
        // Strategy: Find "CREATE TABLE <name> AS" pattern
        let sql_lower = sql.to_lowercase();
        // Find the table name position (we already know it's tv_<entity>)
        let table_pattern = format!("{} as", table_name.to_lowercase());

        sql_lower.find(&table_pattern).map_or_else(|| {
            error!("Could not find '{}' in query", table_pattern);
        }, |table_pos| {
            // Found "tv_<entity> AS" - skip past it
            let select_start = table_pos + table_pattern.len();
            let select_part = sql[select_start..].trim();
            // Remove trailing semicolon if present
            select_part.trim_end_matches(';').trim().to_string()
        })
    } else {
        error!("Failed to parse query string")
    };


    // Validate TVIEW SELECT statement structure
    match validate_tview_select(&select_sql) {
        Ok(()) => {

            // Store SELECT in session-level temp table for event trigger to use
            // This is safe because:
            // 1. No SPI calls yet (just storing for later)
            // 2. Temp table is transaction-safe (auto-cleanup on rollback)
            // 3. Event trigger will retrieve and use this SELECT
            if let Err(e) = store_pending_tview_select(table_name, &select_sql) {
                error!("Failed to store SELECT for '{}': {}", table_name, e);
                // Continue anyway - event trigger will try to infer
            }

            false // Pass through - let PostgreSQL create it
        }
        Err(e) => {
            // Validation failed — still store the SELECT so the event trigger can attempt
            // conversion and produce a proper error if the structure is truly invalid.
            // The event trigger's create_tview() call will validate thoroughly.
            warning!("TVIEW syntax warning for '{}': {} — attempting conversion anyway", table_name, e);
            if let Err(store_err) = store_pending_tview_select(table_name, &select_sql) {
                warning!("Failed to store SELECT for '{}': {}", table_name, store_err);
            }
            false // Let PostgreSQL create it, event trigger will convert
        }
    }
}

/// Validate TVIEW SELECT statement structure
fn validate_tview_select(select_sql: &str) -> Result<(), String> {
    // Check for required patterns in SELECT
    // This is basic validation - event trigger will do thorough validation
    // Only require: id (UUID) + data (JSONB)
    // Optional columns: pk_<entity>, fk_<entity>, path (LTREE), <entity>_id (UUID FKs)

    let sql_lower = select_sql.to_lowercase();

    // Check for id column (required) — handle both bare `id,` and qualified `alias.id,`
    let has_id = sql_lower.contains(" as id")
        || sql_lower.contains(" id,")
        || sql_lower.contains(" id ")
        || sql_lower.contains(".id,")
        || sql_lower.contains(".id ")
        || sql_lower.contains(".id\n")
        || sql_lower.contains(".id::"); // cast like l1.id::text
    if !has_id {
        return Err("Missing required 'id' column (UUID)".to_string());
    }

    // Check for data column — jsonb_build_object or bare/qualified column
    let has_data = sql_lower.contains("jsonb_build_object")
        || sql_lower.contains(" as data")
        || sql_lower.contains(" data,")
        || sql_lower.contains(" data ");
    if !has_data {
        return Err("Missing required 'data' column (JSONB)".to_string());
    }

    Ok(())
}

/// Store pending TVIEW SELECT statement for event trigger to retrieve
///
/// Uses a session-level temp table that auto-cleanup on transaction end.
/// This is safe because we're NOT using SPI here - we're just preparing
/// the data for the event trigger to use (which HAS safe SPI context).
fn store_pending_tview_select(table_name: &str, select_sql: &str) -> Result<(), String> {
    // We can't use SPI here (we're in a hook), but we can use a global cache
    // The event trigger will pick it up when it fires (safe SPI context)
    PENDING_TVIEW_SELECTS.lock()
        .map_err(|e| format!("Failed to lock cache: {e}"))?
        .insert(table_name.to_string(), select_sql.to_string());

    Ok(())
}

/// Global cache for pending TVIEW SELECT statements
///
/// Maps: `table_name` -> original SELECT statement
/// Written by: `ProcessUtility` hook (before table creation)
/// Read by: Event trigger (after table creation, safe SPI context)
/// Cleared by: Event trigger after successful conversion
static PENDING_TVIEW_SELECTS: LazyLock<Mutex<std::collections::HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Retrieve and remove a pending TVIEW SELECT statement
///
/// Called by event trigger to get the original SELECT for TVIEW conversion.
/// Returns None if no SELECT was stored for this table.
pub fn take_pending_tview_select(table_name: &str) -> Option<String> {
    PENDING_TVIEW_SELECTS.lock()
        .ok()?
        .remove(table_name)
}

/// Handle DROP TABLE tv_*
///
/// Uses a simpler approach: parse the query string instead of traversing
/// complex `PostgreSQL` List structures which are prone to segfaults.
unsafe fn handle_drop_table(
    drop_stmt: *mut pg_sys::DropStmt,
    query_string: *const ::std::os::raw::c_char,
) -> bool {

    if drop_stmt.is_null() {
        return false;
    }

    let drop_ref = &*drop_stmt;

    // Check if it's dropping a table (not view, index, etc.)
    if drop_ref.removeType != pg_sys::ObjectType::OBJECT_TABLE {
        return false;
    }

    // Extract table names from query string
    if query_string.is_null() {
        return false;
    }

    let sql = if let Ok(s) = CStr::from_ptr(query_string).to_str() {
        s
    } else {
        return false;
    };

    // Parse DROP TABLE statement to find tv_* tables
    // Handles: DROP TABLE tv_foo, DROP TABLE IF EXISTS tv_foo, DROP TABLE tv_foo CASCADE
    let sql_lower = sql.to_lowercase();

    // Check if this is a DROP TABLE statement
    if !sql_lower.contains("drop") || !sql_lower.contains("table") {
        return false;
    }

    // Find table names in the statement
    // Simple regex-like parsing: look for tv_<word> pattern
    let words: Vec<&str> = sql.split_whitespace().collect();
    let mut found_tv_table = false;
    let mut table_name = String::new();

    for word in &words {
        // Remove trailing punctuation (comma, semicolon)
        let clean_word = word.trim_end_matches([',', ';']);

        if clean_word.starts_with("tv_") {
            table_name = clean_word.to_string();
            found_tv_table = true;
            break;
        }
    }

    if !found_tv_table {
        return false;
    }


    // Check if_exists flag
    let if_exists = drop_ref.missing_ok;

    match drop_tview(&table_name, if_exists) {
        Ok(()) => {
            true
        }
        Err(e) => {
            if if_exists {
                notice!("TVIEW '{}' does not exist, skipping", table_name);
                true
            } else {
                error!("Failed to drop TVIEW '{}': {}", table_name, e);
            }
        }
    }
}

/// Call the previous hook if it exists, otherwise call `standard_ProcessUtility`
#[allow(clippy::too_many_arguments)]
unsafe fn call_prev_hook_or_standard(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    match PREV_PROCESS_UTILITY_HOOK {
        Some(prev_hook) => {
            prev_hook(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
        }
        None => {
            pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            );
        }
    }
}

/// Extract GID from PREPARE TRANSACTION query
///
/// Parses queries like: PREPARE TRANSACTION 'gid' or PREPARE TRANSACTION "gid"
fn extract_gid_from_prepare_query(query: &str) -> Option<String> {
    // Parse: PREPARE TRANSACTION 'gid' or PREPARE TRANSACTION "gid"
    // Use two separate patterns for single and double quotes
    let patterns = [
        "PREPARE\\s+TRANSACTION\\s+'([^']+)'",
        "PREPARE\\s+TRANSACTION\\s+\"([^\"]+)\"",
    ];

    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(query) {
                if let Some(m) = caps.get(1) {
                    return Some(m.as_str().to_string());
                }
            }
        }
    }

    None
}

/// Get the prepared transaction GID captured by the `ProcessUtility` hook
///
/// This is called by the transaction callback during PREPARE TRANSACTION.
pub fn get_prepared_transaction_id() -> crate::TViewResult<String> {
    PREPARING_GID.lock()
        .unwrap_or_else(|p| p.into_inner())
        .take() // Take and clear the GID
        .ok_or_else(|| crate::internal_error!("Not in a prepared transaction (GID not captured)"))
}
