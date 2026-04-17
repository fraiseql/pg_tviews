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

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;
use std::sync::{LazyLock, Mutex};

use crate::TViewError;
use crate::ddl::drop_tview;

/// Previous `ProcessUtility` hook (if any other extension installed one)
static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

/// Reentrancy guard: prevents the hook from processing DDL that the hook itself triggers.
/// When `pg_tviews_create` calls `Spi::run("CREATE VIEW ...")` internally, `PostgreSQL`
/// calls `ProcessUtility` again for that DDL. Without this guard, the hook re-enters and
/// can corrupt state, causing a segfault in `PostgreSQL` 18.
static mut HOOK_IN_PROGRESS: bool = false;

/// Install the `ProcessUtility` hook to intercept CREATE/DROP TABLE `tv_*`
/// Install the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
pub unsafe fn install_hook() {
    unsafe {
        PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
    }
}

/// Check if hook is installed, install it if not
/// This is called lazily to avoid issues during postmaster startup
pub unsafe fn ensure_hook_installed() {
    unsafe {
        static mut HOOK_INSTALLED: bool = false;

        if !HOOK_INSTALLED {
            install_hook();
            HOOK_INSTALLED = true;
        }
    }
}

/// `ProcessUtility` hook that intercepts CREATE TABLE `tv_*` and DROP TABLE `tv_*`
#[pg_guard]
#[allow(clippy::too_many_arguments)] // Reason: PostgreSQL ProcessUtility_hook C callback signature
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
    // Safety: This entire function is an extern "C-unwind" callback invoked by
    // PostgreSQL internals — all pointer dereferences and static accesses are
    // inherently unsafe FFI operations.

    // Reentrancy guard: if we're already inside the hook (e.g., processing DDL triggered
    // internally by pg_tviews_create via Spi::run), skip interception and pass through.
    if unsafe { HOOK_IN_PROGRESS } {
        unsafe {
            call_prev_hook_or_standard(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                qc,
            )
        };
        return;
    }
    unsafe { HOOK_IN_PROGRESS = true };

    // Check for COMMIT/END BEFORE the catch_unwind block.
    // flush_refresh_queue() uses SPI which may trigger PostgreSQL ereport(ERROR)
    // → longjmp → pgrx panic. This MUST NOT be caught by catch_unwind because
    // that corrupts PG_exception_stack. The #[pg_guard] on this function handles
    // error propagation correctly via C-unwind.
    if !pstmt.is_null() && unsafe { !(*pstmt).utilityStmt.is_null() } {
        let utility_stmt = unsafe { (*pstmt).utilityStmt };
        if unsafe { (*utility_stmt).type_ } == pg_sys::NodeTag::T_TransactionStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → TransactionStmt* cast
            let xact_stmt = utility_stmt.cast::<pg_sys::TransactionStmt>();
            if !xact_stmt.is_null() {
                let kind = unsafe { (*xact_stmt).kind };

                if kind == pg_sys::TransactionStmtKind::TRANS_STMT_COMMIT {
                    if let Err(e) = crate::queue::flush_refresh_queue() {
                        unsafe { HOOK_IN_PROGRESS = false };
                        error!("TVIEW refresh failed before COMMIT: {e:?}");
                    }
                    if let Err(e) = crate::audit::flush_audit_buffer() {
                        unsafe { HOOK_IN_PROGRESS = false };
                        error!("Audit flush failed before COMMIT: {e:?}");
                    }
                }

                // Reject PREPARE TRANSACTION when TVIEW refreshes are pending.
                // Full 2PC support is not implemented in 0.1.0 — the queue would
                // be silently discarded, leaving TVIEWs stale.
                if kind == pg_sys::TransactionStmtKind::TRANS_STMT_PREPARE
                    && !crate::queue::is_queue_empty()
                {
                    unsafe { HOOK_IN_PROGRESS = false };
                    error!(
                        "pg_tviews: PREPARE TRANSACTION is not supported when \
                            TVIEW refreshes are pending; commit or rollback first"
                    );
                }
            }
        }
    }

    // Wrap FFI callback in catch_unwind to prevent panics crossing FFI boundary
    // Returns true if the hook handled the statement, false if it should pass through
    let result = std::panic::catch_unwind(|| -> Result<bool, TViewError> {
        let query_str = if query_string.is_null() {
            "[NULL]".to_string()
        } else {
            unsafe { CStr::from_ptr(query_string) }
                .to_string_lossy()
                .to_string()
        };

        let query_lower = query_str.to_lowercase();

        // Skip extension-related statements to avoid infinite recursion during installation
        if query_lower.contains("create extension") || query_lower.contains("drop extension") {
            return Ok(false); // Pass through
        }

        // Safety check
        if pstmt.is_null() {
            return Ok(false); // Pass through
        }

        let pstmt_ref = unsafe { &*pstmt };

        // Check if this is a utility statement
        if pstmt_ref.utilityStmt.is_null() {
            return Ok(false); // Pass through
        }

        let utility_stmt = pstmt_ref.utilityStmt;
        let node_tag = unsafe { (*utility_stmt).type_ };

        // Check for CREATE TABLE AS
        if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
            let ctas = utility_stmt.cast::<pg_sys::CreateTableAsStmt>();
            match unsafe { handle_create_table_as(ctas, query_string) } {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => return Err(e),
            }
        }

        // Check for DROP TABLE
        if node_tag == pg_sys::NodeTag::T_DropStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
            let drop_stmt = utility_stmt.cast::<pg_sys::DropStmt>();
            match unsafe { handle_drop_table(drop_stmt, query_string) } {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => return Err(e),
            }
        }

        // Not a tv_* statement - pass through
        Ok(false)
    });

    // Check if hook handled the statement or if we need to pass through
    let should_pass_through = match result {
        Ok(Ok(handled)) => !handled, // Pass through if hook didn't handle it
        Ok(Err(handler_err)) => {
            // Handler returned an error — reset guard BEFORE raising error!()
            // so that subsequent statements in this session are still intercepted.
            unsafe { HOOK_IN_PROGRESS = false };
            error!("{handler_err}");
            #[allow(unreachable_code)] // Reason: pgrx error!() diverges via longjmp, not Rust's !
            {
                true
            }
        }
        Err(panic_info) => {
            // PANIC in ProcessUtility hook - reset guard BEFORE raising error!()
            unsafe { HOOK_IN_PROGRESS = false };
            let panic_msg = panic_info
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_info.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| format!("{panic_info:?}"));
            error!(
                "PANIC in ProcessUtility hook: {} - This is a bug in pg_tviews - please report it!",
                panic_msg
            );
            #[allow(unreachable_code)] // Reason: pgrx error!() diverges via longjmp, not Rust's !
            {
                true
            }
        }
    };

    // Execute the statement if hook didn't handle it or if it panicked
    if should_pass_through {
        unsafe {
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
    }

    // Release the reentrancy guard
    unsafe { HOOK_IN_PROGRESS = false };
}

/// Handle CREATE TABLE tv_* AS SELECT ...
///
/// Returns `Ok(true)` if the hook handled the statement, `Ok(false)` if it should
/// pass through. Returns `Err` on failures that should abort with `error!()` —
/// the caller is responsible for resetting `HOOK_IN_PROGRESS` before raising.
unsafe fn handle_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // Safety: all pointer dereferences are guarded by null checks above each use.
    unsafe {
        if ctas.is_null() {
            return Ok(false);
        }

        let ctas_ref = &*ctas;

        // Get the INTO clause which contains the table name
        if ctas_ref.into.is_null() {
            return Ok(false);
        }

        let into = &*ctas_ref.into;
        if into.rel.is_null() {
            return Ok(false);
        }

        let rel = &*into.rel;
        if rel.relname.is_null() {
            return Ok(false);
        }

        // Get table name
        let Ok(table_name) = CStr::from_ptr(rel.relname).to_str() else {
            return Ok(false);
        };

        // Check if it starts with tv_
        if !table_name.starts_with("tv_") {
            return Ok(false);
        }

        // Get the explicit schema from `CREATE TABLE [schema.]tv_* AS SELECT …`.
        // NULL means the schema was omitted — the event trigger will resolve it at
        // runtime via `current_schema()`.  Non-NULL overrides `current_schema()` so
        // the TVIEW lands in the schema the user actually specified.
        let schema_name = if rel.schemaname.is_null() {
            String::new()
        } else {
            CStr::from_ptr(rel.schemaname)
                .to_str()
                .unwrap_or("")
                .to_string()
        };

        // Extract entity name
        let entity_name = &table_name[3..]; // Remove "tv_" prefix

        if entity_name.is_empty() {
            return Err(TViewError::InvalidTViewName {
                name: table_name.to_string(),
                reason: "must be tv_<entity>".to_string(),
            });
        }

        // Get the SELECT query
        let select_sql = if query_string.is_null() {
            return Err(crate::internal_error!(
                "No query string provided for CREATE TABLE AS"
            ));
        } else if let Ok(sql) = CStr::from_ptr(query_string).to_str() {
            // Extract the SELECT part from "CREATE TABLE tv_X AS SELECT ..."
            // We need to find the AS that comes after the table name, not column aliases
            // Strategy: Find "CREATE TABLE <name> AS" pattern
            let sql_lower = sql.to_lowercase();
            // Find the table name position (we already know it's tv_<entity>)
            let table_pattern = format!("{} as", table_name.to_lowercase());

            if let Some(table_pos) = sql_lower.find(&table_pattern) {
                // Found "tv_<entity> AS" - skip past it
                let select_start = table_pos + table_pattern.len();
                let select_part = sql[select_start..].trim();
                // Remove trailing semicolon if present
                select_part.trim_end_matches(';').trim().to_string()
            } else {
                return Err(TViewError::InvalidSelectStatement {
                    sql: sql.to_string(),
                    reason: format!("Could not find '{table_pattern}' in query"),
                });
            }
        } else {
            return Err(crate::internal_error!("Failed to parse query string"));
        };

        // Validate TVIEW SELECT statement structure
        match validate_tview_select(&select_sql) {
            Ok(()) => {
                // Store SELECT + schema in cache for event trigger to use
                if let Err(e) = store_pending_tview_select(table_name, &schema_name, &select_sql) {
                    return Err(crate::internal_error!(
                        "Failed to store SELECT for '{}': {}",
                        table_name,
                        e
                    ));
                }

                Ok(false) // Pass through - let PostgreSQL create it
            }
            Err(e) => {
                // Validation failed — still store the SELECT so the event trigger can attempt
                // conversion and produce a proper error if the structure is truly invalid.
                warning!(
                    "TVIEW syntax warning for '{}': {} — attempting conversion anyway",
                    table_name,
                    e
                );
                if let Err(store_err) =
                    store_pending_tview_select(table_name, &schema_name, &select_sql)
                {
                    warning!("Failed to store SELECT for '{}': {}", table_name, store_err);
                }
                Ok(false) // Let PostgreSQL create it, event trigger will convert
            }
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

/// Store pending TVIEW SELECT statement and target schema for event trigger to retrieve.
///
/// Uses a session-level in-memory cache. The event trigger reads it when it fires
/// (safe SPI context). `schema_name` is the explicit schema from the CREATE TABLE
/// statement (e.g. "public" for `CREATE TABLE public.tv_org AS SELECT …`), or an
/// empty string when the schema was not specified (caller should fall back to
/// `current_schema()` at event-trigger time).
fn store_pending_tview_select(
    table_name: &str,
    schema_name: &str,
    select_sql: &str,
) -> Result<(), String> {
    PENDING_TVIEW_SELECTS
        .lock()
        .map_err(|e| format!("Failed to lock cache: {e}"))?
        .insert(
            table_name.to_string(),
            (schema_name.to_string(), select_sql.to_string()),
        );

    Ok(())
}

/// Global cache for pending TVIEW SELECT statements.
///
/// Maps: `table_name` → `(schema_name, select_sql)`.
/// `schema_name` is the explicit schema from `CREATE TABLE [schema.]tv_* AS SELECT …`,
/// or an empty string when the schema was omitted.
/// Written by: `ProcessUtility` hook (before table creation)
/// Read by: Event trigger (after table creation, safe SPI context)
/// Cleared by: Event trigger after successful conversion
static PENDING_TVIEW_SELECTS: LazyLock<Mutex<std::collections::HashMap<String, (String, String)>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// Retrieve and remove a pending TVIEW `(schema_name, SELECT)` pair.
///
/// Called by event trigger to get the original SELECT and target schema for TVIEW
/// conversion.  Returns `None` if no entry was stored for this table (which means the
/// table was created by `pg_tviews_create()` directly, not via DDL interception).
pub fn take_pending_tview_select(table_name: &str) -> Option<(String, String)> {
    PENDING_TVIEW_SELECTS.lock().ok()?.remove(table_name)
}

/// Handle DROP TABLE tv_*
///
/// Iterates over the parsed `DropStmt.objects` list to correctly handle
/// multi-table statements like `DROP TABLE tv_a, tv_b` and mixed statements
/// like `DROP TABLE regular_table, tv_foo`.
///
/// Returns `Ok(true)` if ALL tables were tv_* and handled, `Ok(false)` if
/// no tv_* tables found (pass-through entirely). For mixed statements
/// containing both tv_* and non-tv_* tables, drops the tv_* ones and returns
/// `Ok(false)` to let the standard handler process the remaining tables.
///
/// Returns `Err` on failures — caller resets `HOOK_IN_PROGRESS` before raising.
unsafe fn handle_drop_table(
    drop_stmt: *mut pg_sys::DropStmt,
    _query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // Safety: all pointer dereferences are guarded by null checks.
    unsafe {
        if drop_stmt.is_null() {
            return Ok(false);
        }

        let drop_ref = &*drop_stmt;

        // Check if it's dropping a table (not view, index, etc.)
        if drop_ref.removeType != pg_sys::ObjectType::OBJECT_TABLE {
            return Ok(false);
        }

        let objects = drop_ref.objects;
        if objects.is_null() {
            return Ok(false);
        }

        let if_exists = drop_ref.missing_ok;

        // Collect table names and indices from DropStmt.objects.
        // Each element in objects is a List* of String* (name parts: [schema, table] or [table]).
        let num_tables = pg_sys::list_length(objects);
        let mut tv_entries: Vec<(i32, String)> = Vec::new(); // (index, name)
        let mut has_non_tv = false;

        for i in 0..num_tables {
            let name_list = pg_sys::list_nth(objects, i) as *mut pg_sys::List;
            if name_list.is_null() {
                has_non_tv = true;
                continue;
            }

            // The last element in the name list is the table name (unqualified)
            let name_parts = pg_sys::list_length(name_list);
            if name_parts == 0 {
                has_non_tv = true;
                continue;
            }

            // Get the last name part (table name, ignoring schema qualification)
            let last_part = pg_sys::list_nth(name_list, name_parts - 1) as *mut pg_sys::String;
            if last_part.is_null() {
                has_non_tv = true;
                continue;
            }

            let sval = (*last_part).sval;
            if sval.is_null() {
                has_non_tv = true;
                continue;
            }

            let Ok(table_name) = CStr::from_ptr(sval).to_str() else {
                has_non_tv = true;
                continue;
            };

            if table_name.starts_with("tv_") {
                tv_entries.push((i, table_name.to_string()));
            } else {
                has_non_tv = true;
            }
        }

        if tv_entries.is_empty() {
            return Ok(false);
        }

        // Drop each tv_* table via drop_tview
        for (_, name) in &tv_entries {
            match drop_tview(name, if_exists) {
                Ok(()) => {}
                Err(e) => {
                    if if_exists {
                        notice!("TVIEW '{}' does not exist, skipping", name);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        // If there were non-tv_* tables, remove tv_* entries from the objects list
        // so the standard handler only processes the remaining non-tv_* tables.
        if has_non_tv {
            // Remove in reverse index order to preserve indices
            for (idx, _) in tv_entries.iter().rev() {
                pg_sys::list_delete_nth_cell(objects, *idx);
            }
            return Ok(false);
        }

        // All tables were tv_* — we handled everything
        Ok(true)
    } // unsafe
}

/// Call the previous hook if it exists, otherwise call `standard_ProcessUtility`
#[allow(clippy::too_many_arguments)] // Reason: PostgreSQL ProcessUtility_hook C callback signature
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
    unsafe {
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
}
