//! The inputs of the decisions ([`crate::decide`]) that are facts about the
//! database: each computed here, by a named function, outside the decision.
//! theorems.md lists them as the assumptions the decisions' proofs rest on.
//!
//! - [`read_set`] and [`path_read_set`]: what the stored plan says a TVIEW reads
//!   of a table (`Unknown` when the plan stores no read set);
//! - [`env`]: the plan is this release's, the backing view's chain still reads the
//!   table through the columns it names only, the TVIEW's owner may read them,
//!   and `pg_tviews.direct_patch` is on;
//! - [`copy_is_exact`]: a column's value, copied into `data` by a patch, renders
//!   as `to_jsonb()` renders it.

use crate::decide::{Env, ReadSet};
use pgrx::pg_sys::{self, Oid};
use std::cell::RefCell;
use std::collections::HashMap;

/// `(entity, relid)` → the command the owner's privileges were checked in, and
/// the answer.
type OwnerReads = HashMap<(String, u32), (pg_sys::CommandId, bool)>;

thread_local! {
    /// Forgotten at the end of the transaction ([`forget`]).
    static OWNER_READS: RefCell<OwnerReads> = RefCell::new(HashMap::new());
}

/// A stored read set as the decisions take it: `None` is `Unknown`.
#[must_use]
pub fn stored(read: Option<&[i16]>) -> ReadSet {
    read.map_or(ReadSet::Unknown, |read| {
        ReadSet::Complete(read.iter().collect())
    })
}

/// What `meta`'s plan says its TVIEW reads of table `relid`.
#[must_use]
pub fn read_set(meta: &super::TviewMeta, relid: u32) -> ReadSet {
    stored(
        meta.key_mapping(Oid::from(relid), None)
            .and_then(|m| m.read_set.as_deref()),
    )
}

/// What a local path's TVIEW reads of the path's table.
#[must_use]
pub fn path_read_set(path: &crate::lineage::plan::LocalPath) -> ReadSet {
    stored(path.read_set.as_deref())
}

/// The facts a decision about `meta`'s TVIEW and a write to table `relid` takes.
#[must_use]
pub fn env(meta: &super::TviewMeta, relid: u32) -> Env {
    Env {
        plan_current: super::stopgap::plan_reads_hold(meta),
        chain_reads_named: !super::stopgap::read_beyond_columns(meta.view_oid, Oid::from(relid)),
        owner_reads_all: owner_reads_all(meta, relid),
        direct_patch_enabled: crate::config::direct_patch_enabled(),
    }
}

/// The facts of a patch: [`env`], with `owner_reads_all` also requiring that
/// the owner reads every relation the plan reads unfiltered
/// ([`super::owner_reads::patch_stands_for_read`]): a patch stands for the
/// owner's read of all of them.
#[must_use]
pub fn patch_env(meta: &super::TviewMeta, relid: u32) -> Env {
    let mut env = env(meta, relid);
    env.owner_reads_all = env.owner_reads_all && super::owner_reads::patch_stands_for_read(meta);
    env
}

/// Whether the owner of `meta`'s TVIEW holds the privileges to read what the
/// TVIEW reads of table `relid` (USAGE on its schema, SELECT on those columns,
/// or on every column when they are not known). Syscache lookups, once per
/// TVIEW, table and command.
fn owner_reads_all(meta: &super::TviewMeta, relid: u32) -> bool {
    // SAFETY: reads the transaction's current command id.
    let command = unsafe { pg_sys::GetCurrentCommandId(false) };
    let key = (meta.entity_name.clone(), relid);
    if let Some(answer) = OWNER_READS.with_borrow(|m| {
        m.get(&key)
            .filter(|(at, _)| *at == command)
            .map(|(_, answer)| *answer)
    }) {
        return answer;
    }
    let answer = crate::owner::relation_owner(meta.tview_oid).is_ok_and(|owner| {
        let read = meta
            .key_mapping(Oid::from(relid), None)
            .and_then(|m| m.read_set.clone());
        let columns = read.as_deref().map_or(
            super::owner_reads::Columns::Every,
            super::owner_reads::Columns::Named,
        );
        super::owner_reads::lacks(owner, Oid::from(relid), columns).is_none()
    });
    OWNER_READS.with_borrow_mut(|m| {
        m.insert(key, (command, answer));
    });
    answer
}

/// The transaction ended: forget the privilege checks run in it.
pub fn forget() {
    OWNER_READS.with_borrow_mut(HashMap::clear);
}

/// Whether column `attnum` of relation `relid`, copied into `data` by a patch,
/// renders exactly as `to_jsonb()` renders it (G2): a type whose JSON form is a
/// string, a boolean or an integer (a domain over one, an enum). Not `numeric`,
/// the floating types, `json`, `jsonb`, arrays or composites, which may hold a
/// number a copy through `serde_json` would change. Syscache lookups.
#[must_use]
pub fn copy_is_exact(relid: u32, attnum: i16) -> bool {
    // SAFETY: syscache lookups by OID and attribute number; an unknown column
    // gives InvalidOid.
    unsafe {
        let typid = pg_sys::get_atttype(Oid::from(relid), attnum);
        if typid == pg_sys::InvalidOid {
            return false;
        }
        let base = pg_sys::getBaseType(typid);
        if pg_sys::get_typtype(base).cast_unsigned() == pg_sys::TYPTYPE_ENUM {
            return true;
        }
        matches!(
            base,
            pg_sys::BOOLOID
                | pg_sys::INT2OID
                | pg_sys::INT4OID
                | pg_sys::INT8OID
                | pg_sys::TEXTOID
                | pg_sys::VARCHAROID
                | pg_sys::BPCHAROID
                | pg_sys::NAMEOID
                | pg_sys::CHAROID
                | pg_sys::UUIDOID
                | pg_sys::DATEOID
                | pg_sys::TIMEOID
                | pg_sys::TIMETZOID
                | pg_sys::TIMESTAMPOID
                | pg_sys::TIMESTAMPTZOID
                | pg_sys::INTERVALOID
        )
    }
}
