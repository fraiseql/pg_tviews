use pgrx::prelude::*;
use pgrx::pg_sys;
use std::ffi::CStr;
use once_cell::sync::Lazy;
use std::sync::Mutex;

use crate::ddl::{create_tview, drop_tview};
use crate::TViewError;

/// Previous ProcessUtility hook (if any other extension installed one)
static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

/// Global storage for GID during PREPARE TRANSACTION
static PREPARING_GID: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Install the ProcessUtility hook to intercept CREATE/DROP TABLE tv_*
pub unsafe fn install_hook() {
    PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
    pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
}

/// ProcessUtility hook that intercepts CREATE TABLE tv_* and DROP TABLE tv_*
#[pg_guard]
#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn tview_process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    // Log ALL hook invocations to debug why DROP isn't being caught
    let query_str = if !query_string.is_null() {
        CStr::from_ptr(query_string).to_string_lossy().to_string()
    } else {
        "[NULL]".to_string()
    };
    info!("🔧 HOOK CALLED: {}", query_str);

    // Skip extension-related statements to avoid infinite recursion during installation
    let query_lower = query_str.to_lowercase();
    if query_lower.contains("create extension") || query_lower.contains("drop extension") {
        info!("  → Extension statement, passing through without interception");
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
        return;
    }

    // Check if this is PREPARE TRANSACTION
    if query_lower.trim().starts_with("prepare transaction") {
        // Extract GID from query: PREPARE TRANSACTION 'gid'
        if let Some(gid) = extract_gid_from_prepare_query(&query_str) {
            *PREPARING_GID.lock().unwrap() = Some(gid);
            info!("  → Captured GID for PREPARE TRANSACTION");
        }
    }

    // Safety check
    if pstmt.is_null() {
        info!("  → pstmt is null, passing through");
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
        return;
    }

    let pstmt_ref = &*pstmt;

    // Check if this is a utility statement
    if pstmt_ref.utilityStmt.is_null() {
        info!("  → utilityStmt is null, passing through");
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
        return;
    }

    let utility_stmt = pstmt_ref.utilityStmt;
    let node_tag = (*utility_stmt).type_;
    info!("  → node_tag = {:?}", node_tag);

    // Check for CREATE TABLE AS
    if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {
        let ctas = utility_stmt as *mut pg_sys::CreateTableAsStmt;
        if handle_create_table_as(ctas, query_string) {
            // We handled it - don't call standard utility
            return;
        }
    }

    // Check for DROP TABLE
    if node_tag == pg_sys::NodeTag::T_DropStmt {
        info!("  ✓ Detected T_DropStmt!");
        let drop_stmt = utility_stmt as *mut pg_sys::DropStmt;
        if handle_drop_table(drop_stmt, query_string) {
            // We handled it - don't call standard utility
            info!("  ✓ DROP was handled by hook, NOT calling standard utility");
            return;
        }
        info!("  → DROP not handled, passing through to standard utility");
    }

    // Not a tv_* statement - pass through
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
    let table_name = match CStr::from_ptr(rel.relname).to_str() {
        Ok(name) => name,
        Err(_) => return false,
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
    let select_sql = if !query_string.is_null() {
        match CStr::from_ptr(query_string).to_str() {
            Ok(sql) => {
                // Extract the SELECT part from "CREATE TABLE tv_X AS SELECT ..."
                if let Some(pos) = sql.to_lowercase().find(" as ") {
                    let select_part = sql[pos + 4..].trim();
                    // Remove trailing semicolon if present
                    select_part.trim_end_matches(';').trim().to_string()
                } else {
                    error!("Invalid CREATE TABLE AS syntax");
                }
            }
            Err(_) => error!("Failed to parse query string"),
        }
    } else {
        error!("No query string provided");
    };

    info!("Intercepted CREATE TABLE {} - converting to TVIEW", table_name);

    // Call our TVIEW creation logic
    match create_tview(table_name, &select_sql) {
        Ok(()) => {
            info!("TVIEW '{}' created successfully", table_name);
            true
        }
        Err(e) => {
            error!("Failed to create TVIEW '{}': {}", table_name, e);
        }
    }
}

/// Handle DROP TABLE tv_*
///
/// Uses a simpler approach: parse the query string instead of traversing
/// complex PostgreSQL List structures which are prone to segfaults.
unsafe fn handle_drop_table(
    drop_stmt: *mut pg_sys::DropStmt,
    query_string: *const ::std::os::raw::c_char,
) -> bool {
    info!("handle_drop_table called");

    if drop_stmt.is_null() {
        info!("  drop_stmt is null, returning false");
        return false;
    }

    let drop_ref = &*drop_stmt;

    // Check if it's dropping a table (not view, index, etc.)
    info!("  removeType = {:?}", drop_ref.removeType);
    if drop_ref.removeType != pg_sys::ObjectType::OBJECT_TABLE {
        info!("  not a table drop, returning false");
        return false;
    }

    // Extract table names from query string
    if query_string.is_null() {
        info!("  query_string is null, returning false");
        return false;
    }

    let sql = match CStr::from_ptr(query_string).to_str() {
        Ok(s) => {
            info!("  query_string = '{}'", s);
            s
        }
        Err(_) => {
            info!("  failed to parse query_string, returning false");
            return false;
        }
    };

    // Parse DROP TABLE statement to find tv_* tables
    // Handles: DROP TABLE tv_foo, DROP TABLE IF EXISTS tv_foo, DROP TABLE tv_foo CASCADE
    let sql_lower = sql.to_lowercase();

    // Check if this is a DROP TABLE statement
    if !sql_lower.contains("drop") || !sql_lower.contains("table") {
        info!("  not a DROP TABLE statement, returning false");
        return false;
    }

    // Find table names in the statement
    // Simple regex-like parsing: look for tv_<word> pattern
    let words: Vec<&str> = sql.split_whitespace().collect();
    let mut found_tv_table = false;
    let mut table_name = String::new();

    for word in words.iter() {
        // Remove trailing punctuation (comma, semicolon)
        let clean_word = word.trim_end_matches([',', ';']);

        if clean_word.starts_with("tv_") {
            table_name = clean_word.to_string();
            found_tv_table = true;
            info!("  found tv_* table: {}", table_name);
            break;
        }
    }

    if !found_tv_table {
        info!("  no tv_* table found in query, returning false");
        return false;
    }

    info!("Intercepted DROP TABLE {} - cleaning up TVIEW", table_name);

    // Check if_exists flag
    let if_exists = drop_ref.missing_ok;

    match drop_tview(&table_name, if_exists) {
        Ok(()) => {
            info!("TVIEW '{}' dropped successfully", table_name);
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

/// Call the previous hook if it exists, otherwise call standard_ProcessUtility
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

/// Get the prepared transaction GID captured by the ProcessUtility hook
///
/// This is called by the transaction callback during PREPARE TRANSACTION.
pub fn get_prepared_transaction_id() -> crate::TViewResult<String> {
    PREPARING_GID.lock().unwrap()
        .take() // Take and clear the GID
        .ok_or_else(|| crate::internal_error!("Not in a prepared transaction (GID not captured)"))
}
