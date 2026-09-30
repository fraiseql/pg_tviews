//! `ProcessUtility` Hooks: DDL Interception and Transaction Management
//!
//! This module implements `PostgreSQL` hooks for DDL statement interception:
//! - **`ProcessUtility` Hook**: Intercepts CREATE TABLE `tv_*` and DROP TABLE `tv_*` statements
//! - **Transaction Callbacks**: Handles PREPARE/COMMIT/ABORT events
//! - **COMMIT / PREPARE TRANSACTION**: Flushes the refresh queue before the transaction ends
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

/// Transaction nest level at which `HOOK_IN_PROGRESS` was set, so a (sub)transaction abort
/// that unwinds past the hook can release a guard the hook never got to reset.
static mut HOOK_GUARD_LEVEL: i32 = 0;

/// Install the `ProcessUtility` hook to intercept CREATE/DROP TABLE `tv_*`
/// Install the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
pub unsafe fn install_hook() {
    // SAFETY: install_hook is called during extension load, modifying global PostgreSQL
    // hook pointers. The hook is a valid function pointer matching the C signature.
    unsafe {
        PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
    }
}

/// Check if hook is installed, install it if not
/// This is called lazily to avoid issues during postmaster startup
pub unsafe fn ensure_hook_installed() {
    // SAFETY: Called lazily from PostgreSQL backend context. Modifies static to track
    // initialization state.
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
            );
        };
        return;
    }

    // DO blocks and CALL run arbitrary user code (which may itself issue CREATE TABLE
    // tv_* AS, DROP TABLE tv_*, …). They are not statements this hook handles, so run them
    // without holding the reentrancy guard: otherwise every nested user statement would
    // take the shortcut above, meant for pg_tviews' own internal DDL (issue #80).
    if !pstmt.is_null() && unsafe { !(*pstmt).utilityStmt.is_null() } {
        let tag = unsafe { (*(*pstmt).utilityStmt).type_ };
        if tag == pg_sys::NodeTag::T_DoStmt || tag == pg_sys::NodeTag::T_CallStmt {
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
            };
            return;
        }
    }

    unsafe {
        HOOK_IN_PROGRESS = true;
        HOOK_GUARD_LEVEL = pg_sys::GetCurrentTransactionNestLevel();
    };

    // Check for COMMIT/END BEFORE the catch_unwind block.
    // flush_refresh_queue() uses SPI which may trigger PostgreSQL ereport(ERROR)
    // → longjmp → pgrx panic. This MUST NOT be caught by catch_unwind because
    // that corrupts PG_exception_stack. The #[pg_guard] on this function handles
    // error propagation correctly via C-unwind.
    //
    // Top level only: a COMMIT issued inside a procedure (`CALL`) has its own semantics and
    // used to be skipped by the reentrancy guard the enclosing statement held.
    if !pstmt.is_null()
        && unsafe { !(*pstmt).utilityStmt.is_null() }
        && context == pg_sys::ProcessUtilityContext::PROCESS_UTILITY_TOPLEVEL
    {
        let utility_stmt = unsafe { (*pstmt).utilityStmt };
        if unsafe { (*utility_stmt).type_ } == pg_sys::NodeTag::T_TransactionStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → TransactionStmt* cast
            let xact_stmt = utility_stmt.cast::<pg_sys::TransactionStmt>();
            if !xact_stmt.is_null() {
                let kind = unsafe { (*xact_stmt).kind };

                // Flush before COMMIT and before PREPARE TRANSACTION (issue #59): the
                // refresh writes then belong to this transaction, so a prepared
                // transaction applies or discards them with COMMIT / ROLLBACK PREPARED.
                let ending = if kind == pg_sys::TransactionStmtKind::TRANS_STMT_COMMIT {
                    Some("COMMIT")
                } else if kind == pg_sys::TransactionStmtKind::TRANS_STMT_PREPARE {
                    Some("PREPARE TRANSACTION")
                } else {
                    None
                };
                if let Some(stmt) = ending {
                    if let Err(e) = crate::queue::flush_refresh_queue() {
                        unsafe { HOOK_IN_PROGRESS = false };
                        error!("TVIEW refresh failed before {stmt}: {e:?}");
                    }
                    if let Err(e) = crate::audit::flush_audit_buffer() {
                        unsafe { HOOK_IN_PROGRESS = false };
                        error!("Audit flush failed before {stmt}: {e:?}");
                    }
                }
            }
        }
    }

    // A column rename is applied to TVIEW metadata once PostgreSQL has run it (issue #81).
    let column_rename = unsafe { column_rename_of(pstmt) };

    // Wrap FFI callback in catch_unwind to prevent panics crossing FFI boundary
    // Returns true if the hook handled the statement, false if it should pass through
    let result = std::panic::catch_unwind(|| -> Result<bool, TViewError> {
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

        // Skip extension statements to avoid infinite recursion during installation. This
        // is decided from the statement's node type, not the query text: in a multi-statement
        // batch the text is the whole batch, so a text test would also skip every other
        // statement in it (issue #80).
        if let Some(extensions) = unsafe { extension_statement_names(utility_stmt) } {
            // Invalidate the jsonb_delta availability latch first, so a backend that
            // cached availability re-checks on its next refresh after CREATE/DROP
            // EXTENSION jsonb_delta (issue #50). This is a pair of atomic stores — no
            // SPI — and runs before the pass-through so the stale value cannot survive.
            if extensions.iter().any(|e| e == "jsonb_delta") {
                crate::lifecycle::invalidate_jsonb_delta_cache();
            }
            return Ok(false); // Pass through
        }

        // Check for CREATE TABLE AS
        if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
            let ctas = utility_stmt.cast::<pg_sys::CreateTableAsStmt>();
            match unsafe { handle_create_table_as(ctas, pstmt, query_string) } {
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

        // Check for ALTER TABLE
        if node_tag == pg_sys::NodeTag::T_AlterTableStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → AlterTableStmt* cast
            let alter_stmt = utility_stmt.cast::<pg_sys::AlterTableStmt>();
            match unsafe { handle_alter_table(alter_stmt, query_string) } {
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
            // Something unwound out of the handler. Reset the guard BEFORE re-raising
            // so subsequent statements in this session are still intercepted.
            unsafe { HOOK_IN_PROGRESS = false };

            // A PostgreSQL `ereport(ERROR)` raised by SPI inside the handler (e.g.
            // "cannot drop ... because other objects depend on it") is converted by
            // pgrx into a Rust panic carrying a `CaughtError`; a Rust-side `error!()`
            // carries an `ErrorReportWithLevel`. Both are legitimate, actionable errors
            // — not pg_tviews bugs — so re-raise them faithfully (preserving message,
            // detail, hint, and SQLSTATE) rather than degrading to the opaque
            // "Any { .. }" message.
            let panic_info = match panic_info.downcast::<pg_sys::panic::CaughtError>() {
                Ok(caught) => caught.rethrow(),
                Err(panic_info) => panic_info,
            };
            let panic_info = match panic_info.downcast::<pg_sys::panic::ErrorReportWithLevel>() {
                Ok(report) => pg_sys::panic::CaughtError::ErrorReport(*report).rethrow(),
                Err(panic_info) => panic_info,
            };

            // A genuine Rust panic — this really is a bug in pg_tviews.
            let panic_msg = panic_info
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| panic_info.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| format!("{panic_info:?}"));
            error!(
                "PANIC in ProcessUtility hook: {panic_msg} - This is a bug in pg_tviews - please report it!"
            );
            #[allow(unreachable_code)]
            // Reason: rethrow()/error!() diverge via longjmp, not Rust's !
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

        // After the event trigger should have fired, drain any pending populatesand convert
        // any TVIEWs that weren't converted by the event trigger (fallback for bulk operations).
        // The INSERT runs via SPI (DML, not utility), so it does not re-enter
        // ProcessUtility and there is no reentrancy issue with HOOK_IN_PROGRESS.
        drain_pending_populates();
        drain_pending_unconverted_tviews();

        // Like the drains above, this runs outside catch_unwind: its SPI errors must
        // abort the RENAME rather than leave a TVIEW with stale metadata.
        if let Some((relid, old_name, new_name)) = column_rename.and_then(ColumnRename::resolve)
            && let Err(e) = crate::ddl::rename::handle_column_rename(relid, &old_name, &new_name)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not follow the column rename: {e}");
        }
    }

    // Release the reentrancy guard
    unsafe { HOOK_IN_PROGRESS = false };
}

/// An `ALTER … RENAME COLUMN` statement, captured before it runs.
struct ColumnRename {
    relation: *const pg_sys::RangeVar,
    old_name: String,
    new_name: String,
}

impl ColumnRename {
    /// The renamed relation's OID and the old/new names, once the rename has run.
    /// `None` if the relation is gone (`IF EXISTS` on a missing table).
    fn resolve(self) -> Option<(pg_sys::Oid, String, String)> {
        // SAFETY: `relation` points into the statement's parse tree, which lives
        // until the utility statement finishes.
        let relid = unsafe {
            pg_sys::RangeVarGetRelidExtended(
                self.relation,
                pg_sys::NoLock.cast_signed(),
                pg_sys::RVROption::RVR_MISSING_OK,
                None,
                std::ptr::null_mut(),
            )
        };
        (relid != pg_sys::InvalidOid).then_some((relid, self.old_name, self.new_name))
    }
}

/// The column rename carried by `pstmt`, if it is one.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
unsafe fn column_rename_of(pstmt: *const pg_sys::PlannedStmt) -> Option<ColumnRename> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        if (*node).type_ != pg_sys::NodeTag::T_RenameStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → RenameStmt* cast
        let stmt = &*node.cast::<pg_sys::RenameStmt>();
        if stmt.renameType != pg_sys::ObjectType::OBJECT_COLUMN
            || stmt.relation.is_null()
            || stmt.subname.is_null()
            || stmt.newname.is_null()
        {
            return None;
        }
        Some(ColumnRename {
            relation: stmt.relation,
            old_name: CStr::from_ptr(stmt.subname).to_string_lossy().into_owned(),
            new_name: CStr::from_ptr(stmt.newname).to_string_lossy().into_owned(),
        })
    }
}

/// Handle CREATE TABLE tv_* AS SELECT ...
///
/// Returns `Ok(true)` if the hook handled the statement, `Ok(false)` if it should
/// pass through. Returns `Err` on failures that should abort with `error!()` —
/// the caller is responsible for resetting `HOOK_IN_PROGRESS` before raising.
///
/// SAFETY: This function operates on raw `PostgreSQL` C pointers from the `ProcessUtility` hook.
/// All pointers are validated with null checks before dereferencing.
unsafe fn handle_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    pstmt: *const pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // SAFETY: All pointer dereferences are guarded by null checks above each use.
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

        // TEST ONLY: simulate a hook that never saw this statement (see the
        // missed-interception check in the event trigger).
        if crate::config::test_skip_ctas_intercept() {
            return Ok(false);
        }

        // Resolve the target relation BEFORE PostgreSQL runs the statement (catalog
        // lookup, no SPI). `IF NOT EXISTS` on an existing relation makes PostgreSQL skip
        // the create, so nothing would consume a pending SELECT (issue #79): pass through
        // untouched. Otherwise remember the pre-existing OID so the fallback drain can
        // tell "created by this statement" from "was already there".
        let pre_existing_oid = resolve_relation_oid(into.rel);
        if ctas_ref.if_not_exists && pre_existing_oid != pg_sys::InvalidOid {
            return Ok(false);
        }

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
            // `query_string` is the whole simple-query batch; slice out just this
            // statement, then strip its `CREATE TABLE … AS` prefix (issue #95).
            let stmt_sql = statement_text(sql, pstmt);
            extract_ctas_select(stmt_sql, table_name).ok_or_else(|| {
                TViewError::InvalidSelectStatement {
                    sql: stmt_sql.to_string(),
                    reason: format!("Could not find 'CREATE TABLE {table_name} AS' in query"),
                }
            })?
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
                record_pre_existing_oid(table_name, pre_existing_oid);

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
                record_pre_existing_oid(table_name, pre_existing_oid);
                Ok(false) // Let PostgreSQL create it, event trigger will convert
            }
        }
    }
}

/// The text of the single statement `pstmt` covers within a (possibly multi-statement)
/// `query_string`, using `stmt_location` / `stmt_len` (`-1` / `0` mean "unknown" and
/// "to the end of the string"). Falls back to the whole string if the range is invalid.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt*`.
unsafe fn statement_text(query_string: &str, pstmt: *const pg_sys::PlannedStmt) -> &str {
    if pstmt.is_null() {
        return query_string;
    }
    let (location, len) = unsafe { ((*pstmt).stmt_location, (*pstmt).stmt_len) };
    let Ok(start) = usize::try_from(location) else {
        return query_string;
    };
    let end = match usize::try_from(len) {
        Ok(len) if len > 0 => start.saturating_add(len),
        _ => query_string.len(),
    };
    query_string
        .get(start..end.min(query_string.len()))
        .unwrap_or(query_string)
}

/// Extract the SELECT from the text of one `CREATE TABLE [schema.]tv_x AS SELECT …`
/// statement: everything after the `AS` that follows the table name, without a trailing `;`.
///
/// Anchored at the statement start so an earlier occurrence of the table name (a comment,
/// another statement) can't be matched.
fn extract_ctas_select(stmt_sql: &str, table_name: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(
        r#"(?is)^\s*create\s+(?:[a-z]+\s+){{0,2}}?table\s+(?:if\s+not\s+exists\s+)?(?:"?[^\s."]+"?\s*\.\s*)?"?{}"?\s+as\s+"#,
        regex::escape(table_name)
    ))
    .ok()?;
    let m = re.find(stmt_sql)?;
    let select = stmt_sql[m.end()..].trim().trim_end_matches(';').trim();
    (!select.is_empty()).then(|| select.to_string())
}

/// If `node` is `CREATE EXTENSION` or `DROP EXTENSION`, the extension name(s) it names.
///
/// SAFETY: `node` must be a valid, non-null `Node*`.
unsafe fn extension_statement_names(node: *mut pg_sys::Node) -> Option<Vec<String>> {
    unsafe {
        let tag = (*node).type_;
        if tag == pg_sys::NodeTag::T_CreateExtensionStmt {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: PostgreSQL Node* → CreateExtensionStmt* cast
            let stmt = node.cast::<pg_sys::CreateExtensionStmt>();
            let name = if (*stmt).extname.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*stmt).extname)
                    .to_string_lossy()
                    .into_owned()
            };
            return Some(vec![name]);
        }
        if tag == pg_sys::NodeTag::T_DropStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
            let stmt = node.cast::<pg_sys::DropStmt>();
            if (*stmt).removeType != pg_sys::ObjectType::OBJECT_EXTENSION {
                return None;
            }
            let mut names = Vec::new();
            let objects = (*stmt).objects;
            for i in 0..pg_sys::list_length(objects) {
                let item = pg_sys::list_nth(objects, i).cast::<pg_sys::String>();
                if !item.is_null() && !(*item).sval.is_null() {
                    names.push(CStr::from_ptr((*item).sval).to_string_lossy().into_owned());
                }
            }
            return Some(names);
        }
        None
    }
}

/// Validate TVIEW SELECT statement structure
fn validate_tview_select(select_sql: &str) -> Result<(), String> {
    // Check for required patterns in SELECT
    // This is basic validation - event trigger will do thorough validation
    // Only require: id (UUID) + data (JSONB)
    // Optional columns: pk_<entity>, fk_<entity>, path (LTREE), <entity>_id (UUID FKs)

    let sql_lower = select_sql.to_lowercase();

    // Early return for SELECT * - defer validation to event trigger
    if let Some(pos) = sql_lower.find("select") {
        let after = &sql_lower[pos + 6..].trim_start();
        if after.starts_with('*') {
            return Ok(());
        }
    }

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

/// OID the target relation had before the CTAS ran (`InvalidOid` when it didn't exist).
///
/// Maps: `table_name` → OID. Lets [`drain_pending_unconverted_tviews`] refuse to drop a
/// relation that the statement did not create (issue #79).
static PENDING_PRE_EXISTING_OIDS: LazyLock<Mutex<std::collections::HashMap<String, pg_sys::Oid>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn record_pre_existing_oid(table_name: &str, oid: pg_sys::Oid) {
    if let Ok(mut map) = PENDING_PRE_EXISTING_OIDS.lock() {
        map.insert(table_name.to_string(), oid);
    }
}

/// Resolve a `RangeVar` to a relation OID without locking or raising.
///
/// Returns `InvalidOid` when the relation (or its schema) doesn't exist. This is a
/// direct catalog lookup: no SPI, so it is safe inside the hook's `catch_unwind`.
///
/// SAFETY: `rv` must be null or a valid `RangeVar*`.
unsafe fn resolve_relation_oid(rv: *const pg_sys::RangeVar) -> pg_sys::Oid {
    if rv.is_null() {
        return pg_sys::InvalidOid;
    }
    unsafe {
        pg_sys::RangeVarGetRelidExtended(
            rv,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        )
    }
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

/// Forget every pending CTAS SELECT (and its pre-existing-OID record).
///
/// Called on (sub)transaction abort: a pending entry only lives from the hook storing it to
/// the event trigger consuming it within one statement, so anything left over belongs to a
/// statement that failed and must not be applied to a later, unrelated table of the same
/// name. Pure in-memory work — safe inside transaction callbacks (no SPI).
pub fn discard_pending_ctas() {
    if let Ok(mut pending) = PENDING_TVIEW_SELECTS.lock() {
        pending.clear();
    }
    if let Ok(mut oids) = PENDING_PRE_EXISTING_OIDS.lock() {
        oids.clear();
    }
}

/// Release the reentrancy guard if the hook invocation that took it was aborted.
///
/// An `ereport(ERROR)` raised while the hook runs a statement (e.g. a CTAS inside a DO
/// block failing with "already exists", caught by the block's EXCEPTION clause) longjmps
/// past the hook before it can reset `HOOK_IN_PROGRESS`. The guard is released here when
/// the aborting (sub)transaction is at or above the level that took it; a guard taken by an
/// enclosing, still-running hook invocation (lower level) is left alone.
/// `whole_xact` is true for a top-level transaction abort. No SPI: safe in callbacks.
pub fn release_hook_guard_on_abort(whole_xact: bool) {
    // SAFETY: single-threaded backend; plain reads/writes of process-local statics.
    unsafe {
        if HOOK_IN_PROGRESS
            && (whole_xact || pg_sys::GetCurrentTransactionNestLevel() <= HOOK_GUARD_LEVEL)
        {
            HOOK_IN_PROGRESS = false;
        }
    }
}

/// Retrieve and remove a pending TVIEW `(schema_name, SELECT)` pair.
///
/// Called by event trigger to get the original SELECT and target schema for TVIEW
/// conversion.  Returns `None` if no entry was stored for this table (which means the
/// table was created by `pg_tviews_create()` directly, not via DDL interception).
pub fn take_pending_tview_select(table_name: &str) -> Option<(String, String)> {
    if let Ok(mut map) = PENDING_PRE_EXISTING_OIDS.lock() {
        map.remove(table_name);
    }
    PENDING_TVIEW_SELECTS.lock().ok()?.remove(table_name)
}

/// Pending initial-data population requests deferred from the event trigger.
///
/// When `create_tview` is called from the `ddl_command_end` event trigger (CTAS path),
/// the INSERT that populates the materialized table silently loses its effects due to
/// sub-transaction depth corruption.  Instead, `create_tview` enqueues the populate
/// request here and the `ProcessUtility` hook drains the queue **after** the event
/// trigger returns, in a clean SPI context.
static PENDING_POPULATES: LazyLock<Mutex<Vec<PendingPopulate>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

struct PendingPopulate {
    tv_table_name: String,
    view_name: String,
    schema_name: String,
}

/// Enqueue a deferred initial-data population for a TVIEW created via CTAS.
///
/// Called by `create_tview` when `defer_populate` is `true`.
pub fn enqueue_pending_populate(tv_table_name: &str, view_name: &str, schema_name: &str) {
    if let Ok(mut queue) = PENDING_POPULATES.lock() {
        queue.push(PendingPopulate {
            tv_table_name: tv_table_name.to_string(),
            view_name: view_name.to_string(),
            schema_name: schema_name.to_string(),
        });
    }
}

/// Drain and convert any TVIEW tables that weren't converted by the event trigger.
///
/// Fallback for statements whose event trigger did not run (for example
/// `SET event_triggers = off`, or the trigger missing from the database). A normal CTAS is
/// consumed by the event trigger, so the cache is empty here. Anything still pending is
/// converted directly, but only for a relation this statement created (see
/// [`created_by_this_statement`]).
fn drain_pending_unconverted_tviews() {
    // Get all pending unconverted TVIEWs
    let entries: Vec<(String, String, String)> = PENDING_TVIEW_SELECTS
        .lock()
        .map(|mut cache| {
            cache
                .drain()
                .map(|(table_name, (schema_name, select_sql))| {
                    (table_name, schema_name, select_sql)
                })
                .collect()
        })
        .unwrap_or_default();

    if entries.is_empty() {
        return; // No unconverted TVIEWs, event trigger must have fired
    }

    // Log that we're using the fallback mechanism
    notice!(
        "pg_tviews: Event trigger did not fire for {} TVIEW(s), using fallback conversion",
        entries.len()
    );

    let pre_existing: std::collections::HashMap<String, pg_sys::Oid> = PENDING_PRE_EXISTING_OIDS
        .lock()
        .map(|mut map| std::mem::take(&mut *map))
        .unwrap_or_default();

    // Convert each TVIEW directly in this context
    for (table_name, schema_name, select_sql) in entries {
        if !created_by_this_statement(
            &table_name,
            &schema_name,
            pre_existing.get(&table_name).copied(),
        ) {
            continue;
        }

        notice!(
            "pg_tviews: Fallback converting TVIEW table '{}' (schema: '{}')",
            table_name,
            if schema_name.is_empty() {
                "(current_schema)"
            } else {
                &schema_name
            }
        );

        // Resolve the target schema
        let schema_override: Option<&str> = if schema_name.is_empty() {
            None
        } else {
            Some(schema_name.as_str())
        };

        // Drop the regular table PostgreSQL created and replace it with TVIEW semantics
        let drop_sql = match schema_override {
            Some(s) => format!(
                "DROP TABLE IF EXISTS {}.{} CASCADE",
                crate::utils::quote_identifier(s),
                crate::utils::quote_identifier(&table_name),
            ),
            None => format!(
                "DROP TABLE IF EXISTS {} CASCADE",
                crate::utils::quote_identifier(&table_name)
            ),
        };

        if let Err(e) = Spi::run(&drop_sql) {
            warning!(
                "pg_tviews: Failed to drop table '{}' during fallback conversion: {e}",
                table_name
            );
            continue;
        }

        // Create the proper TVIEW: backing view, materialized table, triggers
        match crate::ddl::create_tview(&table_name, &select_sql, schema_override, true) {
            Ok(()) => {
                notice!(
                    "pg_tviews: Fallback conversion SUCCEEDED for TVIEW '{}'",
                    table_name
                );
            }
            Err(e) => {
                warning!(
                    "pg_tviews: Fallback conversion FAILED for TVIEW '{}': {e}",
                    table_name
                );
            }
        }
    }
}

/// Is the relation named by a pending CTAS one this statement created, and safe to replace?
///
/// The fallback conversion `DROP`s the plain table PostgreSQL made and rebuilds it as a
/// TVIEW. That is only correct for a table the statement just created. A pending entry can
/// outlive its statement (the CTAS was skipped by `IF NOT EXISTS`, or failed with
/// "already exists"), in which case the relation is a pre-existing table or a registered
/// TVIEW that must never be dropped (issue #79). Returns `false` (after warning) then.
fn created_by_this_statement(
    table_name: &str,
    schema_name: &str,
    pre_existing: Option<pg_sys::Oid>,
) -> bool {
    let qualified = if schema_name.is_empty() {
        crate::utils::quote_identifier(table_name)
    } else {
        format!(
            "{}.{}",
            crate::utils::quote_identifier(schema_name),
            crate::utils::quote_identifier(table_name)
        )
    };
    let current: Option<pg_sys::Oid> = Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT to_regclass($1)::oid",
        &[qualified.as_str().into()],
    )
    .ok()
    .flatten();

    let Some(current) = current else {
        warning!("pg_tviews: table '{table_name}' not found for fallback conversion, skipping");
        return false;
    };
    if pre_existing == Some(current) {
        // Relation existed before the statement: it did not create anything.
        return false;
    }
    if matches!(
        crate::catalog::TviewMeta::load_for_tview(current),
        Ok(Some(_))
    ) {
        warning!("pg_tviews: '{table_name}' is a registered TVIEW, not replacing it");
        return false;
    }
    true
}

/// Drain and execute all pending TVIEW population requests.
///
/// Called by the `ProcessUtility` hook after `call_prev_hook_or_standard` returns
/// (the event trigger has completed).  Runs the INSERT via SPI in a clean context
/// outside the event trigger's sub-transaction scope.
fn drain_pending_populates() {
    let entries: Vec<PendingPopulate> = PENDING_POPULATES
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default();

    for entry in entries {
        let view_oid = match Spi::get_one::<pg_sys::Oid>(&format!(
            "SELECT c.oid FROM pg_class c JOIN pg_namespace n ON c.relnamespace = n.oid \
             WHERE c.relname::text = '{}' AND n.nspname::text = '{}'  AND c.relkind = 'v'",
            entry.view_name, entry.schema_name
        )) {
            Ok(Some(oid)) => oid,
            Ok(None) => {
                error!(
                    "pg_tviews: deferred populate failed — view {}.{} not found",
                    entry.schema_name, entry.view_name
                );
            }
            Err(e) => {
                error!(
                    "pg_tviews: deferred populate failed — cannot resolve view {}.{}: {e}",
                    entry.schema_name, entry.view_name
                );
            }
        };

        let view_columns = match crate::utils::get_view_columns_by_oid(view_oid) {
            Ok(cols) if !cols.is_empty() => cols,
            Ok(_) => {
                error!(
                    "pg_tviews: deferred populate failed — view {}.{} has no columns",
                    entry.schema_name, entry.view_name
                );
            }
            Err(e) => {
                error!(
                    "pg_tviews: deferred populate failed — cannot get columns for {}.{}: {e}",
                    entry.schema_name, entry.view_name
                );
            }
        };

        let qi_schema = crate::utils::quote_identifier(&entry.schema_name);
        let qi_tview = crate::utils::quote_identifier(&entry.tv_table_name);
        let qi_view = crate::utils::quote_identifier(&entry.view_name);
        let col_list = view_columns
            .iter()
            .map(|c| crate::utils::quote_identifier(c))
            .collect::<Vec<_>>()
            .join(", ");

        let insert_sql = format!(
            "INSERT INTO {qi_schema}.{qi_tview} ({col_list}) \
             SELECT {col_list} FROM {qi_schema}.{qi_view}"
        );

        if let Err(e) = Spi::run(&insert_sql) {
            error!(
                "pg_tviews: deferred populate failed for {}: {e}",
                entry.tv_table_name
            );
        }
    }
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
///
/// SAFETY: This function operates on raw `PostgreSQL` C pointers from the `ProcessUtility` hook.
/// All pointers are validated with null checks before dereferencing.
unsafe fn handle_drop_table(
    drop_stmt: *mut pg_sys::DropStmt,
    _query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // SAFETY: All pointer dereferences are guarded by null checks.
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

        // Honor the statement's CASCADE/RESTRICT behavior. Without this the internal
        // drop was always RESTRICT, so `DROP TABLE tv_* CASCADE` on a TVIEW with
        // dependents raised a dependency error that surfaced as an opaque panic.
        let cascade = drop_ref.behavior == pg_sys::DropBehavior::DROP_CASCADE;

        // Collect registered TVIEWs from DropStmt.objects. Each element is a List* of
        // String* name parts ([schema, table] or [table]). A name is claimed only if it
        // resolves (schema-aware, like PostgreSQL) to a relation registered in
        // `pg_tview_meta`; everything else — plain `tv_*` tables, missing names, other
        // tables — is left in the list for the standard handler (issue #82).
        let num_tables = pg_sys::list_length(objects);
        let mut tv_entries: Vec<(i32, String)> = Vec::new(); // (index, tv_<entity>)
        let mut has_non_tv = false;

        for i in 0..num_tables {
            let name_list = pg_sys::list_nth(objects, i).cast::<pg_sys::List>();
            if name_list.is_null() || pg_sys::list_length(name_list) == 0 {
                has_non_tv = true;
                continue;
            }

            let rv = pg_sys::makeRangeVarFromNameList(name_list);
            let relid = resolve_relation_oid(rv);
            if relid == pg_sys::InvalidOid {
                has_non_tv = true;
                continue;
            }

            match crate::catalog::TviewMeta::load_for_tview(relid) {
                Ok(Some(meta)) => tv_entries.push((i, format!("tv_{}", meta.entity_name))),
                _ => has_non_tv = true,
            }
        }

        if tv_entries.is_empty() {
            return Ok(false);
        }

        // Drop each registered TVIEW via drop_tview
        for (_, name) in &tv_entries {
            drop_tview(name, if_exists, cascade)?;
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

/// Handle ALTER TABLE statements on TVIEW tables
///
/// SAFETY: This function operates on raw `PostgreSQL` C pointers from the `ProcessUtility` hook.
/// All pointers are validated with null checks before dereferencing.
unsafe fn handle_alter_table(
    alter_stmt: *mut pg_sys::AlterTableStmt,
    _query_string: *const ::std::os::raw::c_char,
) -> Result<bool, TViewError> {
    // SAFETY: All pointer dereferences are guarded by null checks.
    unsafe {
        if alter_stmt.is_null() {
            return Ok(false);
        }

        let alter_ref = &*alter_stmt;

        // Get the table name
        let relation = alter_ref.relation;
        if relation.is_null() {
            return Ok(false);
        }

        let rel_ref = &*relation;
        let table_name_cstr = rel_ref.relname;
        if table_name_cstr.is_null() {
            return Ok(false);
        }

        let table_name = CStr::from_ptr(table_name_cstr).to_str().unwrap_or("");

        // Check if it's a TVIEW table (starts with tv_)
        if !table_name.starts_with("tv_") {
            return Ok(false);
        }

        // Check the ALTER TABLE commands for SET UNLOGGED/LOGGED
        let cmds = alter_ref.cmds;
        if cmds.is_null() {
            return Ok(false);
        }

        let num_cmds = pg_sys::list_length(cmds);
        for i in 0..num_cmds {
            let cmd_node = pg_sys::list_nth(cmds, i);
            if cmd_node.is_null() {
                continue;
            }

            let cmd = cmd_node.cast::<pg_sys::AlterTableCmd>();
            if cmd.is_null() {
                continue;
            }

            let cmd_ref = &*cmd;

            // Check for SET UNLOGGED or SET LOGGED
            if cmd_ref.subtype == pg_sys::AlterTableType::AT_SetUnLogged {
                // SET UNLOGGED - data is preserved, no special handling needed
                return Ok(false); // Let PostgreSQL handle it normally
            } else if cmd_ref.subtype == pg_sys::AlterTableType::AT_SetLogged {
                // SET LOGGED preserves data — no special handling needed
                return Ok(false); // Let PostgreSQL handle it normally
            }
        }

        // Not a SET UNLOGGED/LOGGED command we care about
        Ok(false)
    }
}

/// Call the previous hook if it exists, otherwise call `standard_ProcessUtility`
///
/// SAFETY: This calls into either a previous `PostgreSQL` hook or `standard_ProcessUtility`.
/// The parameters are raw C pointers from the calling `ProcessUtility` hook.
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
    // SAFETY: Delegates to PostgreSQL internal hook or standard utility handler.
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

#[cfg(test)]
mod ctas_extraction_tests {
    use super::extract_ctas_select;

    #[test]
    fn extracts_select_and_strips_semicolon() {
        let sql = "CREATE TABLE tv_post AS SELECT pk_post, id, data FROM tb_post;";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT pk_post, id, data FROM tb_post")
        );
    }

    #[test]
    fn handles_schema_if_not_exists_and_case() {
        let sql = "create table if not exists public.tv_post as\n  select 1";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("select 1")
        );
    }

    #[test]
    fn ignores_earlier_occurrences_of_the_name() {
        let sql = "/* tv_post as */ CREATE TABLE tv_post AS SELECT 'tv_post as x' FROM t";
        // Anchored at the statement start: a leading comment is not a CTAS prefix.
        assert_eq!(extract_ctas_select(sql, "tv_post"), None);
        let sql = "CREATE TABLE tv_post AS SELECT 'tv_post as x' FROM t";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 'tv_post as x' FROM t")
        );
    }

    #[test]
    fn does_not_match_a_different_table() {
        let sql = "CREATE TABLE tv_post_extra AS SELECT 1";
        assert_eq!(extract_ctas_select(sql, "tv_post"), None);
    }

    #[test]
    fn handles_quoted_names() {
        let sql = "CREATE TABLE \"s\".\"tv_post\" AS SELECT 1";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 1")
        );
    }
}
