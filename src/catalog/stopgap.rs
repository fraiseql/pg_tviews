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
//! - anything, when the plan is from an earlier release (`needs_reregister`).
//!
//! No write to such a table is skipped or patched for that TVIEW: its columns are
//! unknown ([`columns_known`]).
//!
//! A refresh reads every row of every table its TVIEW reads: a TVIEW is refused
//! while row security applies on one of them to a role it reads that table as
//! ([`refuse_row_security`]).

use crate::error::{TViewError, TViewResult};
use crate::lineage::walk::{WholeRead, WholeReads};
use pgrx::pg_sys::{self, Oid};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Entity → the command its row-security probe ran in, and its refusal.
type RowSecurity = HashMap<String, (pg_sys::CommandId, Option<TViewError>)>;

thread_local! {
    /// Forgotten at the end of the transaction.
    static ROW_SECURITY: RefCell<RowSecurity> = RefCell::new(HashMap::new());
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
/// reads at all: the plan is this release's.
#[must_use]
pub fn plan_reads_hold(meta: &super::TviewMeta) -> bool {
    meta.plan_release == super::PlanRelease::This
}

/// Refuse to refresh `meta`'s TVIEW, or to skip or patch it, while row security
/// applies on a table it reads (its plan's, and those its declared functions
/// read) to a role it reads that table as ([`reading_roles`]). Syscache lookups,
/// once per TVIEW and command.
///
/// # Errors
/// [`TViewError::RowSecurity`] naming the table and the role.
pub fn refuse_row_security(meta: &super::TviewMeta) -> TViewResult<()> {
    // SAFETY: reads the transaction's current command id.
    let command = unsafe { pg_sys::GetCurrentCommandId(false) };
    if let Some(refusal) = ROW_SECURITY.with_borrow(|m| {
        m.get(&meta.entity_name)
            .filter(|(at, _)| *at == command)
            .map(|(_, refusal)| refusal.clone())
    }) {
        return refusal.map_or(Ok(()), Err);
    }
    let refusal = check_row_security(meta).err();
    ROW_SECURITY.with_borrow_mut(|m| {
        m.insert(meta.entity_name.clone(), (command, refusal.clone()));
    });
    refusal.map_or(Ok(()), Err)
}

/// [`refuse_row_security`], probed now: at `CREATE TVIEW`, at a re-derive, and
/// after a statement that may make row security apply.
///
/// # Errors
/// [`TViewError::RowSecurity`] naming the table and the role, or a refusal when
/// a role it reads a table as cannot be resolved.
pub fn check_row_security(meta: &super::TviewMeta) -> TViewResult<()> {
    let roles = reading_roles(meta)?;
    let mut read: Vec<u32> = meta.plan.tables.iter().map(|t| t.relid).collect();
    read.extend(
        meta.function_reads
            .iter()
            .flat_map(|(_, relids)| relids.iter().map(|r| r.to_u32())),
    );
    for relid in read {
        for &role in &roles {
            // SAFETY: a syscache lookup by OID; `noError` reports instead of
            // raising, and a relation that is gone has no row security.
            let applies = unsafe {
                pg_sys::check_enable_rls(Oid::from(relid), role, true)
                    == pg_sys::CheckEnableRlsResult::RLS_ENABLED.cast_signed()
            };
            if applies {
                return Err(TViewError::RowSecurity {
                    tview: format!("tv_{}", meta.entity_name),
                    table: crate::utils::qualified_relname_from_oid(Oid::from(relid))?,
                    role: crate::owner::role_name(role),
                });
            }
        }
    }
    Ok(())
}

/// The roles a refresh of `meta`'s TVIEW reads its tables as: its owner, and the
/// owner of every view of its backing view's chain (a view that is not
/// `security_invoker` reads as its owner; each is counted for every table).
///
/// # Errors
/// One of them cannot be resolved: the TVIEW is refused.
fn reading_roles(meta: &super::TviewMeta) -> TViewResult<Vec<Oid>> {
    let unresolved = |e: TViewError| TViewError::DefinitionRefused {
        reason: format!(
            "the roles tv_{} reads its tables as cannot be resolved: {e}",
            meta.entity_name
        ),
    };
    let mut roles = vec![crate::owner::relation_owner(meta.tview_oid).map_err(unresolved)?];
    for &view in &chain(meta.view_oid).map_err(unresolved)?.views {
        let owner = crate::owner::relation_owner(view).map_err(unresolved)?;
        if !roles.contains(&owner) {
            roles.push(owner);
        }
    }
    Ok(roles)
}

/// The transaction ended: forget the row-security probes run in it.
pub fn forget() {
    ROW_SECURITY.with_borrow_mut(HashMap::clear);
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
fn whole_read(view: Oid, relid: u32) -> TViewResult<Option<WholeRead>> {
    chain(view).map(|reads| reads.tables.get(&relid).copied())
}

/// The views of backing view `view`'s chain and what they read beyond the
/// columns they name, memoised until one of them changes.
///
/// # Errors
/// A view of the chain cannot be opened.
fn chain(view: Oid) -> TViewResult<Rc<WholeReads>> {
    WHOLE_READS.with(|memo| {
        memo.get_or_load(view.to_u32(), || {
            let reads = crate::lineage::walk::whole_reads(view)?;
            crate::cache::watch(&reads.views);
            Ok(Rc::new(reads))
        })
    })
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
