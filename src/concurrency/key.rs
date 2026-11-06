//! The server's secret key for value-lock tags: 128 bits from a strong random
//! source, made once at server start in shared memory. A value lock's tag is a
//! keyed hash of the value, so the values behind the tags `pg_locks` shows to
//! every role can't be computed without the key.

use pgrx::pg_guard;
use pgrx::pg_sys;
use std::ffi::CStr;
use std::sync::atomic::{AtomicPtr, Ordering};

const NAME: &CStr = c"pg_tviews_lock_key";

/// The key's bytes.
pub type Key = [u8; 16];

/// The shared key, once the startup hook found it; null when the library was
/// not preloaded.
static KEY: AtomicPtr<Key> = AtomicPtr::new(std::ptr::null_mut());

static mut PREV_SHMEM_REQUEST: pg_sys::shmem_request_hook_type = None;
static mut PREV_SHMEM_STARTUP: pg_sys::shmem_startup_hook_type = None;

#[pg_guard]
unsafe extern "C-unwind" fn request_shmem() {
    // SAFETY: called by the postmaster's shmem_request_hook, once.
    unsafe {
        if let Some(prev) = PREV_SHMEM_REQUEST {
            pg_sys::ffi::pg_guard_ffi_boundary(|| prev());
        }
        pg_sys::RequestAddinShmemSpace(std::mem::size_of::<Key>());
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn startup_shmem() {
    // SAFETY: called by shmem_startup_hook, after request_shmem asked for the
    // key's space; a key found already is not touched.
    unsafe {
        if let Some(prev) = PREV_SHMEM_STARTUP {
            pg_sys::ffi::pg_guard_ffi_boundary(|| prev());
        }
        let key = crate::utils::shmem::init_struct::<Key>(NAME, |key| {
            if !pg_sys::pg_strong_random(key.cast(), std::mem::size_of::<Key>()) {
                pgrx::error!("pg_tviews: could not generate the value-lock key");
            }
        });
        KEY.store(key, Ordering::Release);
    }
}

/// Request the shared key. Only while the library is preloaded: a session
/// loading it later has no shared memory to ask for, and takes no value lock.
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

/// The server's key.
///
/// # Errors
/// [`crate::TViewError::WrongState`] when the library was not preloaded.
pub fn current() -> crate::TViewResult<Key> {
    let key = KEY.load(Ordering::Acquire);
    if key.is_null() {
        return Err(crate::TViewError::WrongState {
            reason: "pg_tviews takes value locks only when it is in shared_preload_libraries"
                .to_string(),
        });
    }
    // SAFETY: the shared key, written once before any backend started.
    Ok(unsafe { *key })
}
