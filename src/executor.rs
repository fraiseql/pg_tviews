//! One flush per outermost statement, or at the first read of a TVIEW before it.
//!
//! The statement-level flush trigger fires at the end of every statement that
//! writes a TVIEW's base table, nested statements included. A statement nested in
//! another (run by a function the outer statement calls, or by a trigger of its
//! write) leaves its keys queued: flushing at its end recomputes TVIEW rows the
//! enclosing statement may change again (a mutation function writing one row in
//! several statements, a tree cascade written by a trigger), and they would be
//! recomputed at every level.
//!
//! The executor hooks keep a stack of the queries running (one frame per
//! `ExecutorRun` or `ExecutorFinish` call). The flush trigger skips while any
//! frame encloses the query it fires for. What is queued is flushed when the
//! outermost frame ends: in `ExecutorFinish`, after its `AFTER` triggers (with a
//! snapshot pushed when a portal popped its own), and, for a query that writes no
//! TVIEW's base table (`SELECT app.fn(…)`, a cursor's fetch), already when its
//! `ExecutorRun` returns.
//!
//! A query that reads a TVIEW's table (directly, or through a view) while work is
//! queued flushes it first, in `ExecutorStart`, and runs on a snapshot taken after
//! that flush, as the next statement of a function would under `READ COMMITTED`:
//! a mutation reading its result back before it returns reads it fresh. The
//! snapshot it was given is left alone (a STABLE function's query shares its
//! caller's, which the caller's scans still use); the new one is the query's own,
//! made active around each executor call on it. `pg_tviews`' own queries never
//! flush (the flush itself, the catalog reads of a trigger or a function of the
//! extension).

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;

static mut PREV_EXECUTOR_START: pg_sys::ExecutorStart_hook_type = None;
static mut PREV_EXECUTOR_RUN: pg_sys::ExecutorRun_hook_type = None;
static mut PREV_EXECUTOR_FINISH: pg_sys::ExecutorFinish_hook_type = None;
static mut PREV_EXECUTOR_END: pg_sys::ExecutorEnd_hook_type = None;

thread_local! {
    /// The number of each running query's frame, innermost last.
    static FRAMES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// The number of the last frame pushed: each frame of the backend has its own.
    static LAST_FRAME: Cell<u64> = const { Cell::new(0) };
    /// The queries started on a snapshot of their own after a flush, until they end.
    static RESNAPPED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    /// Whether a flush was left to an enclosing statement since the last flush:
    /// the work queued then is that statement's to flush. Work queued without a
    /// flush trigger firing (one dropped or disabled) is not: the commit fails
    /// loud on it.
    static DEFERRED: Cell<bool> = const { Cell::new(false) };
    /// The command id current when the transaction's last refresh ended: a
    /// snapshot whose command id is not above it does not see that refresh.
    static REFRESHED_AT: Cell<Option<pg_sys::CommandId>> = const { Cell::new(None) };
    /// The OID of `pg_tview_flush_trigger()`, once a trigger calling it was seen.
    static FLUSH_FUNCTION: Cell<pg_sys::Oid> = const { Cell::new(pg_sys::InvalidOid) };
}

const FLUSH_TRIGGER_PREFIX: &[u8] = b"trg_tview_flush_";
const FLUSH_FUNCTION_NAME: &CStr = c"pg_tview_flush_trigger";

/// Install the `ExecutorStart`, `ExecutorRun`, `ExecutorFinish` and
/// `ExecutorEnd` hooks.
///
/// SAFETY: called once, from backend context, while installing the other hooks.
pub unsafe fn install_hooks() {
    // SAFETY: plain writes of the backend's hook globals, saving the previous
    // values first, from the one place that installs them.
    unsafe {
        PREV_EXECUTOR_START = pg_sys::ExecutorStart_hook;
        pg_sys::ExecutorStart_hook = Some(executor_start);
        PREV_EXECUTOR_RUN = pg_sys::ExecutorRun_hook;
        pg_sys::ExecutorRun_hook = Some(executor_run);
        PREV_EXECUTOR_FINISH = pg_sys::ExecutorFinish_hook;
        pg_sys::ExecutorFinish_hook = Some(executor_finish);
        PREV_EXECUTOR_END = pg_sys::ExecutorEnd_hook;
        pg_sys::ExecutorEnd_hook = Some(executor_end);
    }
}

/// Whether the flush trigger firing now leaves the queue to an enclosing
/// statement (a frame below the innermost one), which then flushes it.
pub fn flush_deferred() -> bool {
    defer_if(FRAMES.with(|f| f.borrow().len() > 1))
}

/// Whether a statement is running, which then flushes what is queued now when
/// the outermost one ends, or at a read of a TVIEW before that.
pub fn defers_flush() -> bool {
    defer_if(FRAMES.with(|f| !f.borrow().is_empty()))
}

fn defer_if(deferring: bool) -> bool {
    if deferring {
        DEFERRED.set(true);
    }
    deferring
}

/// Whether no query is running.
fn outermost() -> bool {
    FRAMES.with(|f| f.borrow().is_empty())
}

/// The number of the innermost running query's frame: during its `AFTER`
/// statement triggers, the same for each of them and for no other statement.
/// `None` outside the executor (a `COPY` fires its triggers without one).
pub fn frame() -> Option<u64> {
    FRAMES.with(|f| f.borrow().last().copied())
}

/// Where the executor's state stood when a subtransaction started.
pub struct Mark {
    frames: usize,
    resnapped: usize,
    deferred: bool,
    refreshed_at: Option<pg_sys::CommandId>,
}

/// Record where the executor's state stands: a subtransaction starts.
pub fn mark() -> Mark {
    Mark {
        frames: FRAMES.with(|f| f.borrow().len()),
        resnapped: RESNAPPED.with(|r| r.borrow().len()),
        deferred: DEFERRED.get(),
        refreshed_at: REFRESHED_AT.get(),
    }
}

/// A subtransaction that started at `mark` rolled back: the queries it ran are
/// gone, even those whose frames or end an error skipped, and a flush it ran is
/// undone with its writes, so the work that flush took is deferred again.
pub fn rollback(mark: &Mark) {
    FRAMES.with(|f| f.borrow_mut().truncate(mark.frames));
    RESNAPPED.with(|r| r.borrow_mut().truncate(mark.resnapped));
    DEFERRED.set(mark.deferred);
    REFRESHED_AT.set(mark.refreshed_at);
}

/// Forget every frame, every query started on its own snapshot and the last
/// refresh: the transaction ended, so no query is running.
pub fn reset() {
    FRAMES.with(|f| f.borrow_mut().clear());
    RESNAPPED.with(|r| r.borrow_mut().clear());
    REFRESHED_AT.set(None);
    DEFERRED.set(false);
}

/// A flush drained the queue; when `wrote`, it wrote TVIEW rows, and a query
/// reading a TVIEW on a snapshot taken before now reads on one of its own
/// ([`executor_start`]).
pub fn flushed(wrote: bool) {
    DEFERRED.set(false);
    if wrote {
        // SAFETY: reads the transaction's current command id.
        REFRESHED_AT.set(Some(unsafe { pg_sys::GetCurrentCommandId(false) }));
    }
}

/// A frame pushed for the duration of one hook call, popped on return and while
/// unwinding from an error.
struct Frame;

impl Frame {
    fn push() -> Self {
        let number = LAST_FRAME.get().wrapping_add(1);
        LAST_FRAME.set(number);
        FRAMES.with(|f| f.borrow_mut().push(number));
        Self
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        FRAMES.with(|f| {
            f.borrow_mut().pop();
        });
    }
}

/// While alive, a query started on its own snapshot ([`executor_start`]) has it
/// active, as the executor requires (`standard_ExecutorRun` asserts it): its
/// callers made theirs active. Nothing for any other query.
struct OwnSnapshot(bool);

impl OwnSnapshot {
    /// SAFETY: `query_desc` must be a started query.
    unsafe fn activate(query_desc: *mut pg_sys::QueryDesc) -> Self {
        // SAFETY: the caller's started query.
        let own = RESNAPPED.with(|r| r.borrow().contains(&(query_desc as usize)))
            && unsafe { !(*query_desc).snapshot.is_null() };
        if own {
            // SAFETY: the query's registered snapshot, alive until it ends.
            unsafe { pg_sys::PushActiveSnapshot((*query_desc).snapshot) };
        }
        Self(own)
    }
}

impl Drop for OwnSnapshot {
    fn drop(&mut self) {
        // While unwinding, the (sub)transaction abort pops it.
        if self.0 && !std::thread::panicking() {
            // SAFETY: pops the snapshot `activate` pushed, on top again.
            unsafe { pg_sys::PopActiveSnapshot() };
        }
    }
}

#[cfg(any(feature = "pg16", feature = "pg17"))]
#[pg_guard]
unsafe extern "C-unwind" fn executor_run(
    query_desc: *mut pg_sys::QueryDesc,
    direction: pg_sys::ScanDirection::Type,
    count: u64,
    execute_once: bool,
) {
    // SAFETY: the executor passes the started query it runs.
    let writing = unsafe { writes_tracked_table(query_desc) };
    {
        let _frame = Frame::push();
        // SAFETY: as above.
        let _snapshot = unsafe { OwnSnapshot::activate(query_desc) };
        // SAFETY: an error raised in the previous hook comes back as a Rust panic,
        // which pops the frame on its way out.
        unsafe {
            match PREV_EXECUTOR_RUN {
                Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| {
                    prev(query_desc, direction, count, execute_once);
                }),
                None => pg_sys::standard_ExecutorRun(query_desc, direction, count, execute_once),
            }
        }
    }
    flush_after_reading_statement(writing);
}

#[cfg(not(any(feature = "pg16", feature = "pg17")))]
#[pg_guard]
unsafe extern "C-unwind" fn executor_run(
    query_desc: *mut pg_sys::QueryDesc,
    direction: pg_sys::ScanDirection::Type,
    count: u64,
) {
    // SAFETY: the executor passes the started query it runs.
    let writing = unsafe { writes_tracked_table(query_desc) };
    {
        let _frame = Frame::push();
        // SAFETY: as above.
        let _snapshot = unsafe { OwnSnapshot::activate(query_desc) };
        // SAFETY: an error raised in the previous hook comes back as a Rust panic,
        // which pops the frame on its way out.
        unsafe {
            match PREV_EXECUTOR_RUN {
                Some(prev) => {
                    pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc, direction, count));
                }
                None => pg_sys::standard_ExecutorRun(query_desc, direction, count),
            }
        }
    }
    flush_after_reading_statement(writing);
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_finish(query_desc: *mut pg_sys::QueryDesc) {
    // SAFETY: the executor passes the query it finishes.
    let writing = unsafe { writes_tracked_table(query_desc) };
    {
        let _frame = Frame::push();
        // SAFETY: as in `executor_run`.
        let _snapshot = unsafe { OwnSnapshot::activate(query_desc) };
        // SAFETY: as in `executor_run`.
        unsafe {
            match PREV_EXECUTOR_FINISH {
                Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc)),
                None => pg_sys::standard_ExecutorFinish(query_desc),
            }
        }
    }
    // The outermost statement has run its triggers: refresh what it and the
    // statements nested in it (a trigger's, a function's) queued after its flush
    // trigger fired, or without one (a write to a table no TVIEW reads whose
    // trigger writes one that a TVIEW does).
    if outermost() && (writing || has_callers_work()) {
        // SAFETY: a portal finishes a `SELECT` after popping its snapshot, which
        // SPI needs; the pushed one is popped below, or by the abort.
        let pushed = unsafe { !pg_sys::ActiveSnapshotSet() };
        if pushed {
            // SAFETY: as above.
            unsafe { pg_sys::PushActiveSnapshot(pg_sys::GetTransactionSnapshot()) };
        }
        crate::trigger::flush_after_statement();
        if pushed {
            // SAFETY: pops the snapshot pushed above.
            unsafe { pg_sys::PopActiveSnapshot() };
        }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_end(query_desc: *mut pg_sys::QueryDesc) {
    // SAFETY: as in `executor_run`.
    unsafe {
        match PREV_EXECUTOR_END {
            Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc)),
            None => pg_sys::standard_ExecutorEnd(query_desc),
        }
    }
    RESNAPPED.with(|r| r.borrow_mut().retain(|&q| q != query_desc as usize));
}

/// Work a flush trigger left to the running statement is queued, and this is not
/// `pg_tviews`' own query (one a utility hook runs with nothing above it leaves
/// the queue to its caller).
fn has_callers_work() -> bool {
    DEFERRED.get() && crate::queue::has_pending() && !crate::owner::in_extension_work()
}

/// The outermost statement wrote no TVIEW's base table (`SELECT app.fn(…)`):
/// refresh what the statements it ran queued while its snapshot is active, and
/// after each fetch of a cursor. A writing one refreshes in `ExecutorFinish`,
/// after its `AFTER` triggers.
fn flush_after_reading_statement(writing: bool) {
    if !writing && outermost() && has_callers_work() {
        crate::trigger::flush_after_statement();
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_start(query_desc: *mut pg_sys::QueryDesc, eflags: i32) {
    // SAFETY: the executor passes the query it starts.
    match unsafe { before_start(query_desc, eflags) } {
        Before::Nothing => {}
        Before::Flush => {
            crate::trigger::flush_after_statement();
            // SAFETY: the query is not started.
            unsafe { take_own_snapshot(query_desc) };
        }
        // SAFETY: as above.
        Before::Snapshot => unsafe { take_own_snapshot(query_desc) },
    }
    // SAFETY: as in `executor_run`.
    let _snapshot = unsafe { OwnSnapshot::activate(query_desc) };
    // SAFETY: as in `executor_run`.
    unsafe {
        match PREV_EXECUTOR_START {
            Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc, eflags)),
            None => pg_sys::standard_ExecutorStart(query_desc, eflags),
        }
    }
}

/// What a query about to start needs before it reads a TVIEW.
enum Before {
    Nothing,
    /// Work is queued: flush it, then read on a snapshot taken after.
    Flush,
    /// The query's snapshot predates the last refresh (a STABLE function's query
    /// on its caller's snapshot): read on a snapshot taken now.
    Snapshot,
}

/// Whether the query about to start reads a TVIEW's table while refresh work is
/// queued, or on a snapshot older than the transaction's last refresh. Never for
/// `pg_tviews`' own queries (the flush, a catalog read). Cheap when nothing is
/// queued and the snapshot is current.
///
/// SAFETY: `query_desc` must be a query about to start.
unsafe fn before_start(query_desc: *mut pg_sys::QueryDesc, eflags: i32) -> Before {
    // SAFETY: the caller's query, null-checked.
    let stale = unsafe {
        !query_desc.is_null()
            && !(*query_desc).snapshot.is_null()
            && REFRESHED_AT
                .get()
                .is_some_and(|at| (*(*query_desc).snapshot).curcid <= at)
    };
    let queued = has_callers_work();
    if !(queued || stale)
        || eflags & pg_sys::EXEC_FLAG_EXPLAIN_ONLY.cast_signed() != 0
        || crate::owner::in_extension_work()
        || crate::flush::is_flushing()
        || crate::suspend::is_suspended()
        // SAFETY: a plain backend-global flag.
        || unsafe { pg_sys::IsInParallelMode() }
        // SAFETY: as above.
        || !unsafe { reads_tview_table(query_desc) }
    {
        return Before::Nothing;
    }
    if queued {
        Before::Flush
    } else {
        Before::Snapshot
    }
}

/// Whether the query's range table holds a TVIEW's table (a view over one
/// included: the rewriter expands it into the range table).
///
/// SAFETY: `query_desc` must be null or a query whose plan is valid.
unsafe fn reads_tview_table(query_desc: *const pg_sys::QueryDesc) -> bool {
    // SAFETY: the caller's query; pointers are null-checked, and the range table
    // holds `RangeTblEntry` nodes.
    unsafe {
        if query_desc.is_null() || (*query_desc).plannedstmt.is_null() {
            return false;
        }
        let rtable = (*(*query_desc).plannedstmt).rtable;
        let relations: Vec<pg_sys::Oid> = (0..pg_sys::list_length(rtable))
            .filter_map(|i| {
                let rte = pg_sys::list_nth(rtable, i).cast::<pg_sys::RangeTblEntry>();
                (!rte.is_null() && (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION)
                    .then(|| (*rte).relid)
            })
            .collect();
        if relations.is_empty() {
            return false;
        }
        match crate::catalog::tview_tables() {
            Ok(tables) => relations.iter().any(|relid| tables.contains(relid)),
            Err(e) => e.raise_in("pg_tviews: could not read which tables are TVIEWs"),
        }
    }
}

/// Give the query a snapshot taken now, after the refresh: it sees the refreshed
/// rows and every transaction committed by then, consistently (a snapshot from
/// before the refresh with only its command id advanced would see a row both as
/// a concurrent commit left it and as the refresh rewrote it). The descriptor's
/// registration of the snapshot it was given is released; the new one is
/// registered for it, freed when the descriptor is.
///
/// SAFETY: `query_desc` must be a query not yet started.
unsafe fn take_own_snapshot(query_desc: *mut pg_sys::QueryDesc) {
    // SAFETY: the snapshot functions PostgreSQL exports: the new snapshot is
    // copied by the push, registered for the descriptor and popped (it stays
    // alive, registered); the old one is only unregistered, so its other holders
    // keep it.
    unsafe {
        pg_sys::CommandCounterIncrement();
        pg_sys::PushActiveSnapshot(pg_sys::GetTransactionSnapshot());
        let fresh = pg_sys::RegisterSnapshot(pg_sys::GetActiveSnapshot());
        pg_sys::PopActiveSnapshot();
        let given = (*query_desc).snapshot;
        (*query_desc).snapshot = fresh;
        if !given.is_null() {
            pg_sys::UnregisterSnapshot(given);
        }
    }
    RESNAPPED.with(|r| r.borrow_mut().push(query_desc as usize));
}

/// Whether the query writes a table one of whose triggers calls
/// `pg_tview_flush_trigger()`. Reads only the result relations the executor
/// opened, and the syscache: no SPI.
///
/// SAFETY: `query_desc` must be a started query.
unsafe fn writes_tracked_table(query_desc: *mut pg_sys::QueryDesc) -> bool {
    // SAFETY: the caller's started query; every pointer is checked before use,
    // and the list holds `ResultRelInfo`s the executor opened.
    unsafe {
        if query_desc.is_null() || (*query_desc).estate.is_null() {
            return false;
        }
        let opened = (*(*query_desc).estate).es_opened_result_relations;
        for i in 0..pg_sys::list_length(opened) {
            let rel = pg_sys::list_nth(opened, i).cast::<pg_sys::ResultRelInfo>();
            if !rel.is_null() && has_flush_trigger((*rel).ri_TrigDesc) {
                return true;
            }
        }
        false
    }
}

/// SAFETY: `desc` must be null or a valid `TriggerDesc*`.
unsafe fn has_flush_trigger(desc: *const pg_sys::TriggerDesc) -> bool {
    // SAFETY: the caller's descriptor, checked for null; `numtriggers` bounds the
    // `triggers` array.
    unsafe {
        if desc.is_null() {
            return false;
        }
        let count = usize::try_from((*desc).numtriggers).unwrap_or(0);
        (0..count).any(|i| {
            let trigger = (*desc).triggers.add(i);
            fires((*trigger).tgenabled.cast_unsigned())
                && !(*trigger).tgname.is_null()
                && CStr::from_ptr((*trigger).tgname)
                    .to_bytes()
                    .starts_with(FLUSH_TRIGGER_PREFIX)
                && is_flush_function((*trigger).tgfoid)
        })
    }
}

/// Whether a trigger with this `tgenabled` fires in this session, as
/// `PostgreSQL` decides it (`ALTER TABLE … DISABLE TRIGGER`,
/// `session_replication_role`).
pub(crate) fn fires(enabled: u8) -> bool {
    // SAFETY: a plain backend-global setting.
    let replica = unsafe { pg_sys::SessionReplicationRole }
        == pg_sys::SESSION_REPLICATION_ROLE_REPLICA.cast_signed();
    match enabled {
        pg_sys::TRIGGER_DISABLED => false,
        pg_sys::TRIGGER_FIRES_ON_ORIGIN => !replica,
        pg_sys::TRIGGER_FIRES_ON_REPLICA => replica,
        _ => true,
    }
}

fn is_flush_function(function: pg_sys::Oid) -> bool {
    if FLUSH_FUNCTION.with(Cell::get) == function {
        return true;
    }
    // SAFETY: a syscache lookup; null when the function does not exist.
    let name = unsafe { pg_sys::get_func_name(function) };
    if name.is_null() {
        return false;
    }
    // SAFETY: a palloc'd, NUL-terminated name, freed after the comparison.
    let matches = unsafe {
        let matches = CStr::from_ptr(name) == FLUSH_FUNCTION_NAME;
        pg_sys::pfree(name.cast());
        matches
    };
    if matches {
        FLUSH_FUNCTION.with(|f| f.set(function));
    }
    matches
}
