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

mod ctas;
mod statements;
mod tables;

use ctas::{
    CTAS_HINT, Ctas, CtasTarget, create_tview_from_ctas, inspect_create_table_as, skipped_or_raise,
    tview_ctas_target, utility_of,
};
use statements::{
    ColumnRename, PartitionDdl, PrivilegesChange, column_rename_of, drops_pg_tviews,
    extension_installed, extension_statement_names, matview_refresh_of, partition_ddl_of,
    privileges_change_of, resolve_relation_oid, table_move_of,
};
use tables::{handle_alter_table, handle_drop_table};

use crate::TViewError;
use crate::ddl::drop_tview;
use crate::error::TViewResult;

/// Previous `ProcessUtility` hook (if any other extension installed one)
static mut PREV_PROCESS_UTILITY_HOOK: pg_sys::ProcessUtility_hook_type = None;

/// Install the `ProcessUtility` hook that intercepts `tv_*` DDL.
pub unsafe fn install_hook() {
    // SAFETY: install_hook is called during extension load, modifying global PostgreSQL
    // hook pointers. The hook is a valid function pointer matching the C signature.
    unsafe {
        PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(tview_process_utility_hook);
        crate::executor::install_hooks();
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

/// The arguments of a `ProcessUtility` call, passed on unchanged.
#[derive(Clone, Copy)]
struct UtilityCall {
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
}

impl UtilityCall {
    /// Run the statement: the previous hook, or `PostgreSQL`'s own handler.
    fn pass_through(self) {
        // SAFETY: the arguments `PostgreSQL` gave the hook, unchanged but for a
        // copied statement.
        unsafe {
            call_prev_hook_or_standard(
                self.pstmt,
                self.query_string,
                self.read_only_tree,
                self.context,
                self.params,
                self.query_env,
                self.dest,
                self.qc,
            );
        }
    }

    /// The utility statement's node tag, if there is one.
    fn tag(self) -> Option<pg_sys::NodeTag> {
        // SAFETY: null-checked pointers from the hook.
        unsafe {
            (!self.pstmt.is_null() && !(*self.pstmt).utilityStmt.is_null())
                .then(|| (*(*self.pstmt).utilityStmt).type_)
        }
    }
}

/// `ProcessUtility` hook: intercepts `CREATE TABLE tv_* AS`, `DROP TABLE tv_*` and
/// `ALTER TABLE tv_*`, flushes the refresh queue before `COMMIT` and `PREPARE
/// TRANSACTION`, and follows the DDL that changes what TVIEWs read (column
/// renames, partitions, table moves, privileges, `DROP EXTENSION`, materialized
/// view refreshes).
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
    let mut call = UtilityCall {
        pstmt,
        query_string,
        read_only_tree,
        context,
        params,
        query_env,
        dest,
        qc,
    };

    // Number every TRUNCATE, nested ones included, so its truncate triggers
    // refresh each TVIEW once (one fires per truncated partition).
    if call.tag() == Some(pg_sys::NodeTag::T_TruncateStmt) {
        crate::delta::begin_truncate();
    }

    // DDL pg_tviews itself issues: run it as it is.
    if crate::internal_ddl::InternalDdl::active() {
        call.pass_through();
        return;
    }

    let pass_through = {
        let _internal = crate::internal_ddl::InternalDdl::begin();
        flush_before_transaction_end(call);
        // SAFETY: the hook's statement.
        let follow_ups = unsafe { FollowUps::of(call.pstmt) };
        let pass_through = handle(decide(call), &mut call);
        pass_through.then_some(follow_ups)
    };
    // The user's statement runs without the guard: DDL nested in it (a function
    // it calls) is intercepted like any other.
    if let Some(follow_ups) = pass_through {
        call.pass_through();
        let _internal = crate::internal_ddl::InternalDdl::begin();
        follow_ups.run();
    }
}

/// Flush the refresh queue before a top-level `COMMIT` or `PREPARE TRANSACTION`:
/// the refresh writes then belong to the transaction, so a prepared
/// transaction applies or discards them with `COMMIT` / `ROLLBACK PREPARED`. A
/// `COMMIT` inside a procedure has its own semantics; a failed transaction block
/// ends in `ROLLBACK` whatever the client says, with nothing to refresh.
///
/// Runs outside `catch_unwind`: `PostgreSQL` errors raised by the flush must
/// propagate as they are.
fn flush_before_transaction_end(call: UtilityCall) {
    if call.context != pg_sys::ProcessUtilityContext::PROCESS_UTILITY_TOPLEVEL
        || call.tag() != Some(pg_sys::NodeTag::T_TransactionStmt)
    {
        return;
    }
    // SAFETY: a TransactionStmt, by its tag.
    #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → TransactionStmt* cast
    let kind = unsafe { (*(*call.pstmt).utilityStmt.cast::<pg_sys::TransactionStmt>()).kind };
    let stmt = match kind {
        pg_sys::TransactionStmtKind::TRANS_STMT_COMMIT => "COMMIT",
        pg_sys::TransactionStmtKind::TRANS_STMT_PREPARE => "PREPARE TRANSACTION",
        _ => return,
    };
    // SAFETY: reads of backend transaction state.
    if !unsafe { pg_sys::IsTransactionState() && !pg_sys::IsAbortedTransactionBlockState() } {
        return;
    }
    // Still suspended at the end of the transaction: resume and rebuild the
    // TVIEWs the suspended writes touched.
    if crate::suspend::is_suspended() {
        crate::suspend::force_resume();
        if let Err(e) = crate::suspend::catch_up() {
            e.raise_in(&format!(
                "TVIEW catch-up after suspension failed before {stmt}"
            ));
        }
    }
    if let Err(e) = crate::flush::flush_refresh_queue() {
        e.raise_in(&format!("TVIEW refresh failed before {stmt}"));
    }
    if let Err(e) = crate::audit::flush_audit_buffer() {
        e.raise_in(&format!("Audit flush failed before {stmt}"));
    }
}

/// What to do with the statement, decided without SPI inside `catch_unwind`, which
/// re-raises a `PostgreSQL` error caught there as it was.
fn decide(call: UtilityCall) -> Intercept {
    let result = std::panic::catch_unwind(|| -> Result<Intercept, TViewError> {
        let Some(node_tag) = call.tag() else {
            return Ok(Intercept::PassThrough);
        };
        // SAFETY: a utility statement, by `tag`.
        let utility_stmt = unsafe { (*call.pstmt).utilityStmt };

        // CREATE / DROP EXTENSION is decided from the node, not the query text: in
        // a multi-statement batch the text is the whole batch.
        if let Some(extensions) = unsafe { extension_statement_names(utility_stmt) } {
            // Forget the cached jsonb_delta schema, so this backend re-checks it on
            // its next refresh after CREATE/DROP EXTENSION jsonb_delta.
            if extensions.iter().any(|e| e == "jsonb_delta") {
                crate::cache::invalidate_all();
            }
            if extensions.iter().any(|e| e == "pg_tviews") {
                crate::revision::reset();
            }
            return Ok(Intercept::PassThrough);
        }
        // The library is preloaded cluster-wide: in a database without the
        // extension there is nothing to maintain.
        if !extension_installed() {
            return Ok(Intercept::PassThrough);
        }
        match node_tag {
            pg_sys::NodeTag::T_CreateTableAsStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
                let ctas = utility_stmt.cast::<pg_sys::CreateTableAsStmt>();
                unsafe { inspect_create_table_as(ctas, call.pstmt, call.query_string) }
            }
            // EXPLAIN [ANALYZE] CREATE TABLE tv_* AS … would run the CTAS without
            // this hook seeing it as one, nor the event trigger firing: refuse it.
            pg_sys::NodeTag::T_ExplainStmt => {
                #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → ExplainStmt* cast
                let query = unsafe { (*utility_stmt.cast::<pg_sys::ExplainStmt>()).query };
                Ok(match unsafe { tview_ctas_target(utility_of(query)) } {
                    Some(table) => Intercept::Refuse(
                        format!("EXPLAIN of CREATE TABLE {table} AS … cannot create a TVIEW"),
                        None,
                    ),
                    None => Intercept::PassThrough,
                })
            }
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
            pg_sys::NodeTag::T_DropStmt => Ok(Intercept::DropTable(
                utility_stmt.cast::<pg_sys::DropStmt>(),
            )),
            pg_sys::NodeTag::T_AlterTableStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → AlterTableStmt* cast
                let alter_stmt = utility_stmt.cast::<pg_sys::AlterTableStmt>();
                Ok(
                    if unsafe { handle_alter_table(alter_stmt, call.query_string) }? {
                        Intercept::Handled
                    } else {
                        Intercept::PassThrough
                    },
                )
            }
            _ => Ok(Intercept::PassThrough),
        }
    });
    match result {
        Ok(Ok(intercept)) => intercept,
        Ok(Err(e)) => e.raise(),
        Err(panic_info) => rethrow(panic_info),
    }
}

/// Re-raise what unwound out of `catch_unwind`. A `PostgreSQL` `ereport(ERROR)`
/// raised through SPI arrives as a `CaughtError`, a Rust-side `error!()` as an
/// `ErrorReportWithLevel`: legitimate errors, re-raised with their message,
/// detail, hint and SQLSTATE. Anything else is a bug in `pg_tviews`.
fn rethrow(panic_info: Box<dyn std::any::Any + Send>) -> ! {
    let panic_info = match panic_info.downcast::<pg_sys::panic::CaughtError>() {
        Ok(caught) => caught.rethrow(),
        Err(panic_info) => panic_info,
    };
    let panic_info = match panic_info.downcast::<pg_sys::panic::ErrorReportWithLevel>() {
        Ok(report) => pg_sys::panic::CaughtError::ErrorReport(*report).rethrow(),
        Err(panic_info) => panic_info,
    };
    let panic_msg = panic_info
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic_info.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| format!("{panic_info:?}"));
    error!(
        "PANIC in ProcessUtility hook: {panic_msg} - This is a bug in pg_tviews - please report it!"
    )
}

/// Carry out `intercept`; whether the statement still goes to `PostgreSQL`.
/// Errors are raised as they are (outside `catch_unwind`).
fn handle(intercept: Intercept, call: &mut UtilityCall) -> bool {
    match intercept {
        Intercept::PassThrough => true,
        Intercept::Handled => false,
        Intercept::Refuse(_, Some(target)) | Intercept::CreateTview(Ctas { target, .. })
            if skipped_or_raise(&target) =>
        {
            true
        }
        Intercept::Refuse(reason, _) => {
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_FEATURE_NOT_SUPPORTED,
                format!("pg_tviews: {reason}"),
                function_name!(),
            )
            .set_hint(CTAS_HINT)
            .report(PgLogLevel::ERROR);
            unreachable!("ERROR does not return")
        }
        Intercept::CreateTview(ctas) => {
            // SAFETY: the hook's completion record.
            unsafe { create_tview_from_ctas(&ctas, call.qc) };
            false
        }
        Intercept::DropTable(drop_stmt) => {
            // The TVIEWs are dropped here and taken out of the statement's list. A
            // read-only tree (a cached plan, run again by the next call) is copied
            // first, and the copy goes on to the standard handler.
            let drop_stmt = if call.read_only_tree {
                // SAFETY: a deep copy in the current memory context, which lives
                // for the statement.
                unsafe {
                    call.pstmt = pg_sys::copyObjectImpl(call.pstmt.cast()).cast();
                    #[allow(clippy::cast_ptr_alignment)]
                    // Reason: PostgreSQL Node* → DropStmt* cast
                    (*call.pstmt).utilityStmt.cast::<pg_sys::DropStmt>()
                }
            } else {
                drop_stmt
            };
            match unsafe { handle_drop_table(drop_stmt, call.query_string) } {
                Ok(handled) => !handled,
                Err(e) => e.raise(),
            }
        }
    }
}

/// What the statement changes that TVIEWs follow once `PostgreSQL` has run it,
/// read from the statement before it runs.
struct FollowUps {
    /// A column renamed: the definitions reading it follow.
    column_rename: Option<ColumnRename>,
    /// A partition added or removed: it gets or loses the triggers.
    partition_ddl: Option<PartitionDdl>,
    /// A TVIEW's table renamed or moved: its backing view's name follows.
    table_move: Option<pg_sys::Oid>,
    /// GRANT / REVOKE or an owner change: the backing views follow.
    privileges_change: Option<PrivilegesChange>,
    /// DROP EXTENSION `pg_tviews`: the backing views, read before the catalog goes,
    /// are dropped after it.
    extension_drop: Option<Vec<pg_sys::Oid>>,
    /// A materialized view refreshed: the TVIEWs refreshing in full on its
    /// changes are rebuilt.
    matview_refresh: Option<pg_sys::Oid>,
}

impl FollowUps {
    /// SAFETY: `pstmt` is null or the hook's valid statement.
    unsafe fn of(pstmt: *mut pg_sys::PlannedStmt) -> Self {
        if !extension_installed() {
            return Self {
                column_rename: None,
                partition_ddl: None,
                table_move: None,
                privileges_change: None,
                extension_drop: None,
                matview_refresh: None,
            };
        }
        // SAFETY: as above.
        let extension_drop = unsafe { drops_pg_tviews(pstmt) }.then(|| {
            crate::ddl::drop::backing_views().unwrap_or_else(|e| {
                e.raise_in("pg_tviews: could not read the backing views before DROP EXTENSION")
            })
        });
        // SAFETY: as above.
        unsafe {
            Self {
                column_rename: column_rename_of(pstmt),
                partition_ddl: partition_ddl_of(pstmt),
                table_move: table_move_of(pstmt),
                privileges_change: privileges_change_of(pstmt),
                extension_drop,
                matview_refresh: matview_refresh_of(pstmt),
            }
        }
    }

    /// Follow what the statement changed. SPI errors abort the statement rather
    /// than leave a TVIEW with stale metadata.
    fn run(self) {
        if let Some((relid, old_name, new_name)) =
            self.column_rename.and_then(ColumnRename::resolve)
            && let Err(e) = crate::ddl::rename::handle_column_rename(relid, &old_name, &new_name)
        {
            e.raise_in("pg_tviews: could not follow the column rename");
        }
        if let Some(ddl) = self.partition_ddl
            && let Err(e) = unsafe { ddl.apply() }
        {
            e.raise_in("pg_tviews: could not update the triggers of a partition");
        }
        if let Some(table) = self.table_move
            && let Err(e) = crate::ddl::follow_table_move(table)
        {
            e.raise_in("pg_tviews: could not rename the backing view of a moved TVIEW");
        }
        if let Some(PrivilegesChange { owners }) = self.privileges_change
            && let Err(e) = crate::ddl::privileges::follow(None, owners)
        {
            e.raise_in("pg_tviews: could not give the backing views their tables' privileges");
        }
        if let Some(views) = self.extension_drop
            && let Err(e) = crate::ddl::drop::drop_left_backing_views(&views)
        {
            e.raise_in("pg_tviews: could not drop the backing views of the dropped extension");
        }
        if let Some(matview) = self.matview_refresh
            && let Err(e) = crate::ddl::uncascaded::refresh_readers_of(matview)
        {
            e.raise_in(
                "pg_tviews: could not refresh the TVIEWs reading a refreshed materialized view",
            );
        }
    }
}

/// What the hook does with a utility statement, decided inside `catch_unwind`.
enum Intercept {
    /// Run it unchanged.
    PassThrough,
    /// Already handled.
    Handled,
    /// A `DROP` statement, to handle outside `catch_unwind`.
    DropTable(*mut pg_sys::DropStmt),
    /// A `CREATE TABLE tv_* AS` to run as a TVIEW creation, outside `catch_unwind`.
    CreateTview(Ctas),
    /// A statement that would create a TVIEW in a way `pg_tviews` cannot honour,
    /// and the table it names when that is a `CREATE TABLE`.
    Refuse(String, Option<CtasTarget>),
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
    // An error raised in the previous hook comes back as a Rust panic, so the
    // caller's Rust frames unwind instead of being skipped by a longjmp.
    unsafe {
        match PREV_PROCESS_UTILITY_HOOK {
            Some(prev_hook) => pg_sys::ffi::pg_guard_ffi_boundary(|| {
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
            }),
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
