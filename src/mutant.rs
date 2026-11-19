//! Skip mutants: in a test build (feature `test-mutants`), the skip decision named
//! by `pg_tviews.test_mutant` (`S03`, … as in `test/sql/differential/mutants/`)
//! decides wrongly on purpose, so the differential generator must see a stale
//! TVIEW. A column-set decision misses the last column of its input set; a
//! predicate decision skips one class of inputs it must not. Without the feature
//! every switch is off and compiles away.

/// Whether the mutant `id` is the session's.
#[cfg(feature = "test-mutants")]
pub fn on(id: &str) -> bool {
    crate::config::test_mutant().is_some_and(|m| m == id)
}

/// Whether the mutant `id` is the session's: never, outside a mutants build.
#[cfg(not(feature = "test-mutants"))]
#[inline]
pub const fn on(_id: &str) -> bool {
    false
}

/// `set` without its last element when the mutant `id` is on.
#[cfg(feature = "test-mutants")]
pub fn drop_last<'a, T>(id: &str, set: &'a [T]) -> &'a [T] {
    match set.split_last() {
        Some((_, rest)) if on(id) => rest,
        _ => set,
    }
}

/// `set`: no mutant is on outside a mutants build.
#[cfg(not(feature = "test-mutants"))]
#[inline]
pub const fn drop_last<'a, T>(_id: &str, set: &'a [T]) -> &'a [T] {
    set
}
