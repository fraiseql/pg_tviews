//! What a caller may see of a relation, decided for the caller: the current user
//! outside every owner frame.
//!
//! Everything `pg_tviews` returns to a caller about rows, or reads on its
//! behalf as the caller, is decided here.

use crate::error::TViewResult;
use pgrx::pg_sys::{self, Oid};

/// Whether the caller may read `relid`: `SELECT` on it and `USAGE` on its
/// schema. False for a relation that is gone, and inside an owner frame, where
/// the current user is not the caller.
#[must_use]
pub fn may_read_relation(relid: Oid) -> bool {
    let Ok(caller) = crate::owner::Owner::current_user() else {
        return false;
    };
    let user = caller.oid();
    let mut missing = false;
    // SAFETY: catalog lookups and ACL checks by OID. The schema's ACL check
    // follows a syscache probe that found it, with no invalidation processed in
    // between; the relation's `_ext` check reports a missing one instead of
    // raising.
    unsafe {
        let schema = pg_sys::get_rel_namespace(relid);
        if schema == pg_sys::InvalidOid
            || !pg_sys::SearchSysCacheExists(
                pg_sys::SysCacheIdentifier::NAMESPACEOID.cast_signed(),
                pg_sys::Datum::from(schema),
                pg_sys::Datum::from(0),
                pg_sys::Datum::from(0),
                pg_sys::Datum::from(0),
            )
        {
            return false;
        }
        let usage = pg_sys::object_aclcheck(
            pg_sys::NamespaceRelationId,
            schema,
            user,
            pg_sys::AclMode::from(pg_sys::ACL_USAGE),
        );
        if usage != pg_sys::AclResult::ACLCHECK_OK {
            return false;
        }
        let select = pg_sys::pg_class_aclcheck_ext(
            relid,
            user,
            pg_sys::AclMode::from(pg_sys::ACL_SELECT),
            &raw mut missing,
        );
        !missing && select == pg_sys::AclResult::ACLCHECK_OK
    }
}

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
