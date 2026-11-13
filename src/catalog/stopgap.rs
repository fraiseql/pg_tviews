//! When what a TVIEW's plan says it reads of a table is not all a refresh reads.
//!
//! A write may skip a TVIEW whose plan reads none of the columns it changed, and
//! may copy a value into a document instead of recomputing it, only on the
//! plan's word. Its column list of a table comes from `pg_depend`, which records
//! the columns the definition names, and not:
//!
//! - what it reads of the table through a whole row (`to_jsonb(t)`, `f(t)`,
//!   `t::text`, a join alias) or a system column (`xmin`, `ctid`);
//! - what a declared function reads of it;
//! - what a policy that applies to the TVIEW's owner reads, nor which rows of it
//!   the owner reads (row security is checked at the time of the write);
//! - anything, when the plan is from an earlier release (`needs_reregister`).
//!
//! No write to such a table is skipped or patched for that TVIEW: its columns are
//! unknown ([`columns_known`]). Under row security, none to any table it reads
//! ([`plan_reads_hold`]).

use crate::lineage::walk::{WholeRead, WholeReads};
use pgrx::pg_sys::{self, Oid};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Entity → the command its row-security tables were read in, and those tables.
type OwnerRls = HashMap<String, (pg_sys::CommandId, Rc<[u32]>)>;

thread_local! {
    /// Forgotten at the end of the transaction.
    static OWNER_RLS: RefCell<OwnerRls> = RefCell::new(HashMap::new());
    /// Backing view → what it reads beyond the columns it names; dropped when a
    /// view it walked changes.
    static WHOLE_READS: crate::cache::Memo<u32, Rc<WholeReads>> = crate::cache::Memo::new();
}

/// Whether a write to table `relid` may skip `meta`'s TVIEW, or patch it, on
/// the columns its plan says it reads of the table: the plan's reads hold
/// ([`plan_reads_hold`]), no declared function reads the table, and the
/// definition does not read it whole or through a system column.
#[must_use]
pub fn columns_known(meta: &super::TviewMeta, relid: u32) -> bool {
    plan_reads_hold(meta)
        && !read_by_function(meta, relid)
        && matches!(whole_read(meta.view_oid, relid), Ok(None))
}

/// Whether a write may skip or patch `meta`'s TVIEW on what its plan says it
/// reads at all: the plan is this release's, and no row security applies to its
/// owner on a table it reads ([`owner_rls_tables`]).
#[must_use]
pub fn plan_reads_hold(meta: &super::TviewMeta) -> bool {
    meta.plan_release == super::PlanRelease::This && owner_rls_tables(meta).is_empty()
}

/// The tables `meta`'s TVIEW reads (its plan's, and those its declared functions
/// read) on which row security applies to the TVIEW's owner now, as a refresh
/// reads them. Every table it reads when the owner is unknown. Syscache lookups,
/// once per TVIEW and command.
#[must_use]
pub fn owner_rls_tables(meta: &super::TviewMeta) -> Rc<[u32]> {
    // SAFETY: reads the transaction's current command id.
    let command = unsafe { pg_sys::GetCurrentCommandId(false) };
    if let Some(tables) = OWNER_RLS.with_borrow(|m| {
        m.get(&meta.entity_name)
            .filter(|(at, _)| *at == command)
            .map(|(_, tables)| Rc::clone(tables))
    }) {
        return tables;
    }
    let read = meta.plan.tables.iter().map(|t| t.relid).chain(
        meta.function_reads
            .iter()
            .flat_map(|(_, relids)| relids.iter().map(|r| r.to_u32())),
    );
    let tables: Rc<[u32]> = match crate::owner::relation_owner(meta.tview_oid) {
        Ok(owner) => read
            .filter(|&relid| {
                // SAFETY: a syscache lookup by OID; `noError` reports instead of
                // raising, and a relation that is gone has no row security.
                unsafe {
                    pg_sys::check_enable_rls(Oid::from(relid), owner, true)
                        == pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed()
                }
            })
            .collect(),
        Err(_) => read.collect(),
    };
    OWNER_RLS.with_borrow_mut(|m| {
        m.insert(meta.entity_name.clone(), (command, Rc::clone(&tables)));
    });
    tables
}

/// The transaction ended: forget the row-security tables read in it.
pub fn forget() {
    OWNER_RLS.with_borrow_mut(HashMap::clear);
}

/// Whether a function declared with `meta`'s TVIEW reads table `relid`.
fn read_by_function(meta: &super::TviewMeta, relid: u32) -> bool {
    meta.function_reads
        .iter()
        .any(|(_, relids)| relids.iter().any(|r| r.to_u32() == relid))
}

/// How backing view `view` reads table `relid` beyond the columns it names
/// (`None`: it does not). The walk is memoised until a view it walked changes.
///
/// # Errors
/// A view of the chain cannot be opened.
fn whole_read(view: Oid, relid: u32) -> crate::TViewResult<Option<WholeRead>> {
    let reads = WHOLE_READS.with(|memo| {
        memo.get_or_load(view.to_u32(), || {
            let reads = crate::lineage::walk::whole_reads(view)?;
            crate::cache::watch(&reads.views);
            Ok(Rc::new(reads))
        })
    });
    reads.map(|reads| reads.tables.get(&relid).copied())
}

/// Test builds: why a write to table `relid` may not skip or patch `meta`'s
/// TVIEW on its plan's columns (`stopgap:<reason>`, counted by the caller).
#[cfg(feature = "test-hooks")]
#[must_use]
pub fn reasons(meta: &super::TviewMeta, relid: u32) -> Vec<&'static str> {
    let mut reasons: Vec<&'static str> = Vec::new();
    if meta.plan_release == super::PlanRelease::Earlier {
        reasons.push("reregister");
    }
    if !owner_rls_tables(meta).is_empty() {
        reasons.push("rls");
    }
    if read_by_function(meta, relid) {
        reasons.push("fnread");
    }
    match whole_read(meta.view_oid, relid) {
        Ok(Some(read)) => {
            if read.whole_row {
                reasons.push("wholerow");
            }
            if read.system_column {
                reasons.push("syscol");
            }
        }
        Ok(None) => {}
        Err(_) => reasons.push("unwalked"),
    }
    reasons
}
