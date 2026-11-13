//! The tables a definition reads beyond the columns it names: through a
//! whole-row reference (`to_jsonb(t)`, `f(t)`, `jsonb_agg(t)`, `t::text`, a join
//! alias) or a system column (`xmin`, `ctid`, `tableoid`). `pg_depend` records
//! neither as a column read, so the column list of such a table is not what the
//! definition reads: no write to the table is skipped or patched on it.

use super::{HashSet, Oid, elements, index, pg_guard, pg_sys, tag, view_query};
use crate::error::TViewResult;
use std::collections::BTreeMap;

/// How a definition reads a table beyond the columns it names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WholeRead {
    /// A whole-row reference, or one through a join alias over it.
    pub whole_row: bool,
    /// A system column.
    pub system_column: bool,
}

/// The walk's state: the query levels it is in, innermost last, what it found,
/// and the views to walk next.
struct Walk {
    levels: Vec<*mut pg_sys::Query>,
    found: BTreeMap<u32, WholeRead>,
    views: Vec<Oid>,
}

impl Walk {
    /// Var `var`, of attribute number 0 or below, read at the innermost level.
    ///
    /// SAFETY: `var` is a valid Var of the innermost level's query tree.
    unsafe fn note(&mut self, var: *const pg_sys::Var) {
        // SAFETY: a valid Var, and the range table of the level it names.
        unsafe {
            let up = index((*var).varlevelsup);
            let Some(&query) = self
                .levels
                .len()
                .checked_sub(up + 1)
                .and_then(|i| self.levels.get(i))
            else {
                return;
            };
            let attno = (*var).varattno;
            self.entry(query, index((*var).varno), attno == 0, attno < 0);
        }
    }

    /// Range-table entry `varno` of `query` is read whole (`whole_row`) or
    /// through a system column: a table, or every table under a join.
    ///
    /// SAFETY: `query` is a valid Query.
    unsafe fn entry(&mut self, query: *mut pg_sys::Query, varno: usize, whole: bool, system: bool) {
        crate::utils::guard_depth();
        // SAFETY: the range table of a valid Query, and its entries' fields.
        unsafe {
            let rtable = elements::<pg_sys::RangeTblEntry>((*query).rtable);
            let Some(&rte) = varno.checked_sub(1).and_then(|i| rtable.get(i)) else {
                return;
            };
            match (*rte).rtekind {
                // A view's outputs are read inside it: its own walk finds what
                // it reads whole.
                pg_sys::RTEKind::RTE_RELATION
                    if pg_sys::get_rel_relkind((*rte).relid).cast_unsigned()
                        != pg_sys::RELKIND_VIEW =>
                {
                    let read = self.found.entry((*rte).relid.to_u32()).or_default();
                    read.whole_row |= whole;
                    read.system_column |= system;
                }
                // A join alias read whole reads every table under the join.
                pg_sys::RTEKind::RTE_JOIN if whole => {
                    for &alias in &elements::<pg_sys::Node>((*rte).joinaliasvars) {
                        for varno in alias_entries(alias) {
                            self.entry(query, varno, true, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The range-table entries of its own level that a join alias expression reads
/// (a Var, or COALESCE of two for a merged FULL JOIN column).
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn alias_entries(node: *mut pg_sys::Node) -> HashSet<usize> {
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the set passed below; `node` is valid.
        unsafe {
            let out = &mut *ctx.cast::<HashSet<usize>>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Var) => {
                    let var = node.cast::<pg_sys::Var>();
                    if (*var).varlevelsup == 0 {
                        out.insert(index((*var).varno));
                    }
                    false
                }
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    let mut out = HashSet::new();
    // SAFETY: a read-only walk of a valid expression.
    unsafe {
        walker(node, std::ptr::from_mut(&mut out).cast());
    }
    out
}

#[pg_guard]
unsafe extern "C-unwind" fn walker(node: *mut pg_sys::Node, ctx: *mut std::ffi::c_void) -> bool {
    crate::utils::guard_depth();
    // SAFETY: `ctx` is the Walk passed by `whole_reads`; `node` is valid.
    unsafe {
        let walk = &mut *ctx.cast::<Walk>();
        match tag(node) {
            None => false,
            Some(pg_sys::NodeTag::T_Var) => {
                let var = node.cast::<pg_sys::Var>();
                if (*var).varattno <= 0 {
                    walk.note(var);
                }
                false
            }
            Some(pg_sys::NodeTag::T_Query) => {
                let query = node.cast::<pg_sys::Query>();
                for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                    if (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION
                        && pg_sys::get_rel_relkind((*rte).relid).cast_unsigned()
                            == pg_sys::RELKIND_VIEW
                    {
                        walk.views.push((*rte).relid);
                    }
                }
                walk.levels.push(query);
                // Join aliases and GROUP entries are walked too: a whole-row
                // Var is resolved through them by `entry`, the rest is ignored.
                let done = pg_sys::query_tree_walker(query, Some(walker), ctx, 0);
                walk.levels.pop();
                done
            }
            Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
        }
    }
}

/// What a backing view reads beyond the columns it names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WholeReads {
    /// The tables it reads whole or through a system column, by OID.
    pub tables: BTreeMap<u32, WholeRead>,
    /// The views walked: itself and every view it reads.
    pub views: Vec<Oid>,
}

/// The tables backing view `view_oid` reads whole or through a system column,
/// at any level of its query and of every view it reads.
///
/// # Errors
/// Returns an error if a view it reads cannot be opened.
pub fn whole_reads(view_oid: Oid) -> TViewResult<WholeReads> {
    let mut walk = Walk {
        levels: Vec::new(),
        found: BTreeMap::new(),
        views: vec![view_oid],
    };
    let mut seen: Vec<Oid> = Vec::new();
    while let Some(view) = walk.views.pop() {
        if seen.contains(&view) {
            continue;
        }
        seen.push(view);
        // SAFETY: `view_query` returns a copy in the current memory context; the
        // walk only reads it.
        unsafe {
            let query = view_query(view)?;
            walker(query.cast(), std::ptr::from_mut(&mut walk).cast());
        }
    }
    Ok(WholeReads {
        tables: walk.found,
        views: seen,
    })
}
