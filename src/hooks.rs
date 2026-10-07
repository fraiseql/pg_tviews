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

use pgrx::datum::DatumWithOid;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;

use crate::TViewError;
use crate::ddl::drop_tview;
use crate::error::TViewResult;

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

    // Number every TRUNCATE, nested ones included, so its truncate triggers
    // refresh each TVIEW once (one fires per truncated partition).
    if !pstmt.is_null()
        && unsafe { !(*pstmt).utilityStmt.is_null() }
        && unsafe { (*(*pstmt).utilityStmt).type_ } == pg_sys::NodeTag::T_TruncateStmt
    {
        crate::delta::begin_truncate();
    }

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
                    // Still suspended at the end of the transaction: resume and rebuild
                    // the TVIEWs the suspended writes touched.
                    if crate::suspend::is_suspended() {
                        crate::suspend::force_resume();
                        if let Err(e) = crate::suspend::catch_up() {
                            unsafe { HOOK_IN_PROGRESS = false };
                            error!("TVIEW catch-up after suspension failed before {stmt}: {e:?}");
                        }
                    }
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

    // A column rename is applied to TVIEW metadata once PostgreSQL has run it (issue #81),
    // and a partition added or removed gets or loses its triggers.
    // A TVIEW's table renamed or moved to another schema takes its backing view's
    // name along (#181); its OID is resolved before the statement renames it.
    // A GRANT or REVOKE on tables, or a change of a table's owner, is followed by
    // the backing views (#181): their privileges are their tables'.
    // A materialized view refreshed rebuilds the TVIEWs that refresh in full on
    // its changes (#189).
    // DROP EXTENSION pg_tviews leaves the backing views, which are not members
    // (pg_dump keeps them): read them before the catalog goes, drop them after (#199).
    let extension_drop = if extension_installed() && unsafe { drops_pg_tviews(pstmt) } {
        match crate::ddl::drop::backing_views() {
            Ok(views) => Some(views),
            Err(e) => {
                unsafe { HOOK_IN_PROGRESS = false };
                error!("pg_tviews: could not read the backing views before DROP EXTENSION: {e}");
            }
        }
    } else {
        None
    };

    let (column_rename, partition_ddl, table_move, privileges_change, matview_refresh) =
        if extension_installed() {
            unsafe {
                (
                    column_rename_of(pstmt),
                    partition_ddl_of(pstmt),
                    table_move_of(pstmt),
                    privileges_change_of(pstmt),
                    matview_refresh_of(pstmt),
                )
            }
        } else {
            (None, None, None, None, None)
        };

    // Wrap FFI callback in catch_unwind to prevent panics crossing FFI boundary.
    // A DROP TABLE is only recognised here and handled after catch_unwind: its SPI
    // errors must propagate as PostgreSQL errors, not be caught.
    let result = std::panic::catch_unwind(|| -> Result<Intercept, TViewError> {
        // Safety check
        if pstmt.is_null() {
            return Ok(Intercept::PassThrough);
        }

        let pstmt_ref = unsafe { &*pstmt };

        // Check if this is a utility statement
        if pstmt_ref.utilityStmt.is_null() {
            return Ok(Intercept::PassThrough);
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
            if extensions.iter().any(|e| e == "pg_tviews") {
                crate::revision::reset();
            }
            return Ok(Intercept::PassThrough);
        }

        // The library is preloaded cluster-wide: in a database without the extension
        // there is no catalog to consult and nothing to maintain (issue #128).
        if !extension_installed() {
            return Ok(Intercept::PassThrough);
        }

        // Check for CREATE TABLE AS
        if node_tag == pg_sys::NodeTag::T_CreateTableAsStmt {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
            let ctas = utility_stmt.cast::<pg_sys::CreateTableAsStmt>();
            return unsafe { inspect_create_table_as(ctas, pstmt, query_string) };
        }

        // EXPLAIN [ANALYZE] CREATE TABLE tv_* AS … would run the CTAS without this
        // hook seeing it as one, nor the event trigger firing: refuse it.
        if node_tag == pg_sys::NodeTag::T_ExplainStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → ExplainStmt* cast
            let query = unsafe { (*utility_stmt.cast::<pg_sys::ExplainStmt>()).query };
            if let Some(table) = unsafe { tview_ctas_target(utility_of(query)) } {
                return Ok(Intercept::Refuse(
                    format!("EXPLAIN of CREATE TABLE {table} AS … cannot create a TVIEW"),
                    None,
                ));
            }
        }

        // DROP TABLE: handled after catch_unwind.
        if node_tag == pg_sys::NodeTag::T_DropStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → DropStmt* cast
            return Ok(Intercept::DropTable(
                utility_stmt.cast::<pg_sys::DropStmt>(),
            ));
        }

        // Check for ALTER TABLE
        if node_tag == pg_sys::NodeTag::T_AlterTableStmt {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → AlterTableStmt* cast
            let alter_stmt = utility_stmt.cast::<pg_sys::AlterTableStmt>();
            match unsafe { handle_alter_table(alter_stmt, query_string) } {
                Ok(true) => return Ok(Intercept::Handled),
                Ok(false) => {}
                Err(e) => return Err(e),
            }
        }

        // Not a tv_* statement - pass through
        Ok(Intercept::PassThrough)
    });

    // Check if hook handled the statement or if we need to pass through
    let should_pass_through = match result {
        Ok(Ok(Intercept::PassThrough)) => true,
        Ok(Ok(Intercept::Handled)) => false,
        Ok(Ok(
            Intercept::Refuse(_, Some(target)) | Intercept::CreateTview(Ctas { target, .. }),
        )) if skipped_or_raise(&target) => true,
        Ok(Ok(Intercept::Refuse(reason, _))) => {
            unsafe { HOOK_IN_PROGRESS = false };
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_FEATURE_NOT_SUPPORTED,
                format!("pg_tviews: {reason}"),
                function_name!(),
            )
            .set_hint(CTAS_HINT)
            .report(PgLogLevel::ERROR);
            unreachable!("ERROR does not return")
        }
        Ok(Ok(Intercept::CreateTview(ctas))) => {
            unsafe { create_tview_from_ctas(&ctas, qc) };
            false
        }
        Ok(Ok(Intercept::DropTable(drop_stmt))) => {
            match unsafe { handle_drop_table(drop_stmt, query_string) } {
                Ok(handled) => !handled,
                Err(e) => {
                    unsafe { HOOK_IN_PROGRESS = false };
                    error!("{e}");
                }
            }
        }
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

        // Like the drains above, this runs outside catch_unwind: its SPI errors must
        // abort the RENAME rather than leave a TVIEW with stale metadata.
        if let Some((relid, old_name, new_name)) = column_rename.and_then(ColumnRename::resolve)
            && let Err(e) = crate::ddl::rename::handle_column_rename(relid, &old_name, &new_name)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not follow the column rename: {e}");
        }
        if let Some(ddl) = partition_ddl
            && let Err(e) = unsafe { ddl.apply() }
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not update the triggers of a partition: {e}");
        }
        if let Some(table) = table_move
            && let Err(e) = crate::ddl::follow_table_move(table)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not rename the backing view of a moved TVIEW: {e}");
        }
        if let Some(PrivilegesChange { owners }) = privileges_change
            && let Err(e) = crate::ddl::privileges::follow(None, owners)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not give the backing views their tables' privileges: {e}");
        }
        if let Some(views) = extension_drop
            && let Err(e) = crate::ddl::drop::drop_left_backing_views(&views)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("pg_tviews: could not drop the backing views of the dropped extension: {e}");
        }
        if let Some(matview) = matview_refresh
            && let Err(e) = crate::ddl::uncascaded::refresh_readers_of(matview)
        {
            unsafe { HOOK_IN_PROGRESS = false };
            error!(
                "pg_tviews: could not refresh the TVIEWs reading a refreshed materialized view: {e}"
            );
        }
    }

    // Release the reentrancy guard
    unsafe { HOOK_IN_PROGRESS = false };
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

/// A `CREATE [UNLOGGED] TABLE [IF NOT EXISTS] [schema.]tv_* [WITH (fillfactor = n)]
/// AS SELECT …`, read from the parse tree.
struct Ctas {
    target: CtasTarget,
    query: String,
    logged: Option<bool>,
    fillfactor: Option<i32>,
}

/// The table a `CREATE TABLE … AS` creates.
struct CtasTarget {
    /// The schema named in the statement; `None`: `current_schema()`.
    schema: Option<String>,
    table: String,
    if_not_exists: bool,
}

impl CtasTarget {
    /// The name `pg_tviews_create_or_replace()` takes: `tv_<entity>` or
    /// `"schema".tv_<entity>`.
    fn name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("\"{}\".{}", schema.replace('"', "\"\""), self.table),
            None => self.table.clone(),
        }
    }

    /// Whether `IF NOT EXISTS` applies: a relation of that name exists in the
    /// schema the table would be created in. `PostgreSQL` then skips the statement
    /// with a notice. Uses SPI: call it outside `catch_unwind`.
    fn skipped(&self) -> TViewResult<bool> {
        if !self.if_not_exists {
            return Ok(false);
        }
        let args = [
            // SAFETY: the datums borrow `self`, which outlives the query.
            unsafe {
                DatumWithOid::new(
                    self.schema.as_deref(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                )
            },
            unsafe {
                DatumWithOid::new(
                    self.table.as_str(),
                    PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
                )
            },
        ];
        Spi::connect(|client| {
            client
                .select(
                    "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relname = $2 AND n.nspname = COALESCE($1, current_schema()))",
                    None,
                    &args,
                )?
                .first()
                .get_one::<bool>()
        })
        .map(|exists| exists == Some(true))
        .map_err(|e| TViewError::CatalogError {
            operation: format!("Look up {}", self.name()),
            pg_error: e.to_string(),
        })
    }
}

/// Where a refused `CREATE TABLE tv_* AS` sends the user.
const CTAS_HINT: &str = "Create or change the TVIEW with \
    SELECT tviews.pg_tviews_create_or_replace('tv_<entity>', $$<query>$$, \
    options => '{\"logged\": …, \"fillfactor\": …}').";

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

/// Whether `pg_tviews` is installed in the current database. A syscache lookup:
/// no SPI, and no catalog query that could fail when the extension is absent.
fn extension_installed() -> bool {
    // SAFETY: both calls only read backend state; the syscache lookup runs only
    // inside a transaction, where it is valid.
    unsafe {
        pg_sys::IsTransactionState()
            && pg_sys::get_extension_oid(c"pg_tviews".as_ptr(), true) != pg_sys::InvalidOid
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

/// The table an `ALTER TABLE … RENAME TO` or `ALTER TABLE … SET SCHEMA` renames or
/// moves, resolved before the statement runs.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
unsafe fn table_move_of(pstmt: *const pg_sys::PlannedStmt) -> Option<pg_sys::Oid> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        let relation = match (*node).type_ {
            pg_sys::NodeTag::T_RenameStmt => {
                let stmt = &*node.cast::<pg_sys::RenameStmt>();
                (stmt.renameType == pg_sys::ObjectType::OBJECT_TABLE).then_some(stmt.relation)
            }
            pg_sys::NodeTag::T_AlterObjectSchemaStmt => {
                let stmt = &*node.cast::<pg_sys::AlterObjectSchemaStmt>();
                (stmt.objectType == pg_sys::ObjectType::OBJECT_TABLE).then_some(stmt.relation)
            }
            _ => None,
        }?;
        if relation.is_null() {
            return None;
        }
        let relid = pg_sys::RangeVarGetRelidExtended(
            relation,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        );
        (relid != pg_sys::InvalidOid).then_some(relid)
    }
}

/// The materialized view a `REFRESH MATERIALIZED VIEW` fills (not `WITH NO DATA`,
/// which leaves nothing to read), resolved before the statement runs.
///
/// SAFETY: `pstmt` is null or a valid `PlannedStmt`.
unsafe fn matview_refresh_of(pstmt: *const pg_sys::PlannedStmt) -> Option<pg_sys::Oid> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        if (*node).type_ != pg_sys::NodeTag::T_RefreshMatViewStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        let stmt = &*node.cast::<pg_sys::RefreshMatViewStmt>();
        if stmt.skipData || stmt.relation.is_null() {
            return None;
        }
        let relid = pg_sys::RangeVarGetRelidExtended(
            stmt.relation,
            pg_sys::NoLock.cast_signed(),
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        );
        (relid != pg_sys::InvalidOid).then_some(relid)
    }
}

/// A statement after which the backing views must follow their tables' grants,
/// and their owners when `owners`.
struct PrivilegesChange {
    owners: bool,
}

/// `GRANT` / `REVOKE` on tables (by name or `ALL TABLES IN SCHEMA`), `ALTER TABLE
/// … OWNER TO` and `REASSIGN OWNED`.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
unsafe fn privileges_change_of(pstmt: *const pg_sys::PlannedStmt) -> Option<PrivilegesChange> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → statement cast by tag
        match (*node).type_ {
            pg_sys::NodeTag::T_GrantStmt => {
                let stmt = &*node.cast::<pg_sys::GrantStmt>();
                (stmt.objtype == pg_sys::ObjectType::OBJECT_TABLE)
                    .then_some(PrivilegesChange { owners: false })
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                let cmds = stmt.cmds;
                let changes_owner = stmt.objtype == pg_sys::ObjectType::OBJECT_TABLE
                    && (0..pg_sys::list_length(cmds)).any(|i| {
                        let cmd = pg_sys::list_nth(cmds, i).cast::<pg_sys::AlterTableCmd>();
                        !cmd.is_null() && (*cmd).subtype == pg_sys::AlterTableType::AT_ChangeOwner
                    });
                changes_owner.then_some(PrivilegesChange { owners: true })
            }
            pg_sys::NodeTag::T_ReassignOwnedStmt => Some(PrivilegesChange { owners: true }),
            _ => None,
        }
    }
}

/// While alive, statements `pg_tviews` runs pass through the hook unhandled, as
/// those it runs for a statement the hook handles do. Taken around its own
/// `GRANT`s and owner changes, which the hook would otherwise follow mid-way.
pub(crate) struct InternalDdl {
    took: bool,
}

impl InternalDdl {
    pub(crate) fn begin() -> Self {
        // SAFETY: single-threaded backend; plain reads/writes of process-local statics.
        unsafe {
            if HOOK_IN_PROGRESS {
                return Self { took: false };
            }
            HOOK_IN_PROGRESS = true;
            HOOK_GUARD_LEVEL = pg_sys::GetCurrentTransactionNestLevel();
        }
        Self { took: true }
    }
}

impl Drop for InternalDdl {
    fn drop(&mut self) {
        if self.took {
            // SAFETY: as in `begin`.
            unsafe { HOOK_IN_PROGRESS = false };
        }
    }
}

/// The tables a `CREATE TABLE … PARTITION OF`, `ALTER TABLE … ATTACH PARTITION` or
/// `… DETACH PARTITION` (also `CONCURRENTLY` and `FINALIZE`) adds to or removes
/// from a partition tree, resolved once `PostgreSQL` has run the statement.
struct PartitionDdl {
    tables: Vec<*mut pg_sys::RangeVar>,
    /// The partitioned table an `ATTACH` or `DETACH` changes the rows of.
    changed_rows_of: Option<*mut pg_sys::RangeVar>,
}

impl PartitionDdl {
    /// Give each added partition the triggers its tree's TVIEWs need, and take
    /// ours off each removed one.
    ///
    /// SAFETY: the `RangeVar`s come from the statement's parse tree, which outlives
    /// the statement.
    unsafe fn apply(self) -> TViewResult<()> {
        // Partition roots are cached per backend.
        crate::delta::clear_caches();
        for rv in self.tables {
            // SAFETY: see above.
            let oid = unsafe { resolve_relation_oid(rv) };
            if oid != pg_sys::InvalidOid {
                crate::dependency::triggers::ensure_partition_triggers(oid)?;
            }
        }
        // The rows of an attached or detached partition enter or leave the
        // partitioned table with no row written: refresh its TVIEWs in full.
        // SAFETY: see above.
        let parent = self
            .changed_rows_of
            .map_or(pg_sys::InvalidOid, |rv| unsafe { resolve_relation_oid(rv) });
        if parent != pg_sys::InvalidOid {
            crate::delta::refresh_tviews_over(parent)?;
        }
        Ok(())
    }
}

/// The partition change carried by `pstmt`, if it is one.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt` from the `ProcessUtility` hook.
unsafe fn partition_ddl_of(pstmt: *const pg_sys::PlannedStmt) -> Option<PartitionDdl> {
    // SAFETY: every pointer is null-checked before it is dereferenced.
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return None;
        }
        let node = (*pstmt).utilityStmt;
        let mut tables = Vec::new();
        let mut changed_rows_of = None;
        match (*node).type_ {
            pg_sys::NodeTag::T_CreateStmt => {
                #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → CreateStmt* cast
                let stmt = &*node.cast::<pg_sys::CreateStmt>();
                if !stmt.partbound.is_null() && !stmt.relation.is_null() {
                    tables.push(stmt.relation);
                }
            }
            pg_sys::NodeTag::T_AlterTableStmt => {
                #[allow(clippy::cast_ptr_alignment)]
                // Reason: PostgreSQL Node* → AlterTableStmt* cast
                let stmt = &*node.cast::<pg_sys::AlterTableStmt>();
                for i in 0..pg_sys::list_length(stmt.cmds) {
                    let cmd = pg_sys::list_nth(stmt.cmds, i).cast::<pg_sys::AlterTableCmd>();
                    if cmd.is_null()
                        || !matches!(
                            (*cmd).subtype,
                            pg_sys::AlterTableType::AT_AttachPartition
                                | pg_sys::AlterTableType::AT_DetachPartition
                                | pg_sys::AlterTableType::AT_DetachPartitionFinalize
                        )
                        || (*cmd).def.is_null()
                    {
                        continue;
                    }
                    #[allow(clippy::cast_ptr_alignment)]
                    // Reason: PostgreSQL Node* → PartitionCmd* cast
                    let partition = &*(*cmd).def.cast::<pg_sys::PartitionCmd>();
                    if !partition.name.is_null() {
                        tables.push(partition.name);
                        changed_rows_of = Some(stmt.relation);
                    }
                }
            }
            _ => {}
        }
        (!tables.is_empty()).then_some(PartitionDdl {
            tables,
            changed_rows_of,
        })
    }
}

/// Read a `CREATE TABLE tv_* AS` into a decision, from the parse tree only (no
/// SPI: this runs inside `catch_unwind`).
///
/// Anything but a `tv_*` table target passes through. What a TVIEW cannot honour is
/// refused; the rest becomes a [`Ctas`] to create after `catch_unwind`. Both carry
/// the target, so that `IF NOT EXISTS` on an existing relation is checked first.
///
/// SAFETY: the pointers come from the `ProcessUtility` hook and are null-checked.
unsafe fn inspect_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    pstmt: *const pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
) -> Result<Intercept, TViewError> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        let Some(table_name) = tview_ctas_target(ctas.cast()) else {
            return Ok(Intercept::PassThrough);
        };
        // TEST ONLY: simulate a hook that never saw this statement (see the
        // missed-interception check in the event trigger).
        if crate::config::test_skip_ctas_intercept() {
            return Ok(Intercept::PassThrough);
        }
        let ctas_ref = &*ctas;
        let into = &*ctas_ref.into;
        let rel = &*into.rel;

        let schema = (!rel.schemaname.is_null()).then(|| {
            CStr::from_ptr(rel.schemaname)
                .to_string_lossy()
                .into_owned()
        });
        let target = || CtasTarget {
            schema: schema.clone(),
            table: table_name.clone(),
            if_not_exists: ctas_ref.if_not_exists,
        };
        let refuse = |reason: &str| Ok(Intercept::Refuse(reason.to_string(), Some(target())));
        if ctas_ref.is_select_into {
            return Ok(Intercept::Refuse(
                format!("SELECT … INTO {table_name} cannot create a TVIEW"),
                None,
            ));
        }
        let persistence = rel.relpersistence.cast_unsigned();
        if persistence == pg_sys::RELPERSISTENCE_TEMP
            || schema
                .as_deref()
                .is_some_and(|schema| schema == "pg_temp" || schema.starts_with("pg_temp_"))
        {
            return refuse(&format!("{table_name} cannot be a temporary TVIEW"));
        }
        if !into.colNames.is_null() {
            return refuse(&format!(
                "{table_name} takes its column names from its query, not from a column list"
            ));
        }
        if !into.tableSpaceName.is_null() {
            return refuse(&format!(
                "TABLESPACE is not supported for TVIEW {table_name}"
            ));
        }
        if !into.accessMethod.is_null() {
            return refuse(&format!("USING is not supported for TVIEW {table_name}"));
        }
        if into.skipData {
            return refuse(&format!(
                "WITH NO DATA is not supported: TVIEW {table_name} is always populated"
            ));
        }
        if is_execute(ctas_ref.query) {
            return refuse(&format!(
                "CREATE TABLE {table_name} AS EXECUTE cannot create a TVIEW"
            ));
        }
        if !ctas_ref.query.is_null() && contains_param(ctas_ref.query, std::ptr::null_mut()) {
            return refuse(&format!(
                "a query with parameters (such as PL/pgSQL variables) cannot define TVIEW \
                 {table_name}"
            ));
        }
        let mut fillfactor = None;
        for i in 0..pg_sys::list_length(into.options) {
            let option = pg_sys::list_nth(into.options, i).cast::<pg_sys::DefElem>();
            if option.is_null() || (*option).defname.is_null() {
                continue;
            }
            let name = CStr::from_ptr((*option).defname).to_string_lossy();
            if name != "fillfactor" || !(*option).defnamespace.is_null() {
                return refuse(&format!(
                    "storage parameter {name} is not supported for TVIEW {table_name}; only \
                     fillfactor is"
                ));
            }
            match option_integer(option) {
                Some(value) if (10..=100).contains(&value) => fillfactor = Some(value),
                _ => {
                    return refuse(&format!(
                        "fillfactor for TVIEW {table_name} must be an integer from 10 to 100"
                    ));
                }
            }
        }

        let sql = if query_string.is_null() {
            ""
        } else {
            CStr::from_ptr(query_string).to_str().unwrap_or("")
        };
        // `query_string` is the whole simple-query batch; slice out just this
        // statement, then strip its `CREATE TABLE … AS` prefix (issue #95).
        let stmt_sql = statement_text(sql, pstmt);
        let query = extract_ctas_select(stmt_sql, &table_name).ok_or_else(|| {
            TViewError::InvalidSelectStatement {
                sql: stmt_sql.to_string(),
                reason: format!("Could not find 'CREATE TABLE {table_name} AS' in query"),
            }
        })?;
        Ok(Intercept::CreateTview(Ctas {
            target: target(),
            query,
            logged: (persistence == pg_sys::RELPERSISTENCE_UNLOGGED).then_some(false),
            fillfactor,
        }))
    }
}

/// The `tv_*` table a `CREATE TABLE … AS` (or `SELECT … INTO`) creates, if `node`
/// is one. A `CREATE MATERIALIZED VIEW` is left to `PostgreSQL`.
///
/// SAFETY: `node` must be null or a valid `Node*`.
unsafe fn tview_ctas_target(node: *mut pg_sys::Node) -> Option<String> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_CreateTableAsStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
        let ctas = &*node.cast::<pg_sys::CreateTableAsStmt>();
        if ctas.objtype != pg_sys::ObjectType::OBJECT_TABLE
            || ctas.into.is_null()
            || (*ctas.into).rel.is_null()
            || (*(*ctas.into).rel).relname.is_null()
        {
            return None;
        }
        let table = CStr::from_ptr((*(*ctas.into).rel).relname).to_str().ok()?;
        (table.starts_with("tv_") && table.len() > 3).then(|| table.to_string())
    }
}

/// The statement a utility `Query` wraps, as parse analysis leaves `EXECUTE` and
/// explained statements; any other node as it is.
///
/// SAFETY: `node` must be null or a valid `Node*`.
unsafe fn utility_of(node: *mut pg_sys::Node) -> *mut pg_sys::Node {
    // SAFETY: `node` is checked for null before it is dereferenced.
    unsafe {
        if !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_Query {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Query* cast
            let utility = (*node.cast::<pg_sys::Query>()).utilityStmt;
            if !utility.is_null() {
                return utility;
            }
        }
        node
    }
}

/// Whether a `CREATE TABLE … AS` query is an `EXECUTE`, raw or analyzed.
///
/// SAFETY: `query` must be null or a valid `Node*`.
unsafe fn is_execute(query: *mut pg_sys::Node) -> bool {
    // SAFETY: `utility_of` returns null or a valid node.
    unsafe {
        let node = utility_of(query);
        !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_ExecuteStmt
    }
}

/// Whether an analyzed query or expression contains a `Param` node.
///
/// SAFETY: a `tree_walker` callback over a valid parse tree.
unsafe extern "C-unwind" fn contains_param(
    node: *mut pg_sys::Node,
    context: *mut std::ffi::c_void,
) -> bool {
    if node.is_null() {
        return false;
    }
    // SAFETY: `node` is a valid node of the tree being walked.
    unsafe {
        match (*node).type_ {
            pg_sys::NodeTag::T_Param => true,
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Query* cast
            pg_sys::NodeTag::T_Query => pg_sys::query_tree_walker(
                node.cast::<pg_sys::Query>(),
                Some(contains_param),
                context,
                0,
            ),
            _ => pg_sys::expression_tree_walker(node, Some(contains_param), context),
        }
    }
}

/// The integer value of a `WITH (name = value)` option, if it is one.
///
/// SAFETY: `option` must be null or a valid `DefElem*`.
unsafe fn option_integer(option: *mut pg_sys::DefElem) -> Option<i32> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        if option.is_null() || (*option).arg.is_null() {
            return None;
        }
        let arg = (*option).arg;
        match (*arg).type_ {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Integer* cast
            pg_sys::NodeTag::T_Integer => Some((*arg.cast::<pg_sys::Integer>()).ival),
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → String* cast
            pg_sys::NodeTag::T_String => {
                let value = (*arg.cast::<pg_sys::String>()).sval;
                (!value.is_null())
                    .then(|| CStr::from_ptr(value).to_str().ok()?.parse().ok())
                    .flatten()
            }
            _ => None,
        }
    }
}

/// Whether `target` is skipped by `IF NOT EXISTS`, so the statement goes to
/// `PostgreSQL`, which skips it with its notice. Raises if the lookup fails.
fn skipped_or_raise(target: &CtasTarget) -> bool {
    target.skipped().unwrap_or_else(|e| {
        unsafe { HOOK_IN_PROGRESS = false };
        error!("{e}")
    })
}

/// Create the TVIEW a `CREATE TABLE tv_* AS` names, with `CREATE TABLE AS`
/// semantics, and report its rows in the command tag (`SELECT n`), as
/// `PostgreSQL` would. Runs outside `catch_unwind`: errors are raised as they are.
///
/// SAFETY: `qc` must be null or the hook's valid `QueryCompletion*`.
unsafe fn create_tview_from_ctas(ctas: &Ctas, qc: *mut pg_sys::QueryCompletion) {
    if !crate::revision::is_current() {
        unsafe { HOOK_IN_PROGRESS = false };
        crate::revision::check();
    }
    let created = crate::ddl::replace::create_only(
        &ctas.target.name(),
        &ctas.query,
        crate::ddl::replace::Options::storage(ctas.logged, ctas.fillfactor),
        ctas.target.if_not_exists,
    );
    match created {
        Ok(crate::ddl::replace::Created::Rows(rows)) => {
            if !qc.is_null() {
                // SAFETY: `qc` is the hook's completion record.
                unsafe {
                    (*qc).commandTag = pg_sys::CommandTag::CMDTAG_SELECT;
                    (*qc).nprocessed = rows;
                }
            }
        }
        Ok(crate::ddl::replace::Created::Skipped) => {}
        Ok(crate::ddl::replace::Created::Exists(name)) => {
            unsafe { HOOK_IN_PROGRESS = false };
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_DUPLICATE_TABLE,
                format!("TVIEW {name} already exists"),
                function_name!(),
            )
            .set_hint(CTAS_HINT)
            .report(PgLogLevel::ERROR);
        }
        Err(e) => {
            unsafe { HOOK_IN_PROGRESS = false };
            error!("{e}");
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
        r#"(?is)^\s*create\s+(?:[a-z]+\s+){{0,2}}?table\s+(?:if\s+not\s+exists\s+)?(?:"?[^\s."]+"?\s*\.\s*)?"?{}"?\s+(?:with\s*\([^)]*\)\s*)?as\s+"#,
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

/// Whether `pstmt` is a `DROP EXTENSION` naming `pg_tviews`.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt*`.
unsafe fn drops_pg_tviews(pstmt: *const pg_sys::PlannedStmt) -> bool {
    unsafe {
        if pstmt.is_null() || (*pstmt).utilityStmt.is_null() {
            return false;
        }
        let node = (*pstmt).utilityStmt;
        (*node).type_ == pg_sys::NodeTag::T_DropStmt
            && extension_statement_names(node)
                .is_some_and(|names| names.iter().any(|n| n == "pg_tviews"))
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
