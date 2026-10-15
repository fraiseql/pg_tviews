//! Per-TVIEW refresh statistics, readable from any session (ADR 0221).
//!
//! A backend adds to a local map while its transaction refreshes TVIEWs, and
//! merges it into shared memory when the transaction ends, committed or not: one
//! lock acquisition per transaction, no SPI. The shared table holds a fixed number
//! of TVIEWs for the whole cluster, keyed by database and table; a TVIEW that
//! finds it full is untracked, and the view says so. Counters start over at a
//! restart, a reset, and a rebuild (a new table).

use pgrx::pg_guard;
use pgrx::pg_sys::{self, Oid};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// TVIEWs the cluster tracks (a power of two).
const CAPACITY: usize = 4096;

/// What a refresh counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// Rows recomputed from the backing view.
    ViewRecomputes,
    /// Refreshes skipped because the row already held the result.
    NoopSkipped,
    /// Direct patches captured by a row trigger.
    PatchCaptured,
    /// Rows patched in place.
    PatchApplied,
    /// Patched rows recomputed instead.
    PatchFallbacks,
    /// Parent lookups skipped because the child's row did not change.
    PropagationPruned,
    /// Rows inserted or updated.
    RowsWritten,
    /// Rows deleted.
    RowsDeleted,
    /// Whole-TVIEW refreshes.
    FullRefreshes,
    /// Microseconds spent refreshing.
    RefreshMicros,
}

const COUNTERS: usize = 10;

/// One TVIEW's counters. `db` 0 marks a free slot (no database has OID 0): the
/// all-zero slot is free, so zeroed memory is an empty table.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Slot {
    db: u32,
    table: u32,
    since: pg_sys::TimestampTz,
    counts: [u64; COUNTERS],
}

impl Slot {
    const FREE: Self = Self {
        db: 0,
        table: 0,
        since: 0,
        counts: [0; COUNTERS],
    };
}

/// The cluster's table, in shared memory; all zeros is empty.
#[repr(C)]
pub struct Table {
    slots: [Slot; CAPACITY],
    /// A TVIEW found the table full since the last reset.
    overflowed: bool,
}

impl Table {
    fn home(db: u32, table: u32) -> usize {
        // Fibonacci hashing of the pair.
        let key = (u64::from(db) << 32) | u64::from(table);
        let mixed = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        usize::try_from(mixed >> (64 - CAPACITY.trailing_zeros())).unwrap_or(0)
    }

    /// The slot of `(db, table)`, if it has one.
    fn find(&self, db: u32, table: u32) -> Option<usize> {
        let mut i = Self::home(db, table);
        for _ in 0..CAPACITY {
            let slot = &self.slots[i];
            if slot.db == 0 {
                return None;
            }
            if slot.db == db && slot.table == table {
                return Some(i);
            }
            i = (i + 1) % CAPACITY;
        }
        None
    }

    /// The slot of `(db, table)`, taken now if it had none; `None` when full.
    fn find_or_take(&mut self, db: u32, table: u32, now: pg_sys::TimestampTz) -> Option<usize> {
        let mut i = Self::home(db, table);
        for _ in 0..CAPACITY {
            let slot = &mut self.slots[i];
            if slot.db == db && slot.table == table {
                return Some(i);
            }
            if slot.db == 0 {
                *slot = Slot {
                    db,
                    table,
                    since: now,
                    counts: [0; COUNTERS],
                };
                return Some(i);
            }
            i = (i + 1) % CAPACITY;
        }
        None
    }

    /// Free slot `i`, moving back the slots after it that probed past it.
    fn free(&mut self, mut i: usize) {
        let mut j = i;
        loop {
            self.slots[i] = Slot::FREE;
            loop {
                j = (j + 1) % CAPACITY;
                let slot = self.slots[j];
                if slot.db == 0 {
                    self.overflowed = false;
                    return;
                }
                let home = Self::home(slot.db, slot.table);
                // Slot j stays unless its home lies cyclically in (i, j].
                let stays = if i <= j {
                    i < home && home <= j
                } else {
                    i < home || home <= j
                };
                if !stays {
                    self.slots[i] = slot;
                    i = j;
                    break;
                }
            }
        }
    }
}

const NAME: &CStr = c"pg_tviews_stats";

/// The shared table and its lock, once the startup hook found them; null when
/// the library was not preloaded.
static TABLE: AtomicPtr<Table> = AtomicPtr::new(std::ptr::null_mut());
static LOCK: AtomicPtr<pg_sys::LWLock> = AtomicPtr::new(std::ptr::null_mut());

static mut PREV_SHMEM_REQUEST: pg_sys::shmem_request_hook_type = None;
static mut PREV_SHMEM_STARTUP: pg_sys::shmem_startup_hook_type = None;

/// The shared table under its lock, shared or exclusive; released on drop.
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
}

impl std::ops::Deref for Locked {
    type Target = Table;
    fn deref(&self) -> &Table {
        // SAFETY: the shared table, held under its lock while `self` lives.
        unsafe { &*self.table }
    }
}

impl std::ops::DerefMut for Locked {
    fn deref_mut(&mut self) -> &mut Table {
        // SAFETY: as in `deref`; exclusive callers took the lock exclusively.
        unsafe { &mut *self.table }
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
    // SAFETY: called by shmem_startup_hook; ShmemInitStruct runs under the addin
    // initialization lock, and a table found already is not touched.
    unsafe {
        if let Some(prev) = PREV_SHMEM_STARTUP {
            pg_sys::ffi::pg_guard_ffi_boundary(|| prev());
        }
        let init_lock = &raw mut (*pg_sys::MainLWLockArray.add(21)).lock;
        pg_sys::LWLockAcquire(init_lock, pg_sys::LWLockMode::LW_EXCLUSIVE);
        let mut found = false;
        let table =
            pg_sys::ShmemInitStruct(NAME.as_ptr(), std::mem::size_of::<Table>(), &raw mut found)
                .cast::<Table>();
        if !found {
            std::ptr::write_bytes(table, 0, 1);
        }
        TABLE.store(table, Ordering::Release);
        LOCK.store(
            &raw mut (*pg_sys::GetNamedLWLockTranche(NAME.as_ptr())).lock,
            Ordering::Release,
        );
        pg_sys::LWLockRelease(init_lock);
    }
}

thread_local! {
    /// This transaction's additions: entity -> (table, counts).
    static LOCAL: RefCell<HashMap<String, (Oid, [u64; COUNTERS])>> = RefCell::new(HashMap::new());
}

/// Request the shared table. Only while the library is preloaded: a session
/// loading it later has no shared memory to ask for, and `tviews.stats` says so.
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

/// Whether the shared table exists in this server.
#[must_use]
pub fn available() -> bool {
    !TABLE.load(Ordering::Acquire).is_null()
}

/// Add `n` to `counter` of `entity`'s TVIEW, for this transaction. A TVIEW whose
/// catalog row cannot be read is not counted.
pub fn add(entity: &str, counter: Counter, n: u64) {
    if n == 0 || !available() {
        return;
    }
    let known = LOCAL.with_borrow_mut(|local| {
        local.get_mut(entity).map(|(_, counts)| {
            counts[counter as usize] = counts[counter as usize].saturating_add(n);
        })
    });
    if known.is_some() {
        return;
    }
    let Ok(Some(meta)) = crate::catalog::TviewMeta::load_by_entity(entity) else {
        return;
    };
    LOCAL.with_borrow_mut(|local| {
        let (_, counts) = local
            .entry(entity.to_string())
            .or_insert((meta.tview_oid, [0; COUNTERS]));
        counts[counter as usize] = counts[counter as usize].saturating_add(n);
    });
}

/// Merge this transaction's additions into the shared table, at its end however
/// it ends. Runs in a transaction callback: no SPI, nothing that can fail.
pub fn merge() {
    let local = LOCAL.with_borrow_mut(std::mem::take);
    if local.is_empty() || !available() {
        return;
    }
    // SAFETY: reads backend globals.
    let (db, now) = unsafe { (pg_sys::MyDatabaseId.to_u32(), pg_sys::GetCurrentTimestamp()) };
    let Some(mut table) = Locked::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE) else {
        return;
    };
    for (oid, counts) in local.into_values() {
        match table.find_or_take(db, oid.to_u32(), now) {
            Some(i) => {
                for (total, n) in table.slots[i].counts.iter_mut().zip(counts) {
                    *total = total.saturating_add(n);
                }
            }
            None => table.overflowed = true,
        }
    }
}

/// One TVIEW's counters as `tviews.stats` shows them, and when they started.
pub struct Counts {
    pub counts: [u64; COUNTERS],
    pub since: pg_sys::TimestampTz,
}

impl Counts {
    #[must_use]
    pub const fn get(&self, counter: Counter) -> u64 {
        self.counts[counter as usize]
    }
}

/// The counters of the current database's TVIEWs, by table, and whether the
/// table overflowed (a TVIEW missing from it is then untracked, not idle).
///
/// # Errors
/// [`crate::TViewError::WrongState`] when the library was not preloaded.
pub fn read() -> crate::TViewResult<(HashMap<Oid, Counts>, bool)> {
    // SAFETY: reads a backend global.
    let db = unsafe { pg_sys::MyDatabaseId.to_u32() };
    let table = Locked::acquire(pg_sys::LWLockMode::LW_SHARED).ok_or_else(unavailable)?;
    let counts = table
        .slots
        .iter()
        .filter(|slot| slot.db == db)
        .map(|slot| {
            (
                Oid::from(slot.table),
                Counts {
                    counts: slot.counts,
                    since: slot.since,
                },
            )
        })
        .collect();
    Ok((counts, table.overflowed))
}

/// Zero the counters of `table` (one TVIEW of the current database), or of
/// every TVIEW of the current database.
///
/// # Errors
/// [`crate::TViewError::WrongState`] when the library was not preloaded.
pub fn reset(table: Option<Oid>) -> crate::TViewResult<()> {
    // SAFETY: reads backend globals.
    let (db, now) = unsafe { (pg_sys::MyDatabaseId.to_u32(), pg_sys::GetCurrentTimestamp()) };
    let mut shared = Locked::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE).ok_or_else(unavailable)?;
    if let Some(table) = table {
        if let Some(i) = shared.find(db, table.to_u32()) {
            shared.slots[i].counts = [0; COUNTERS];
            shared.slots[i].since = now;
        }
    } else {
        while let Some(i) = shared.slots.iter().position(|slot| slot.db == db) {
            shared.free(i);
        }
        shared.overflowed = false;
    }
    Ok(())
}

/// Forget `table`, dropped or replaced by a rebuild.
pub fn forget(table: Oid) {
    // SAFETY: reads a backend global.
    let db = unsafe { pg_sys::MyDatabaseId.to_u32() };
    if let Some(mut shared) = Locked::acquire(pg_sys::LWLockMode::LW_EXCLUSIVE)
        && let Some(i) = shared.find(db, table.to_u32())
    {
        shared.free(i);
    }
}

fn unavailable() -> crate::TViewError {
    crate::TViewError::WrongState {
        reason: "tviews.stats needs the library preloaded: \
                 shared_preload_libraries = 'pg_tviews', then restart"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, Slot, Table};

    /// A zeroed table on the heap, as shared memory starts.
    fn table() -> Box<Table> {
        let layout = std::alloc::Layout::new::<Table>();
        #[allow(clippy::cast_ptr_alignment)] // Reason: alloc_zeroed honors the layout's alignment
        // SAFETY: allocated with Table's own layout (so aligned for it), and an
        // all-zero Table is valid (free slots, not overflowed).
        unsafe {
            Box::from_raw(std::alloc::alloc_zeroed(layout).cast::<Table>())
        }
    }

    #[test]
    fn slots_are_found_taken_and_freed() {
        let mut t = table();
        let a = t.find_or_take(1, 100, 0).unwrap();
        let b = t.find_or_take(1, 200, 0).unwrap();
        assert_ne!(a, b);
        assert_eq!(t.find(1, 100), Some(a));
        assert_eq!(t.find(2, 100), None);
        t.free(a);
        assert_eq!(t.find(1, 100), None);
        assert_eq!(t.find(1, 200).map(|i| t.slots[i].table), Some(200));
    }

    #[test]
    fn freeing_keeps_every_probe_chain_reachable() {
        let mut t = table();
        // Fill a quarter of the table, free every other entry, and check the rest.
        let keys: Vec<u32> = (1..=u32::try_from(CAPACITY / 4).unwrap()).collect();
        for &k in &keys {
            t.find_or_take(7, k, 0).unwrap();
        }
        for &k in keys.iter().step_by(2) {
            let i = t.find(7, k).unwrap();
            t.free(i);
        }
        for (n, &k) in keys.iter().enumerate() {
            assert_eq!(t.find(7, k).is_some(), n % 2 == 1, "key {k}");
        }
    }

    #[test]
    fn a_full_table_takes_no_more() {
        let mut t = table();
        for k in 0..u32::try_from(CAPACITY).unwrap() {
            assert!(t.find_or_take(1, k + 1, 0).is_some());
        }
        assert!(t.find_or_take(2, 1, 0).is_none());
        assert!(t.slots.iter().all(|s: &Slot| s.db != 0));
    }
}
