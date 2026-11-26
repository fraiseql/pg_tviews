// Each line marked FLAG is reported by scripts/check-ffi-guards.sh.
unsafe extern "C" {
    pub fn raw_public(x: i32) -> bool; // FLAG: public
    fn datumIsEqual(a: i32) -> bool; // FLAG: not raw_*
    fn raw_private(x: i32) -> bool;
}

#[no_mangle]
pub unsafe extern "C-unwind" fn unguarded_callback() {} // FLAG: no #[pg_guard]

fn unguarded() -> bool {
    unsafe { raw_private(1) } // FLAG: outside the boundary
}

fn far() -> bool {
    unsafe {
        pg_sys::ffi::pg_guard_ffi_boundary(|| {
            let x = 1;
            raw_private(x) // FLAG: two lines below the boundary
        })
    }
}
