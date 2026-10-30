//! The `ProcessUtility` hook: `CREATE TABLE tv_* AS` becomes a TVIEW, `DROP TABLE
//! tv_*` drops one, renames and partition DDL are followed, `COMMIT` and `PREPARE
//! TRANSACTION` flush the refresh queue first, `DISCARD ALL` clears the caches.
//!
//! What to do is decided without SPI inside `catch_unwind` ([`decide`]); the work
//! itself, which may raise PostgreSQL errors, runs outside it, holding an
//! [`InternalDdl`](crate::internal_ddl::InternalDdl) so the DDL `pg_tviews` issues
//! passes through unhandled. The previous hook is called across
//! `pg_guard_ffi_boundary`.

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;

mod ctas;
mod indexes;
mod statements;
mod tables;

use ctas::{
    CTAS_HINT, Ctas, CtasTarget, create_tview_from_ctas, inspect_create_table_as, skipped_or_raise,
    tview_ctas_target, utility_of,
};
use indexes::{IndexFollowUp, index_follow_up_of};
use statements::{
    ColumnChange, ColumnRename, PartitionDdl, PrivilegesChange, ValueRewrite, column_change_of,
    column_rename_of, drops_pg_tviews, extension_installed, extension_statement_names,
    leaving_partitions_of, matview_refresh_of, partition_ddl_of, persistence_change_of,
    privileges_change_of, resolve_relation_oid, table_move_of, value_rewrite_of,
};
use tables::handle_drop_table;

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
/// `ALTER TABLE tv_*` (column changes its refreshes can't write through are refused), flushes the refresh queue before `COMMIT` and `PREPARE
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

    // A statement that may change the catalog: if the transaction rolls back, the
    // caches forget what they memoized of it.
    if call.tag().is_some_and(may_change_catalog) {
        crate::cache::mark_changed();
    }

    // DDL pg_tviews itself issues: run it as it is.
    if crate::internal_ddl::InternalDdl::active() {
        call.pass_through();
        return;
    }

    let pass_through = {
        let _internal = crate::internal_ddl::InternalDdl::begin();
        // What pg_tviews does about the statement resolves nothing on the caller's
        // search_path but the names the statement gives (`resolve_relation_oid`).
        let _path = crate::owner::CatalogPath::pin();
        flush_before_transaction_end(call);
        // SAFETY: the hook's statement.
        let follow_ups = unsafe { FollowUps::of(call.pstmt) };
        let pass_through = handle(decide(call), &mut call);
        pass_through.then(|| {
            follow_ups.prepare();
            follow_ups
        })
    };
    // The user's statement runs without the guard: DDL nested in it (a function
    // it calls) is intercepted like any other.
    if let Some(follow_ups) = pass_through {
        call.pass_through();
        let _internal = crate::internal_ddl::InternalDdl::begin();
        let _path = crate::owner::CatalogPath::pin();
        follow_ups.run();
    }
}

/// Whether a utility statement may change the catalog: any but those that only
/// control the session, the transaction, cursors and prepared statements, or that
/// run statements of their own (`DO`, `CALL`, `EXPLAIN`, `EXECUTE`: DDL they run
/// comes through the hook itself).
const fn may_change_catalog(tag: pg_sys::NodeTag) -> bool {
    use pg_sys::NodeTag as T;
    !matches!(
        tag,
        T::T_TransactionStmt
            | T::T_VariableSetStmt
            | T::T_VariableShowStmt
            | T::T_ConstraintsSetStmt
            | T::T_ExplainStmt
            | T::T_CopyStmt
            | T::T_PrepareStmt
            | T::T_ExecuteStmt
            | T::T_DeallocateStmt
            | T::T_DeclareCursorStmt
            | T::T_FetchStmt
            | T::T_ClosePortalStmt
            | T::T_NotifyStmt
            | T::T_ListenStmt
            | T::T_UnlistenStmt
            | T::T_LockStmt
            | T::T_DoStmt
            | T::T_CallStmt
            | T::T_DiscardStmt
            | T::T_CheckPointStmt
    )
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
    #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → TransactionStmt* cast
    // SAFETY: a TransactionStmt, by its tag.
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
        // SAFETY: the statement's utility node, non-null for a utility statement.
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
                // SAFETY: a CreateTableAsStmt, by its tag, of this planned statement.
                unsafe { inspect_create_table_as(ctas, call.pstmt, call.query_string) }
            }
            // EXPLAIN [ANALYZE] CREATE TABLE tv_* AS … would run the CTAS without
            // this hook seeing it as one, nor the event trigger firing: refuse it.
            pg_sys::NodeTag::T_ExplainStmt => {
                #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → ExplainStmt* cast
                // SAFETY: an ExplainStmt, by its tag.
                let query = unsafe { (*utility_stmt.cast::<pg_sys::ExplainStmt>()).query };
                // SAFETY: the explained statement's node, or null.
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
            // SAFETY: the statement's utility node, non-null.
            _ => Ok(unsafe { column_change_of(utility_stmt) }
                .map_or(Intercept::PassThrough, Intercept::ColumnChange)),
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
    // The payload's type tells what unwound: a message (a Rust `panic!`) or
    // something else, named by its type id since `Any` has no type name.
    let panic_msg = panic_info
        .downcast_ref::<&str>()
        .map(|s| format!("panic: {s}"))
        .or_else(|| {
            panic_info
                .downcast_ref::<String>()
                .map(|s| format!("panic: {s}"))
        })
        .unwrap_or_else(|| {
            format!(
                "unwind with a non-message payload ({:?})",
                (*panic_info).type_id()
            )
        });
    error!(
        "PANIC in ProcessUtility hook: {panic_msg} - This is a bug in pg_tviews - please report it!"
    )
}

/// Carry out `intercept`; whether the statement still goes to `PostgreSQL`.
/// Errors are raised as they are (outside `catch_unwind`).
fn handle(intercept: Intercept, call: &mut UtilityCall) -> bool {
    match intercept {
        Intercept::PassThrough => true,
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
        Intercept::ColumnChange(change) => match refuse_column_change(&change) {
            Ok(()) => true,
            Err(e) => e.raise(),
        },
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
            // SAFETY: a DropStmt of this planned statement (or the copy above).
            match unsafe { handle_drop_table(drop_stmt, call.query_string) } {
                Ok(handled) => !handled,
                Err(e) => e.raise(),
            }
        }
    }
}

/// Refuse `ALTER TABLE child INHERIT parent` onto a table a TVIEW reads: writes to
/// the child would fire no trigger on the parent, and the TVIEW would go stale
/// (a TVIEW reading the parent with its children is refused at create).
fn refuse_inherit(change: &ColumnChange) -> TViewResult<()> {
    for &parent in &change.inherits {
        // SAFETY: a RangeVar of the statement's parse tree.
        let parent = unsafe { resolve_relation_oid(parent) };
        if parent == pg_sys::InvalidOid {
            continue;
        }
        let readers = crate::catalog::queries::table_readers_of(parent)?;
        if let Some(entity) = readers.first() {
            // SAFETY: a RangeVar of the statement's parse tree; the child may not
            // exist yet (CREATE TABLE … INHERITS).
            let child = unsafe { CStr::from_ptr((*change.relation).relname) }.to_string_lossy();
            let parent = crate::utils::qualified_relname_from_oid(parent)?;
            return Err(TViewError::DefinitionRefused {
                reason: format!(
                    "{child} cannot inherit from {parent}: tv_{entity} reads it, and writes to \
                     an inheritance child fire no trigger on its parent; make {parent} a \
                     partitioned table and attach {child} as a partition"
                ),
            });
        }
    }
    Ok(())
}

/// Refuse a column change to a TVIEW's table that its refreshes could not write
/// through: they write every column the definition outputs, by name, with the
/// type the backing view gives it. Renaming or dropping one is refused, and so is
/// retyping `pk_<entity>`, `id` or `data`, whose types are fixed. Another column's
/// new type is refused unless the view's type converts to it on assignment (a
/// TVIEW can keep such a type until `pg_tviews_create_or_replace` retypes it).
fn refuse_column_change(change: &ColumnChange) -> TViewResult<()> {
    // The extension's own scripts change its tables as they need.
    if crate::revision::in_own_script() {
        return Ok(());
    }
    refuse_inherit(change)?;
    // SAFETY: a RangeVar of the statement's parse tree.
    let relid = unsafe { resolve_relation_oid(change.relation) };
    if relid == pg_sys::InvalidOid || crate::catalog::TviewMeta::entity_of_table(relid)?.is_none() {
        return Ok(());
    }
    let refused = |what: String| -> TViewResult<()> {
        Err(TViewError::ColumnDdlRefused {
            table: crate::utils::qualified_relname_from_oid(relid)?,
            change: what,
        })
    };
    if let Some(what) = change.removes {
        return refused(what.to_string());
    }
    for (column, type_name) in &change.retypes {
        let mut to = pg_sys::InvalidOid;
        let mut typmod = -1;
        {
            let _caller = crate::owner::CallerPath::enter();
            // SAFETY: a TypeName of the statement's parse tree; an unknown type
            // raises the error PostgreSQL would.
            unsafe {
                pg_sys::typenameTypeIdAndMod(
                    std::ptr::null_mut(),
                    *type_name,
                    &raw mut to,
                    &raw mut typmod,
                );
            }
        }
        let from = crate::catalog::queries::view_column_type(relid, column.as_str())?;
        let fixed = column == "id" || column == "data" || column.starts_with("pk_");
        // SAFETY: catalog lookups on two type OIDs.
        let writable = !fixed
            && from.is_some_and(|from| unsafe {
                pg_sys::can_coerce_type(
                    1,
                    &raw const from,
                    &raw const to,
                    pg_sys::CoercionContext::COERCION_ASSIGNMENT,
                )
            });
        if !writable {
            let new_type = crate::utils::qualified_type_name(to, typmod);
            return refused(format!(
                "ALTER COLUMN {} TYPE {}",
                crate::utils::ident::quoted(column),
                new_type.strip_prefix("pg_catalog.").unwrap_or(&new_type)
            ));
        }
    }
    Ok(())
}

/// What the statement changes that TVIEWs follow once `PostgreSQL` has run it,
/// read from the statement before it runs.
struct FollowUps {
    /// A column renamed: the definitions reading it follow.
    column_rename: Option<ColumnRename>,
    /// A partition added or removed: it gets or loses the triggers.
    partition_ddl: Option<PartitionDdl>,
    /// Partitions dropped, detached or truncated: their rows leave the TVIEWs over
    /// the tree.
    leaving_partitions: Vec<(pg_sys::Oid, bool)>,
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
    /// An index of a TVIEW's table created, renamed or dropped: `pg_tviews`'
    /// record of its own follows.
    index_follow_up: Option<IndexFollowUp>,
    /// A TVIEW's table (its entity and OID) switched to LOGGED (`true`) or
    /// UNLOGGED: a reset one is filled first, and its validity row follows.
    persistence_change: Option<(String, pg_sys::Oid, bool)>,
    /// Values rewritten without a trigger (a generation expression, an enum
    /// label): the TVIEWs reading them are refreshed.
    value_rewrite: Option<ValueRewrite>,
}

impl FollowUps {
    /// SAFETY: `pstmt` is null or the hook's valid statement.
    unsafe fn of(pstmt: *mut pg_sys::PlannedStmt) -> Self {
        if !extension_installed() {
            return Self {
                column_rename: None,
                partition_ddl: None,
                leaving_partitions: Vec::new(),
                table_move: None,
                privileges_change: None,
                extension_drop: None,
                matview_refresh: None,
                index_follow_up: None,
                persistence_change: None,
                value_rewrite: None,
            };
        }
        // SAFETY: as above.
        let extension_drop = unsafe { drops_pg_tviews(pstmt) }.then(|| {
            crate::catalog::queries::extension_backing_views().unwrap_or_else(|e| {
                e.raise_in("pg_tviews: could not read the backing views before DROP EXTENSION")
            })
        });
        // SAFETY: as above.
        unsafe {
            Self {
                column_rename: column_rename_of(pstmt),
                partition_ddl: partition_ddl_of(pstmt),
                leaving_partitions: leaving_partitions_of(pstmt),
                table_move: table_move_of(pstmt),
                privileges_change: privileges_change_of(pstmt),
                extension_drop,
                matview_refresh: matview_refresh_of(pstmt),
                index_follow_up: index_follow_up_of(pstmt).unwrap_or_else(|e| e.raise()),
                persistence_change: persistence_change_of(pstmt).and_then(|(table, logged)| {
                    crate::catalog::TviewMeta::entity_of_table(table)
                        .unwrap_or_else(|e| e.raise())
                        .map(|entity| (entity, table, logged))
                }),
                value_rewrite: value_rewrite_of(pstmt),
            }
        }
    }

    /// What must happen before `PostgreSQL` runs the statement.
    fn prepare(&self) {
        for &(partition, lock_parent) in &self.leaving_partitions {
            if let Err(e) = crate::delta::map_leaving_partition(partition, lock_parent) {
                e.raise_in("pg_tviews: could not map the rows of a partition leaving its tree");
            }
        }
        if let Some((entity, _, logged)) = &self.persistence_change
            && let Err(e) = crate::lifecycle::validity::before_persistence_change(entity, *logged)
        {
            e.raise_in("pg_tviews: could not fill a reset TVIEW before changing its persistence");
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
        // The rows that left with the statement: refreshed now (no flush trigger
        // follows DDL), or by the writing statement this one is nested in.
        if !self.leaving_partitions.is_empty()
            && !crate::executor::inside_writing_statement()
            && let Err(e) = crate::flush::flush_refresh_queue()
        {
            e.raise_in(
                "pg_tviews: could not refresh the TVIEWs over a dropped or detached partition",
            );
        }
        if let Some(ddl) = self.partition_ddl
            // SAFETY: the RangeVars come from the parse tree, which outlives the statement.
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
        if let Some(follow_up) = self.index_follow_up
            && let Err(e) = follow_up.run()
        {
            e.raise_in("pg_tviews: could not record the indexes of a TVIEW's table");
        }
        if let Some((entity, table, logged)) = self.persistence_change
            && let Err(e) =
                crate::lifecycle::validity::after_persistence_change(&entity, table, logged)
        {
            e.raise_in("pg_tviews: could not record the persistence of a TVIEW's table");
        }
        if let Some(matview) = self.matview_refresh
            && let Err(e) = crate::ddl::uncascaded::refresh_readers_of(matview)
        {
            e.raise_in(
                "pg_tviews: could not refresh the TVIEWs reading a refreshed materialized view",
            );
        }
        if let Some(rewrite) = self.value_rewrite {
            let readers = match &rewrite {
                #[cfg(not(feature = "pg16"))]
                ValueRewrite::Expression { table, columns } => {
                    crate::catalog::queries::column_readers_of(*table, columns)
                }
                ValueRewrite::EnumLabel(enum_type) => {
                    crate::catalog::queries::type_readers_of(*enum_type)
                }
            };
            if let Err(e) = readers.and_then(|r| crate::ddl::uncascaded::refresh_entities(&r)) {
                e.raise_in("pg_tviews: could not refresh the TVIEWs reading rewritten values");
            }
        }
    }
}

/// What the hook does with a utility statement, decided inside `catch_unwind`.
enum Intercept {
    /// Run it unchanged.
    PassThrough,
    /// A `DROP` statement, to handle outside `catch_unwind`.
    DropTable(*mut pg_sys::DropStmt),
    /// An `ALTER TABLE` renaming, dropping or retyping columns: checked against
    /// a TVIEW's table outside `catch_unwind`.
    ColumnChange(ColumnChange),
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
