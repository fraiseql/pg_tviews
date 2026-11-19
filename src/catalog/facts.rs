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

/// `(TVIEW table OID, relid)` → the command the owner's privileges were checked
/// in, and the answer.
type OwnerReads = HashMap<(u32, u32), (pg_sys::CommandId, bool)>;

/// `(TVIEW table OID, relid, of a patch)` → the command and cache epoch
/// ([`crate::cache::epoch`]) the facts were established in, and them
/// (`direct_patch_enabled` aside: read each time).
type Facts = HashMap<(u32, u32, bool), (pg_sys::CommandId, u64, Env)>;

thread_local! {
    /// Forgotten at the end of the transaction ([`forget`]).
    static OWNER_READS: RefCell<OwnerReads> = RefCell::new(HashMap::new());
    /// Forgotten at the end of the transaction ([`forget`]).
    static FACTS: RefCell<Facts> = RefCell::new(HashMap::new());
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
/// Established once per command and cache epoch ([`facts`]).
#[must_use]
pub fn env(meta: &super::TviewMeta, relid: u32) -> Env {
    facts(meta, relid, false)
}

/// The facts of a patch: [`env`], with `owner_reads_all` also requiring that
/// the owner reads every relation the plan reads unfiltered
/// ([`super::owner_reads::patch_stands_for_read`]): a patch stands for the
/// owner's read of all of them.
#[must_use]
pub fn patch_env(meta: &super::TviewMeta, relid: u32) -> Env {
    facts(meta, relid, true)
}

/// [`env`] or [`patch_env`], established once per command and cache epoch: what
/// they rest on (the plan, the views of the chain, the owner's privileges) is
/// itself read once per command (the privileges) or until a view changes (the
/// chain), and a catalog change in this backend starts a new command.
/// `direct_patch_enabled` is read each time.
fn facts(meta: &super::TviewMeta, relid: u32, patch: bool) -> Env {
    // SAFETY: reads the transaction's current command id.
    let command = unsafe { pg_sys::GetCurrentCommandId(false) };
    let epoch = crate::cache::epoch();
    let key = (meta.tview_oid.to_u32(), relid, patch);
    let known = FACTS.with_borrow(|m| {
        m.get(&key)
            .filter(|(at, in_epoch, _)| *at == command && *in_epoch == epoch)
            .map(|(_, _, env)| *env)
    });
    let mut env = known.unwrap_or_else(|| {
        let mut env = Env {
            plan_current: super::stopgap::plan_reads_hold(meta),
            chain_reads_named: !super::stopgap::read_beyond_columns(
                meta.view_oid,
                Oid::from(relid),
            ),
            owner_reads_all: owner_reads_all(meta, relid),
            direct_patch_enabled: false,
        };
        if patch {
            env.owner_reads_all =
                env.owner_reads_all && super::owner_reads::patch_stands_for_read(meta);
        }
        FACTS.with_borrow_mut(|m| {
            m.insert(key, (command, epoch, env));
        });
        env
    });
    env.direct_patch_enabled = crate::config::direct_patch_enabled();
    env
}

/// Whether the owner of `meta`'s TVIEW holds the privileges to read what the
/// TVIEW reads of table `relid` (USAGE on its schema, SELECT on those columns,
/// or on every column when they are not known). Syscache lookups, once per
/// TVIEW, table and command.
fn owner_reads_all(meta: &super::TviewMeta, relid: u32) -> bool {
    // SAFETY: reads the transaction's current command id.
    let command = unsafe { pg_sys::GetCurrentCommandId(false) };
    let key = (meta.tview_oid.to_u32(), relid);
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
    FACTS.with_borrow_mut(HashMap::clear);
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
