/// Tagged with a theorem the document states.
///
/// Theorem: T1 (docs/proofs/theorems.md).
#[must_use]
pub fn decided() -> bool {
    true
}

/// Tagged with a theorem the document does not state.
///
/// Theorem: T99 (docs/proofs/theorems.md).
pub fn tagged_unknown() -> bool {
    true
}

/// Not tagged.
pub const fn untagged() -> bool {
    true
}
