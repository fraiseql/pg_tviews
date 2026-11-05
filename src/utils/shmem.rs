//! Shared memory a preloaded library sets up at server start.

use pgrx::pg_sys;
use std::ffi::CStr;

/// The shared struct `name` of type `T`: created and initialized with `init`
/// by the first process to ask (the postmaster), found by every other one.
/// Runs under the add-in initialization lock, so `init` runs once.
///
/// # Safety
/// Called from a `shmem_startup_hook`, once the space for `T` was requested
/// with `RequestAddinShmemSpace`; `init` gets memory it must fully initialize.
pub unsafe fn init_struct<T>(name: &CStr, init: impl FnOnce(*mut T)) -> *mut T {
    // SAFETY: AddinShmemInitLock is the 22nd individual lock of the main array
    // (lwlocklist.h) on every supported server; ShmemInitStruct returns memory
    // of the requested size, found or newly allocated.
    unsafe {
        let init_lock = &raw mut (*pg_sys::MainLWLockArray.add(21)).lock;
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
