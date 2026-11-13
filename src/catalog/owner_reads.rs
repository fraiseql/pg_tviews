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
/// SELECT (<columns>) on <relation>"`, `"<role> lacks SELECT on every column of
/// <relation>"` or `"<role> lacks USAGE on schema <schema>"`, sorted; empty when
/// every read holds. A view reads the columns it names, or every column of a
/// table the chain reads whole or that a declared function reads ([`lacks`]).
///
/// # Errors
/// A failed catalog read.
pub fn lacking(meta: &super::TviewMeta) -> TViewResult<Vec<String>> {
    let declared: Vec<Oid> = meta
        .function_reads
        .iter()
        .flat_map(|(_, relids)| relids.iter().copied())
        .collect();
    // (reader, relation, its name, the columns the reader's views name; NULL for
    // a declared function's table).
    let reads = crate::utils::spi::catalog_rows(
        "WITH RECURSIVE views(view, depth) AS ( \
                 SELECT $1::pg_catalog.oid, 0 \
               UNION \
                 SELECT d.refobjid, v.depth OPERATOR(pg_catalog.+) 1 \
                 FROM views v \
                 JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.view \
                 JOIN pg_catalog.pg_depend d \
                   ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
                  AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                  AND d.refobjid <> v.view \
                 JOIN pg_catalog.pg_class r ON r.oid = d.refobjid AND r.relkind = 'v' \
                 WHERE v.depth OPERATOR(pg_catalog.<) 100 \
             ), needed(reader, relid, attnums) AS ( \
                 SELECT v.relowner, d.refobjid, \
                        pg_catalog.array_agg(DISTINCT d.refobjsubid::pg_catalog.int2) \
                            FILTER (WHERE d.refobjsubid OPERATOR(pg_catalog.>) 0) \
                 FROM (SELECT DISTINCT view FROM views) x \
                 JOIN pg_catalog.pg_class v ON v.oid = x.view AND v.relkind = 'v' \
                 JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid \
                 JOIN pg_catalog.pg_depend d \
                   ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass AND d.objid = w.oid \
                  AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass \
                  AND d.refobjid <> v.oid \
                 GROUP BY v.relowner, d.refobjid \
               UNION ALL \
                 SELECT t.relowner, f.relid, NULL \
                 FROM pg_catalog.pg_class t, pg_catalog.unnest($3::pg_catalog.oid[]) AS f(relid) \
                 WHERE t.oid OPERATOR(pg_catalog.=) $2 \
             ) \
             SELECT n.reader, n.relid, r.oid::pg_catalog.regclass::pg_catalog.text, n.attnums \
             FROM needed n \
             JOIN pg_catalog.pg_class r ON r.oid = n.relid AND r.relkind IN ('r', 'p', 'v', 'm', 'f')",
        &[
            crate::utils::spi::oid(meta.view_oid),
            crate::utils::spi::oid(meta.tview_oid),
            crate::utils::spi::oid_array(declared),
        ],
        |row| {
            Ok((
                row.get::<Oid>(1)?.unwrap_or(pg_sys::InvalidOid),
                row.get::<Oid>(2)?.unwrap_or(pg_sys::InvalidOid),
                row.get::<String>(3)?.unwrap_or_default(),
                row.get::<Vec<i16>>(4)?,
            ))
        },
    )?;
    let mut found = Vec::new();
    for (reader, relid, name, attnums) in reads {
        let columns = match &attnums {
            Some(attnums) if !super::stopgap::read_beyond_columns(meta.view_oid, relid) => {
                Columns::Named(attnums)
            }
            _ => Columns::Every,
        };
        if let Some(lack) = lacks(reader, relid, columns) {
            found.push(format!(
                "{} lacks {}",
                crate::owner::role_name(reader),
                lack.describe(relid, &name)
            ));
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// Whether the owner of `meta`'s TVIEW reads of every relation its plan reads
/// exactly what a write to it holds: `USAGE` on its schema, `SELECT` on each
/// column the plan reads of it ([`lacks`]: every column, where the plan's columns
/// of it are not known), and no row security applying to the owner (a TVIEW
/// under row security is refused before it gets here:
/// `catalog::stopgap::refuse_row_security`). Only then may a written value be
/// copied into the TVIEW instead of read: otherwise the read as the owner fails,
/// and the row is recomputed. False for a relation that is gone.
/// Syscache lookups only.
#[must_use]
pub fn patch_stands_for_read(meta: &super::TviewMeta) -> bool {
    let Ok(owner) = crate::owner::relation_owner(meta.tview_oid) else {
        return false;
    };
    let tables = meta.plan.tables.iter().map(|t| {
        let columns = if !t.attnums.is_empty() && super::stopgap::columns_known(meta, t.relid) {
            Columns::Named(&t.attnums)
        } else {
            Columns::Every
        };
        (Oid::from(t.relid), columns)
    });
    let declared = meta
        .function_reads
        .iter()
        .flat_map(|(_, relids)| relids.iter().map(|&relid| (relid, Columns::Every)));
    tables.chain(declared).all(|(relid, columns)| {
        lacks(owner, relid, columns).is_none()
            // SAFETY: a syscache lookup by OID; `noError` reports instead of raising.
            && unsafe { pg_sys::check_enable_rls(relid, owner, true) }
                != pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed()
    })
}

/// The columns of a relation a role reads for a TVIEW.
#[derive(Debug, Clone, Copy)]
pub enum Columns<'a> {
    /// Those, by attnum; none: the relation is read without naming a column.
    Named(&'a [i16]),
    /// Every column: what is read of it is not known column by column.
    Every,
}

/// What a role lacks to read some columns of a relation ([`lacks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lack {
    /// `USAGE` on the relation's schema.
    Schema,
    /// `SELECT` on these columns, by attnum.
    Columns(Vec<i16>),
    /// `SELECT` on every column (or the relation is gone).
    Every,
    /// `SELECT` on the relation, or on any column of it.
    Any,
}

impl Lack {
    /// What is lacking, for `relid` named `name`.
    fn describe(&self, relid: Oid, name: &str) -> String {
        match self {
            Self::Schema => {
                // SAFETY: syscache lookups by OID; null when gone.
                let schema = unsafe {
                    quoted(pg_sys::get_namespace_name(pg_sys::get_rel_namespace(relid)))
                };
                format!("USAGE on schema {}", schema.unwrap_or_default())
            }
            Self::Columns(attnums) => {
                let names: Vec<String> = attnums
                    .iter()
                    .map(|&attnum| {
                        // SAFETY: a syscache lookup by OID and attnum; null when gone.
                        unsafe { quoted(pg_sys::get_attname(relid, attnum, true)) }
                            .unwrap_or_else(|| attnum.to_string())
                    })
                    .collect();
                format!("SELECT ({}) on {name}", names.join(", "))
            }
            Self::Every => format!("SELECT on every column of {name}"),
            Self::Any => format!("SELECT on {name}"),
        }
    }
}

/// What `role` lacks to read `columns` of relation `relid`, as PostgreSQL
/// checks a query naming them: `USAGE` on its schema, and `SELECT` on the
/// relation or on each column named (on any column when none is named, on every
/// column for [`Columns::Every`]). `None` when it may read them. Write path and
/// catalog checks alike: syscache lookups only.
#[must_use]
pub fn lacks(role: Oid, relid: Oid, columns: Columns<'_>) -> Option<Lack> {
    if !crate::visibility::schema_usable(relid, role) {
        return Some(Lack::Schema);
    }
    let select = pg_sys::AclMode::from(pg_sys::ACL_SELECT);
    // SAFETY: ACL checks by OID. Each reports a relation or column that is gone
    // instead of raising, and the column checks run only on a relation found.
    unsafe {
        let mut missing = false;
        let on_table = pg_sys::pg_class_aclcheck_ext(relid, role, select, &raw mut missing);
        if missing {
            return Some(Lack::Every);
        }
        if on_table == pg_sys::AclResult::ACLCHECK_OK {
            return None;
        }
        // No column at all: said as SELECT on the relation, whatever is read.
        if pg_sys::pg_attribute_aclcheck_all(relid, role, select, pg_sys::AclMaskHow::ACLMASK_ANY)
            != pg_sys::AclResult::ACLCHECK_OK
        {
            return Some(Lack::Any);
        }
        match columns {
            Columns::Named([]) => None,
            Columns::Named(attnums) => {
                let lacking: Vec<i16> = attnums
                    .iter()
                    .copied()
                    .filter(|&attnum| {
                        let mut gone = false;
                        pg_sys::pg_attribute_aclcheck_ext(relid, attnum, role, select, &raw mut gone)
                            != pg_sys::AclResult::ACLCHECK_OK
                            || gone
                    })
                    .collect();
                (!lacking.is_empty()).then_some(Lack::Columns(lacking))
            }
            Columns::Every => (pg_sys::pg_attribute_aclcheck_all(
                relid,
                role,
                select,
                pg_sys::AclMaskHow::ACLMASK_ALL,
            ) != pg_sys::AclResult::ACLCHECK_OK)
                .then_some(Lack::Every),
        }
    }
}

/// `name` as `quote_ident` writes it; `None` for a null pointer.
///
/// # Safety
/// `name` is null or a NUL-terminated string.
unsafe fn quoted(name: *const std::ffi::c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    // SAFETY: `quote_identifier` returns its argument or a palloc'd copy, both
    // NUL-terminated.
    let quoted = unsafe { std::ffi::CStr::from_ptr(pg_sys::quote_identifier(name)) };
    Some(quoted.to_string_lossy().into_owned())
}
