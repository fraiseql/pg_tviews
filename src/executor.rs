//! One flush per outermost writing statement.
//!
//! The statement-level flush trigger fires at the end of every statement that
//! writes a TVIEW's base table, nested statements included. A statement run from
//! a trigger of such a write is nested inside it: flushing at its end recomputes
//! TVIEW rows from a state the enclosing statement is still changing, and they are
//! recomputed again at every level below (a tree cascade written by a trigger).
//!
//! The executor hooks keep a stack of the queries running (one frame per
//! `ExecutorRun` or `ExecutorFinish` call). A frame is *writing* when its query
//! writes a table carrying a flush trigger. The flush trigger skips while a
//! writing frame encloses the query it fires for: that statement's own flush,
//! later, sees every key. When the outermost writing frame finishes, what is
//! still queued is flushed, so a nested write after the enclosing flush trigger
//! (from a user trigger that fires after it) is not left behind.
//!
//! A query that only reads (`SELECT f()`) does not defer: each statement of `f`
//! that writes still refreshes the TVIEWs when it ends.

use pgrx::pg_sys;
use pgrx::prelude::*;
use std::cell::{Cell, RefCell};
use std::ffi::CStr;

static mut PREV_EXECUTOR_RUN: pg_sys::ExecutorRun_hook_type = None;
static mut PREV_EXECUTOR_FINISH: pg_sys::ExecutorFinish_hook_type = None;

thread_local! {
    /// Whether each running query writes a table carrying a flush trigger,
    /// innermost last.
    static FRAMES: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    /// The OID of `pg_tview_flush_trigger()`, once a trigger calling it was seen.
    static FLUSH_FUNCTION: Cell<pg_sys::Oid> = const { Cell::new(pg_sys::InvalidOid) };
}

const FLUSH_TRIGGER_PREFIX: &[u8] = b"trg_tview_flush_";
const FLUSH_FUNCTION_NAME: &CStr = c"pg_tview_flush_trigger";

/// Install the `ExecutorRun` and `ExecutorFinish` hooks.
///
/// SAFETY: called once, from backend context, while installing the other hooks.
pub unsafe fn install_hooks() {
    // SAFETY: plain writes of the backend's hook globals, saving the previous
    // values first, from the one place that installs them.
    unsafe {
        PREV_EXECUTOR_RUN = pg_sys::ExecutorRun_hook;
        pg_sys::ExecutorRun_hook = Some(executor_run);
        PREV_EXECUTOR_FINISH = pg_sys::ExecutorFinish_hook;
        pg_sys::ExecutorFinish_hook = Some(executor_finish);
    }
}

/// Whether the flush trigger firing now should leave the queue to an enclosing
/// writing statement: a writing frame below the innermost one.
pub fn flush_deferred() -> bool {
    FRAMES.with(|f| {
        let frames = f.borrow();
        frames
            .split_last()
            .is_some_and(|(_, enclosing)| enclosing.iter().any(|&writing| writing))
    })
}

/// Whether a writing statement is running: work queued now is flushed when
/// the outermost one finishes.
pub fn inside_writing_statement() -> bool {
    FRAMES.with(|f| f.borrow().iter().any(|&writing| writing))
}

/// How many queries are running: a subtransaction records it when it starts.
pub fn depth() -> usize {
    FRAMES.with(|f| f.borrow().len())
}

/// A subtransaction that started at `depth` rolled back: the queries it ran are
/// gone, even those whose frames an error skipped.
pub fn truncate_to(depth: usize) {
    FRAMES.with(|f| f.borrow_mut().truncate(depth));
}

/// Forget every frame: the transaction ended, so no query is running.
pub fn reset() {
    FRAMES.with(|f| f.borrow_mut().clear());
}

/// A frame pushed for the duration of one hook call, popped on return and while
/// unwinding from an error.
struct Frame;

impl Frame {
    fn push(writing: bool) -> Self {
        FRAMES.with(|f| f.borrow_mut().push(writing));
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

#[cfg(any(feature = "pg16", feature = "pg17"))]
#[pg_guard]
unsafe extern "C-unwind" fn executor_run(
    query_desc: *mut pg_sys::QueryDesc,
    direction: pg_sys::ScanDirection::Type,
    count: u64,
    execute_once: bool,
) {
    // SAFETY: the executor passes the started query it runs.
    let _frame = Frame::push(unsafe { writes_tracked_table(query_desc) });
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

#[cfg(feature = "pg18")]
#[pg_guard]
unsafe extern "C-unwind" fn executor_run(
    query_desc: *mut pg_sys::QueryDesc,
    direction: pg_sys::ScanDirection::Type,
    count: u64,
) {
    // SAFETY: the executor passes the started query it runs.
    let _frame = Frame::push(unsafe { writes_tracked_table(query_desc) });
    // SAFETY: an error raised in the previous hook comes back as a Rust panic,
    // which pops the frame on its way out.
    unsafe {
        match PREV_EXECUTOR_RUN {
            Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc, direction, count)),
            None => pg_sys::standard_ExecutorRun(query_desc, direction, count),
        }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_finish(query_desc: *mut pg_sys::QueryDesc) {
    // SAFETY: the executor passes the query it finishes.
    let writing = unsafe { writes_tracked_table(query_desc) };
    {
        let _frame = Frame::push(writing);
        // SAFETY: as in `executor_run`.
        unsafe {
            match PREV_EXECUTOR_FINISH {
                Some(prev) => pg_sys::ffi::pg_guard_ffi_boundary(|| prev(query_desc)),
                None => pg_sys::standard_ExecutorFinish(query_desc),
            }
        }
    }
    // The outermost writing statement has run its triggers: refresh what nested
    // statements queued after its flush trigger fired.
    if writing && !FRAMES.with(|f| f.borrow().iter().any(|&w| w)) {
        crate::trigger::flush_after_statement();
    }
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
fn fires(enabled: u8) -> bool {
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
