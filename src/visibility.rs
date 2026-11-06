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
    if !schema_usable(relid, user) {
        return false;
    }
    let mut missing = false;
    // SAFETY: an ACL check by OID; the `_ext` check reports a relation that is
    // gone instead of raising.
    unsafe {
        let select = pg_sys::pg_class_aclcheck_ext(
            relid,
            user,
            pg_sys::AclMode::from(pg_sys::ACL_SELECT),
            &raw mut missing,
        );
        !missing && select == pg_sys::AclResult::ACLCHECK_OK
    }
}

/// Whether the caller holds `mode` (`SELECT`, `UPDATE`) on `relid`, or on each
/// of its columns, and `USAGE` on its schema: what acting on the table's rows as
/// the caller needs. False for a relation that is gone, and inside an owner
/// frame.
#[must_use]
pub fn may_act_on_rows(relid: Oid, mode: u32) -> bool {
    let Ok(caller) = crate::owner::Owner::current_user() else {
        return false;
    };
    let user = caller.oid();
    let mode = pg_sys::AclMode::from(mode);
    if !schema_usable(relid, user) {
        return false;
    }
    let mut missing = false;
    // SAFETY: ACL checks by OID. The relation's `_ext` check reports one that is
    // gone instead of raising; the columns' check finds no privilege on it.
    unsafe {
        let on_table = pg_sys::pg_class_aclcheck_ext(relid, user, mode, &raw mut missing);
        !missing
            && (on_table == pg_sys::AclResult::ACLCHECK_OK
                || pg_sys::pg_attribute_aclcheck_all(
                    relid,
                    user,
                    mode,
                    pg_sys::AclMaskHow::ACLMASK_ALL,
                ) == pg_sys::AclResult::ACLCHECK_OK)
    }
}

/// Whether the caller ([`crate::owner::caller`], inside an owner frame too)
/// holds `TRIGGER` on `relid`: it may attach work to the table's writes, and so
/// lock them out while it does. False for a relation that is gone.
#[must_use]
pub fn caller_may_trigger(relid: Oid) -> bool {
    let mut missing = false;
    // SAFETY: an ACL check by OID; the `_ext` check reports a relation that is
    // gone instead of raising.
    let may = unsafe {
        pg_sys::pg_class_aclcheck_ext(
            relid,
            crate::owner::caller(),
            pg_sys::AclMode::from(pg_sys::ACL_TRIGGER),
            &raw mut missing,
        )
    };
    !missing && may == pg_sys::AclResult::ACLCHECK_OK
}

/// Whether `user` holds `USAGE` on the schema of `relid`. False for a relation
/// or schema that is gone.
fn schema_usable(relid: Oid, user: Oid) -> bool {
    // SAFETY: catalog lookups and an ACL check by OID. The schema's ACL check
    // follows a syscache probe that found it, with no invalidation processed in
    // between.
    unsafe {
        let schema = pg_sys::get_rel_namespace(relid);
        schema != pg_sys::InvalidOid
            && pg_sys::SearchSysCacheExists(
                pg_sys::SysCacheIdentifier::NAMESPACEOID.cast_signed(),
                pg_sys::Datum::from(schema),
                pg_sys::Datum::from(0),
                pg_sys::Datum::from(0),
                pg_sys::Datum::from(0),
            )
            && pg_sys::object_aclcheck(
                pg_sys::NamespaceRelationId,
                schema,
                user,
                pg_sys::AclMode::from(pg_sys::ACL_USAGE),
            ) == pg_sys::AclResult::ACLCHECK_OK
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
