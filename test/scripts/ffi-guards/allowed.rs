// Nothing here is reported by scripts/check-ffi-guards.sh.
unsafe extern "C" {
    fn raw_private(x: i32) -> bool;
}

#[pg_guard]
pub unsafe extern "C-unwind" fn guarded_callback() {}

fn same_line() -> bool {
    unsafe { pg_sys::ffi::pg_guard_ffi_boundary(|| raw_private(1)) }
}

fn next_line() -> bool {
    unsafe {
        pg_sys::ffi::pg_guard_ffi_boundary(|| {
            raw_private(2)
        })
    }
}
