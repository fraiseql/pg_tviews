//! What a TVIEW's owner may still read of what its view reads.
//!
//! A refresh reads as the TVIEW's owner, and each view it reads through reads as
//! that view's owner. A privilege granted at the create may be revoked later:
//! [`lacking`] names what each of those roles may no longer read, for
//! re-registration (which refuses the TVIEW) and the health check (which reports
//! it). A patch copies a written value instead of reading it:
//! [`patch_stands_for_read`] tells whether the owner's read would return that value,
//! in memory, for the write path.

use crate::error::TViewResult;
use pgrx::pg_sys::{self, Oid};

/// What the roles reading for `meta`'s TVIEW may no longer read: each view's
/// owner what that view reads (its backing view's owner is the TVIEW's), and the
/// TVIEW's owner the tables its declared functions read. Each is `"<role> lacks
/// SELECT on <relation>"` or `"<role> lacks USAGE on schema <schema>"`, sorted;
/// empty when every read holds.
///
/// # Errors
/// A failed catalog read.
pub fn lacking(meta: &super::TviewMeta) -> TViewResult<Vec<String>> {
    let declared: Vec<Oid> = meta
        .function_reads
        .iter()
        .flat_map(|(_, relids)| relids.iter().copied())
        .collect();
    crate::utils::spi::catalog_rows(
        "WITH RECURSIVE chain(reader, relid, depth) AS ( \
                 SELECT v.relowner, v.oid, 0 FROM pg_catalog.pg_class v \
                 WHERE v.oid OPERATOR(pg_catalog.=) $1 \
               UNION \
                 SELECT v.relowner, d.refobjid, c.depth OPERATOR(pg_catalog.+) 1 \
                 FROM chain c \
                 JOIN pg_catalog.pg_class v ON v.oid = c.relid AND v.relkind = 'v' \
                 JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
                 JOIN pg_catalog.pg_depend d \
                   ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
                  AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                  AND d.refobjid <> v.oid \
                 WHERE c.depth OPERATOR(pg_catalog.<) 100 \
             ), needed(reader, relid) AS ( \
                 SELECT DISTINCT c.reader, c.relid FROM chain c WHERE c.depth > 0 \
               UNION \
                 SELECT t.relowner, f.relid \
                 FROM pg_catalog.pg_class t, pg_catalog.unnest($3::pg_catalog.oid[]) AS f(relid) \
                 WHERE t.oid OPERATOR(pg_catalog.=) $2 \
             ), lack(reader, privilege) AS ( \
                 SELECT n.reader, pg_catalog.format('USAGE on schema %I', s.nspname) \
                 FROM needed n \
                 JOIN pg_catalog.pg_class r ON r.oid = n.relid AND r.relkind IN ('r', 'p', 'v', 'm', 'f') \
                 JOIN pg_catalog.pg_namespace s ON s.oid = r.relnamespace \
                 WHERE NOT pg_catalog.has_schema_privilege(n.reader, s.oid, 'USAGE') \
               UNION \
                 SELECT n.reader, pg_catalog.format('SELECT on %s', r.oid::pg_catalog.regclass) \
                 FROM needed n \
                 JOIN pg_catalog.pg_class r ON r.oid = n.relid AND r.relkind IN ('r', 'p', 'v', 'm', 'f') \
                 WHERE NOT pg_catalog.has_any_column_privilege(n.reader, r.oid, 'SELECT') \
             ) \
             SELECT pg_catalog.format('%s lacks %s', pg_catalog.pg_get_userbyid(l.reader), l.privilege) \
             FROM lack l ORDER BY 1",
        &[
            crate::utils::spi::oid(meta.view_oid),
            crate::utils::spi::oid(meta.tview_oid),
            crate::utils::spi::oid_array(declared),
        ],
        |row| Ok(row.get::<String>(1)?.unwrap_or_default()),
    )
}

/// Whether the owner of `meta`'s TVIEW reads of every relation its plan reads
/// exactly what a write to it holds: `USAGE` on its schema, `SELECT` on a column
/// of it, and no row security applying to the owner. Only then may a written value
/// be copied into the TVIEW instead of read: otherwise the read as the owner fails
/// or filters, and the row is recomputed. False for a relation that is gone.
/// Syscache lookups only.
#[must_use]
pub fn patch_stands_for_read(meta: &super::TviewMeta) -> bool {
    let Ok(owner) = crate::owner::relation_owner(meta.tview_oid) else {
        return false;
    };
    let tables = meta.plan.tables.iter().map(|t| Oid::from(t.relid));
    let declared = meta
        .function_reads
        .iter()
        .flat_map(|(_, relids)| relids.iter().copied());
    tables
        .chain(declared)
        .all(|relid| owner_reads_all_of(owner, relid))
}

/// Whether `owner` reads every row of `relid` (see [`patch_stands_for_read`]).
fn owner_reads_all_of(owner: Oid, relid: Oid) -> bool {
    if !crate::visibility::schema_usable(relid, owner) {
        return false;
    }
    // SAFETY: ACL checks by OID. The table's reports a relation that is gone
    // instead of raising, and the column check runs only on one found.
    unsafe {
        let mut missing = false;
        // SELECT on the table, else on one of its columns, as
        // has_any_column_privilege(): the column check alone sees neither the
        // table's grants nor a superuser.
        let select = pg_sys::AclMode::from(pg_sys::ACL_SELECT);
        let on_table = pg_sys::pg_class_aclcheck_ext(relid, owner, select, &raw mut missing);
        if missing {
            return false;
        }
        if on_table != pg_sys::AclResult::ACLCHECK_OK
            && pg_sys::pg_attribute_aclcheck_all(
                relid,
                owner,
                select,
                pg_sys::AclMaskHow::ACLMASK_ANY,
            ) != pg_sys::AclResult::ACLCHECK_OK
        {
            return false;
        }
        pg_sys::check_enable_rls(relid, owner, true)
            != pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed()
    }
}
