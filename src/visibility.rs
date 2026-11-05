//! What a caller may see of a relation, decided for the caller: the current user
//! outside every owner frame.
//!
//! Everything `pg_tviews` returns to a caller about rows, or reads on its
//! behalf as the caller, is decided here.

use crate::error::TViewResult;
use pgrx::pg_sys::{self, Oid};

/// Whether row-level security applies to the caller's reads of `relid`: then a
/// read as the caller sees only the rows its policies allow.
///
/// # Errors
/// An internal error inside an owner frame.
pub fn rls_active_for_caller(relid: Oid) -> TViewResult<bool> {
    let caller = crate::owner::Owner::current_user()?;
    // SAFETY: an RLS check by OID for a role; noError returns RLS_NONE for a
    // relation that is gone.
    let rls = unsafe { pg_sys::check_enable_rls(relid, caller.oid(), true) };
    Ok(rls == pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed())
}
