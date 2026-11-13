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

/// Whether backing view `view`'s chain reads table `relid` beyond the columns
/// its views name: whole or through a system column. True when the chain
/// cannot be walked.
#[must_use]
pub fn read_beyond_columns(view: Oid, relid: Oid) -> bool {
    !matches!(whole_read(view, relid.to_u32()), Ok(None))
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

/// [`refuse_row_security`], probed now: at `CREATE TVIEW`, at a re-derive, at a
/// refresh, and after a statement that may make row security apply.
///
/// # Errors
/// [`TViewError::RowSecurity`] naming the table and the role, or a refusal when
/// a role it reads a table as cannot be resolved.
pub fn check_row_security(meta: &super::TviewMeta) -> TViewResult<()> {
    let tables: Vec<u32> = meta.plan.tables.iter().map(|t| t.relid).collect();
    check_reads(
        &meta.entity_name,
        meta.tview_oid,
        meta.view_oid,
        &tables,
        &meta.function_reads,
    )
}

/// The probe of [`check_row_security`], over a TVIEW's parts: before its
/// registration (`CREATE TVIEW`, before the fill) and after it.
///
/// The tables are `tables` (the plan's), every relation the backing view's chain
/// names as its views read now, and those `function_reads` declares; the roles
/// are [`reading_roles`]'. Every role is probed on every table.
///
/// # Errors
/// As [`check_row_security`].
pub fn check_reads(
    entity: &str,
    tview: Oid,
    view: Oid,
    tables: &[u32],
    function_reads: &[(String, Vec<Oid>)],
) -> TViewResult<()> {
    let unresolved = |e: TViewError| TViewError::DefinitionRefused {
        reason: format!("the roles tv_{entity} reads its tables as cannot be resolved: {e}"),
    };
    let reads = chain(view).map_err(unresolved)?;
    let roles = reading_roles(tview, &reads.views, function_reads).map_err(unresolved)?;
    let mut read: Vec<u32> = tables.to_vec();
    read.extend(reads.relations.iter().copied());
    read.extend(
        function_reads
            .iter()
            .flat_map(|(_, relids)| relids.iter().map(|r| r.to_u32())),
    );
    read.sort_unstable();
    read.dedup();
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
                    tview: format!("tv_{entity}"),
                    table: crate::utils::qualified_relname_from_oid(Oid::from(relid))?,
                    role: crate::owner::role_name(role),
                });
            }
        }
    }
    Ok(())
}

/// The roles a refresh of TVIEW `tview` reads its tables as: its owner, the
/// owner of every view of its backing view's chain `views` (a view that is not
/// `security_invoker` reads as its owner), and the owner of every declared
/// function that is `SECURITY DEFINER`. Each is counted for every table.
///
/// # Errors
/// One of them cannot be resolved (a declared function among them).
fn reading_roles(
    tview: Oid,
    views: &[Oid],
    function_reads: &[(String, Vec<Oid>)],
) -> TViewResult<Vec<Oid>> {
    let mut roles = vec![crate::owner::relation_owner(tview)?];
    for &view in views {
        let owner = crate::owner::relation_owner(view)?;
        if !roles.contains(&owner) {
            roles.push(owner);
        }
    }
    for (function, _) in function_reads {
        if let Some(owner) = definer(function)?
            && !roles.contains(&owner)
        {
            roles.push(owner);
        }
    }
    Ok(roles)
}

/// The owner of declared function `function` (`schema.name(argument types)`)
/// when it is `SECURITY DEFINER`.
///
/// # Errors
/// No function has that signature.
fn definer(function: &str) -> TViewResult<Option<Oid>> {
    // `InvalidOid` for a function that is not `SECURITY DEFINER`; no row for
    // none. Matched through `pg_proc` by name, then by the signature as stored.
    let owner = crate::utils::spi::one::<Oid>(
        &format!(
            "SELECT CASE WHEN p.prosecdef THEN p.proowner ELSE 0::pg_catalog.oid END \
             FROM pg_catalog.pg_proc p WHERE p.proname = $2 AND {} = $1 LIMIT 1",
            crate::lineage::FUNCTION_SIGNATURE
        ),
        &[
            crate::utils::spi::text(function),
            crate::utils::spi::text(function_name(function).as_str()),
        ],
    )?;
    match owner {
        Some(owner) => Ok((owner != pg_sys::InvalidOid).then_some(owner)),
        None => Err(TViewError::DefinitionRefused {
            reason: format!("function {function} does not exist"),
        }),
    }
}

/// The name of function `signature` (`schema.name(argument types)`, each part
/// as `quote_ident` writes it), unquoted.
fn function_name(signature: &str) -> String {
    let mut part = String::new();
    let mut quoted = false;
    let mut chars = signature.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                part.push('"');
            }
            '"' => quoted = !quoted,
            '.' if !quoted => part.clear(),
            '(' if !quoted => break,
            c => part.push(c),
        }
    }
    part
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

#[cfg(test)]
mod tests {
    use super::function_name;

    #[test]
    fn a_function_name_is_read_from_its_signature() {
        assert_eq!(function_name("public.extra_label(integer)"), "extra_label");
        assert_eq!(
            function_name(r#""my.schema"."f(""x"")"(text, integer)"#),
            r#"f("x")"#
        );
        assert_eq!(function_name("s.g()"), "g");
    }
}
