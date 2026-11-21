//! The time boundary as writers see it.
//!
//! The time refresh brings a time-dependent TVIEW's rows to its transaction's
//! `now()`, the boundary `B`. A writer whose transaction began before `B` computes
//! its rows with an earlier `now()`; if it writes them after the refresh read the
//! table, they would stay at that earlier instant. The two meet on an advisory lock
//! of the TVIEW, `BOUNDARY(V)`:
//!
//! 1. The time refresh announces `B` in shared memory ([`announce`]), then waits for
//!    the transactions holding `BOUNDARY(V)` ([`crate::concurrency::wait_for_boundary_writers`])
//!    without joining the lock's queue, and only then compares the rows.
//! 2. Every flush that wrote rows of `V` takes `BOUNDARY(V)` in `ShareLock`, held to
//!    the end of its transaction, and reads `B` after taking it ([`check_flush`]). A
//!    transaction that began before `B` records the keys it wrote in
//!    `tviews.pg_tview_time_handoff`, committed with its rows.
//!
//! A writer that took the lock before the announcement is waited for, so the
//! refresh reads its rows and recomputes them at `B`. One that takes it after reads
//! `B` and hands its keys off. Every write of a row is flushed before its
//! transaction commits, and every flush checks: no write escapes both. The keys
//! handed off are refreshed again, after the writer commits, by
//! `crate::admin::drain_time_handoff`.

use crate::TViewResult;
use pgrx::pg_guard;
use pgrx::pg_sys;
use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, Ordering};

const NAME: &CStr = c"pg_tviews_boundaries";

/// Boundaries kept at once, across databases. A full table forgets the oldest:
/// a TVIEW is announced at the start of its time refresh, so the oldest slot is
/// one whose refresh ended long ago.
const SLOTS: usize = 1024;

/// One TVIEW's last announced boundary; `database` 0 marks a free slot.
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    database: u32,
    tview: u32,
    at: pg_sys::TimestampTz,
}

#[repr(C)]
struct Table {
    slots: [Slot; SLOTS],
}

static TABLE: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
static LOCK: AtomicPtr<pg_sys::LWLock> = AtomicPtr::new(std::ptr::null_mut());

static mut PREV_SHMEM_REQUEST: pg_sys::shmem_request_hook_type = None;
static mut PREV_SHMEM_STARTUP: pg_sys::shmem_startup_hook_type = None;

/// The shared table under its lock; released on drop.
struct Locked {
    table: *mut Table,
    lock: *mut pg_sys::LWLock,
}

impl Locked {
    fn acquire(mode: pg_sys::LWLockMode::Type) -> Option<Self> {
        let (table, lock) = (TABLE.load(Ordering::Acquire), LOCK.load(Ordering::Acquire));
        if table.is_null() || lock.is_null() {
            return None;
        }
        // SAFETY: the tranche's lock, set up by the startup hook.
        unsafe { pg_sys::LWLockAcquire(lock, mode) };
        Some(Self { table, lock })
    }

    fn slots(&mut self) -> &mut [Slot; SLOTS] {
        // SAFETY: the shared table, held under its lock while `self` lives;
        // writers took it exclusively.
        unsafe { &mut (*self.table).slots }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: the lock `acquire` took.
        unsafe { pg_sys::LWLockRelease(self.lock) };
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn request_shmem() {
    // SAFETY: called by the postmaster's shmem_request_hook, once.
    unsafe {
        if let Some(prev) = PREV_SHMEM_REQUEST {
            pg_sys::ffi::pg_guard_ffi_boundary(|| prev());
        }
        pg_sys::RequestAddinShmemSpace(std::mem::size_of::<Table>());
        pg_sys::RequestNamedLWLockTranche(NAME.as_ptr(), 1);
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn startup_shmem() {
    // SAFETY: called by shmem_startup_hook, after request_shmem asked for the
    // table's space; a table found already is not touched.
    unsafe {
        if let Some(prev) = PREV_SHMEM_STARTUP {
            pg_sys::ffi::pg_guard_ffi_boundary(|| prev());
        }
        let table = crate::utils::shmem::init_struct::<Table>(NAME, |table| {
            std::ptr::write_bytes(table, 0, 1);
        });
        TABLE.store(table, Ordering::Release);
        LOCK.store(
            &raw mut (*pg_sys::GetNamedLWLockTranche(NAME.as_ptr())).lock,
            Ordering::Release,
        );
    }
}

/// Request the shared table, while the library is preloaded.
pub fn init() {
    // SAFETY: _PG_init during shared_preload_libraries: the hooks are installed
    // once, before any backend starts, chaining the previous ones.
    unsafe {
        if !pg_sys::process_shared_preload_libraries_in_progress {
            return;
        }
        PREV_SHMEM_REQUEST = pg_sys::shmem_request_hook;
        pg_sys::shmem_request_hook = Some(request_shmem);
        PREV_SHMEM_STARTUP = pg_sys::shmem_startup_hook;
        pg_sys::shmem_startup_hook = Some(startup_shmem);
    }
}

fn database() -> u32 {
    // SAFETY: MyDatabaseId is set once the backend is connected to a database.
    unsafe { pg_sys::MyDatabaseId }.to_u32()
}

/// Announce that TVIEW `tview` of this database is being brought to `at`. A later
/// boundary already announced is kept. Without the shared table (the library not
/// preloaded) nothing is announced, and no writer hands off.
pub fn announce(tview: pg_sys::Oid, at: pg_sys::TimestampTz) {
    let Some(mut table) = Locked::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE) else {
        return;
    };
    let (database, tview) = (database(), tview.to_u32());
    let slots = table.slots();
    let chosen = slots
        .iter()
        .position(|s| s.database == database && s.tview == tview)
        .or_else(|| slots.iter().position(|s| s.database == 0))
        .unwrap_or_else(|| {
            slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.at)
                .map_or(0, |(i, _)| i)
        });
    let slot = &mut slots[chosen];
    if slot.database == database && slot.tview == tview {
        slot.at = slot.at.max(at);
    } else {
        *slot = Slot {
            database,
            tview,
            at,
        };
    }
}

/// The boundary last announced for TVIEW `tview` of this database.
#[must_use]
pub fn announced(tview: pg_sys::Oid) -> Option<pg_sys::TimestampTz> {
    let mut table = Locked::acquire(pg_sys::LWLockMode::LW_SHARED)?;
    let (database, tview) = (database(), tview.to_u32());
    table
        .slots()
        .iter()
        .find(|s| s.database == database && s.tview == tview)
        .map(|s| s.at)
}

thread_local! {
    /// The transaction recorded keys in `tviews.pg_tview_time_handoff`: its commit
    /// starts the drain.
    static HANDED_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this transaction handed keys off, cleared: the commit callback asks once.
pub fn take_handed_off() -> bool {
    HANDED_OFF.replace(false)
}

/// The transaction ended: forget whether it handed keys off.
pub fn forget() {
    HANDED_OFF.set(false);
}

/// The rows of time-dependent TVIEWs the flush that just ran wrote, checked
/// against their announced boundaries: under `BOUNDARY(V)`, held to the end of the
/// transaction, a transaction that began before `V`'s boundary records the keys it
/// wrote in `tviews.pg_tview_time_handoff` (a whole TVIEW rebuilt: a NULL key).
///
/// # Errors
/// Returns an error if the catalog cannot be read or the hand-off written.
pub fn check_flush(
    changed: &std::collections::BTreeMap<String, Vec<i64>>,
    rebuilt: &std::collections::BTreeSet<String>,
) -> TViewResult<()> {
    let entities = changed.keys().chain(rebuilt.iter());
    let mut checked = std::collections::BTreeSet::new();
    for entity in entities {
        if !checked.insert(entity.as_str()) {
            continue;
        }
        let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? else {
            continue;
        };
        if !meta.time_dependent {
            continue;
        }
        crate::concurrency::lock_boundary(meta.tview_oid.to_u32());
        let Some(boundary) = announced(meta.tview_oid) else {
            continue;
        };
        // SAFETY: a transaction is in progress (a flush runs in one).
        let started = unsafe { pg_sys::GetCurrentTransactionStartTimestamp() };
        if started >= boundary {
            continue;
        }
        let keys: Vec<Option<i64>> = if rebuilt.contains(entity) {
            vec![None]
        } else {
            changed
                .get(entity)
                .into_iter()
                .flatten()
                .copied()
                .map(Some)
                .collect()
        };
        crate::catalog::queries::hand_off(entity, &keys, boundary)?;
        HANDED_OFF.set(true);
    }
    Ok(())
}
