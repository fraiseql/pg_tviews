//! Shared memory a preloaded library sets up at server start.

use pgrx::pg_sys;
use std::ffi::CStr;

/// The individual lock of the main array that serializes add-in shared-memory
/// initialization: `PG_LWLOCK(21, AddinShmemInit)` in PostgreSQL 18's
/// `lwlocklist.h`. Checked on the running server before it is taken
/// ([`is_addin_init_lock`]).
const ADDIN_SHMEM_INIT: u16 = 21;

/// Whether the lock at index [`ADDIN_SHMEM_INIT`] of the main array, whose
/// tranche is `tranche` and whose tranche name is `name`, is
/// `AddinShmemInitLock`.
fn is_addin_init_lock(tranche: u16, name: &CStr) -> bool {
    tranche == ADDIN_SHMEM_INIT && name == c"AddinShmemInit"
}

/// The shared struct `name` of type `T`: created and initialized with `init`
/// by the first process to ask (the postmaster), found by every other one.
/// Runs under the add-in initialization lock, so `init` runs once. Null, with a
/// warning, on a server whose main lock array does not hold that lock where
/// PostgreSQL 18 does: what needs the struct then refuses at first use, as
/// without `shared_preload_libraries`.
///
/// # Safety
/// Called from a `shmem_startup_hook`, once the space for `T` was requested
/// with `RequestAddinShmemSpace`; `init` gets memory it must fully initialize.
pub unsafe fn init_struct<T>(name: &CStr, init: impl FnOnce(*mut T)) -> *mut T {
    // SAFETY: the main lock array is set up before any shmem_startup_hook runs,
    // and holds every individual lock; GetLWLockIdentifier returns a static
    // name. ShmemInitStruct returns memory of the requested size, found or newly
    // allocated.
    unsafe {
        let init_lock = &raw mut (*pg_sys::MainLWLockArray.add(usize::from(ADDIN_SHMEM_INIT))).lock;
        let identifier = pg_sys::GetLWLockIdentifier(pg_sys::PG_WAIT_LWLOCK, ADDIN_SHMEM_INIT);
        if identifier.is_null()
            || !is_addin_init_lock((*init_lock).tranche, CStr::from_ptr(identifier))
        {
            pgrx::warning!(
                "pg_tviews: the main lock array does not hold AddinShmemInitLock at index {}; \
                 {} is not set up",
                ADDIN_SHMEM_INIT,
                name.to_string_lossy()
            );
            return std::ptr::null_mut();
        }
        pg_sys::LWLockAcquire(init_lock, pg_sys::LWLockMode::LW_EXCLUSIVE);
        let mut found = false;
        let shared =
            pg_sys::ShmemInitStruct(name.as_ptr(), std::mem::size_of::<T>(), &raw mut found)
                .cast::<T>();
        if !found {
            init(shared);
        }
        pg_sys::LWLockRelease(init_lock);
        shared
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_addin_init_lock_at_its_index_is_taken() {
        assert!(is_addin_init_lock(21, c"AddinShmemInit"));
        assert!(!is_addin_init_lock(21, c"NotifyQueueTail"));
        assert!(!is_addin_init_lock(22, c"AddinShmemInit"));
    }
}
