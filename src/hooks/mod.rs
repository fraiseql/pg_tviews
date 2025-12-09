use pgrx::prelude::*;
use std::sync::Once;
use crate::error::{TViewError, TViewResult};
use crate::parser::{parse_create_tview, parse_drop_tview};

static INIT_HOOK: Once = Once::new();

// SAFETY: ProcessUtility hook installation
//
// Invariants:
// 1. PostgreSQL extension initialization (_PG_init) is called exactly once per backend
// 2. _PG_init runs before any SQL commands execute (single-threaded context)
// 3. ProcessUtility hook is called serially by PostgreSQL (no concurrent DDL)
// 4. PREV_PROCESS_UTILITY_HOOK is written once during init, read during hook execution
//
// Checked:
// - Once::call_once ensures single initialization even if _PG_init called multiple times
// - Hook is installed before any SQL commands can run
// - PostgreSQL DDL lock (ShareLock on system catalogs) ensures no concurrent CREATE TVIEW
//
// Lifetime:
// - Static variable lives for entire backend lifetime
// - Hook pointer is valid for process lifetime (PostgreSQL internal)
// - No deallocation needed (PostgreSQL manages hook lifecycle)
//
// Synchronization:
// - Once::call_once provides memory barrier
// - PostgreSQL guarantees serial DDL execution (AccessExclusiveLock on objects)
// - No additional locking needed
//
// Reviewed: 2025-12-09, PostgreSQL Expert
static mut PREV_PROCESS_UTILITY_HOOK: Option<ProcessUtilityHook> = None;

type ProcessUtilityHook = unsafe extern "C" fn(
    *mut pg_sys::PlannedStmt,
    *const std::os::raw::c_char,
    bool,
    pg_sys::ProcessUtilityContext::Type,
    *mut pg_sys::ParamListInfoData,
    *mut pg_sys::QueryEnvironment,
    *mut pg_sys::DestReceiver,
    *mut pg_sys::QueryCompletion,
);

pub fn install_hooks() {
    INIT_HOOK.call_once(|| {
        // SAFETY: See detailed comment above
        unsafe {
            PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
            pg_sys::ProcessUtility_hook = Some(process_utility_hook);
        }
        info!("pg_tviews ProcessUtility hook installed");
    });
}

#[pg_guard]
unsafe extern "C" fn process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: *mut pg_sys::ParamListInfoData,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    // SAFETY: query_string is guaranteed valid by PostgreSQL
    // It points to the query buffer, which outlives this hook call
    let query_cstr = unsafe { std::ffi::CStr::from_ptr(query_string) };

    let query_str = match query_cstr.to_str() {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid UTF-8 in query string: {}", e);
        }
    };

    let query_upper = query_str.trim().to_uppercase();

    // Handle CREATE TVIEW
    if query_upper.starts_with("CREATE TVIEW") {
        match handle_create_tview_safe(query_str) {
            Ok(_) => {
                // Success - complete the command
                unsafe {
                    if !completion_tag.is_null() {
                        (*completion_tag).commandTag = pg_sys::CommandTag::CMDTAG_SELECT;
                        (*completion_tag).nprocessed = 0;
                    }
                }
                return;
            }
            Err(e) => {
                // Error - raise to PostgreSQL
                error!("CREATE TVIEW failed: {}", e);
            }
        }
    }

    // Handle DROP TVIEW
    if query_upper.starts_with("DROP TVIEW") {
        match handle_drop_tview_safe(query_str) {
            Ok(_) => {
                unsafe {
                    if !completion_tag.is_null() {
                        (*completion_tag).commandTag = pg_sys::CommandTag::CMDTAG_UNKNOWN;
                        (*completion_tag).nprocessed = 0;
                    }
                }
                return;
            }
            Err(e) => {
                error!("DROP TVIEW failed: {}", e);
            }
        }
    }

    // Pass through to previous hook or standard processing
    // SAFETY: PREV_PROCESS_UTILITY_HOOK is set during init, never modified after
    unsafe {
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            prev_hook(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                completion_tag,
            );
        } else {
            pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                completion_tag,
            );
        }
    }
}

/// Safe wrapper around CREATE TVIEW handling (can return errors)
fn handle_create_tview_safe(query: &str) -> TViewResult<()> {
    // Parse CREATE TVIEW statement
    let parsed = parse_create_tview(query)?;

    // Create the TVIEW in transaction
    // If this fails, PostgreSQL will ROLLBACK automatically
    crate::ddl::create_tview(&parsed.tview_name, &parsed.select_sql)?;

    notice!("TVIEW {} created successfully", parsed.tview_name);

    Ok(())
}

fn handle_drop_tview_safe(query: &str) -> TViewResult<()> {
    let parsed = parse_drop_tview(query)?;

    crate::ddl::drop_tview(&parsed.tview_name, parsed.if_exists)?;

    notice!("TVIEW {} dropped", parsed.tview_name);

    Ok(())
}
