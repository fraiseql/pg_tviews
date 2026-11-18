//! What a definition reads of each table, from every Var of every query level
//! of its view and of every view it reads: the columns it names (through join
//! aliases and group entries), and whether it reads the table beyond them,
//! through a whole-row reference (`to_jsonb(t)`, `f(t)`, `jsonb_agg(t)`,
//! `t::text`, a join alias) or a system column (`xmin`, `ctid`; `tableoid` of a
//! partitioned table, which changes with the partition key). A table read
//! beyond its columns has no column read set: no write to it is skipped or
//! patched (D1). `pg_depend` records the named columns too, and is checked
//! against this walk when a TVIEW is registered.

use super::{HashSet, Oid, elements, index, pg_guard, pg_sys, tag, view_query};
use crate::error::TViewResult;
use std::collections::{BTreeMap, BTreeSet};

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
    columns: BTreeMap<u32, BTreeSet<i16>>,
    views: Vec<Oid>,
    relations: BTreeSet<u32>,
}

impl Walk {
    /// Var `var` read at the innermost level.
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
                // walk: n/a(a Var of a level the walk is in)
                return;
            };
            self.entry(query, index((*var).varno), (*var).varattno);
        }
    }

    /// Attribute `attno` of range-table entry `varno` of `query` is read (0: the
    /// whole row): a table's column, or what a join alias or a group entry
    /// stands for, at that level.
    ///
    /// SAFETY: `query` is a valid Query.
    unsafe fn entry(&mut self, query: *mut pg_sys::Query, varno: usize, attno: i16) {
        crate::utils::guard_depth();
        // SAFETY: the range table of a valid Query, and its entries' fields.
        unsafe {
            let rtable = elements::<pg_sys::RangeTblEntry>((*query).rtable);
            let Some(&rte) = varno.checked_sub(1).and_then(|i| rtable.get(i)) else {
                // walk: n/a(a Var names an entry of its own level's range table)
                return;
            };
            match (*rte).rtekind {
                // A view's outputs are read inside it: its own walk finds what
                // it reads.
                pg_sys::RTEKind::RTE_RELATION => {
                    let relkind = pg_sys::get_rel_relkind((*rte).relid).cast_unsigned();
                    if relkind == pg_sys::RELKIND_VIEW {
                        return;
                    }
                    let relid = (*rte).relid.to_u32();
                    if attno > 0 {
                        self.columns.entry(relid).or_default().insert(attno);
                        return;
                    }
                    // `tableoid` is constant but in a partitioned table, where
                    // it is the partition the key chooses.
                    let constant_oid = i32::from(attno) == pg_sys::TableOidAttributeNumber
                        && relkind != pg_sys::RELKIND_PARTITIONED_TABLE;
                    if constant_oid {
                        self.columns.entry(relid).or_default();
                        return;
                    }
                    let read = self.found.entry(relid).or_default();
                    read.whole_row |= attno == 0;
                    read.system_column |= attno < 0;
                }
                // A join alias or a group entry stands for expressions over
                // this level's entries: each Var of them is read (all of them,
                // for the whole row).
                pg_sys::RTEKind::RTE_JOIN | pg_sys::RTEKind::RTE_GROUP => {
                    let list = if (*rte).rtekind == pg_sys::RTEKind::RTE_JOIN {
                        (*rte).joinaliasvars
                    } else {
                        (*rte).groupexprs
                    };
                    let exprs = elements::<pg_sys::Node>(list);
                    let chosen: Vec<*mut pg_sys::Node> = if attno == 0 {
                        exprs
                    } else {
                        usize::try_from(attno)
                            .ok()
                            .and_then(|a| a.checked_sub(1))
                            .and_then(|i| exprs.get(i).copied())
                            .into_iter()
                            .collect()
                    };
                    for expr in chosen {
                        for (under, column) in alias_reads(expr) {
                            // Read whole, it reads whole every entry under it.
                            self.entry(query, under, if attno == 0 { 0 } else { column });
                        }
                    }
                }
                // walk: leaf(walked-inside) a subquery, CTE or function entry: the walk visits
                // every Var of its own level, read above or not
                _ => {}
            }
        }
    }
}

/// The `(varno, attno)` of its own level that a join alias or group expression
/// reads (a Var, or COALESCE of two for a merged FULL JOIN column).
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn alias_reads(node: *mut pg_sys::Node) -> HashSet<(usize, i16)> {
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the set passed below; `node` is valid.
        unsafe {
            let out = &mut *ctx.cast::<HashSet<(usize, i16)>>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Var) => {
                    let var = node.cast::<pg_sys::Var>();
                    if (*var).varlevelsup == 0 {
                        out.insert((index((*var).varno), (*var).varattno));
                    }
                    false
                }
                // walk: leaf(descend) nothing of its own: its children are walked
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
                walk.note(node.cast::<pg_sys::Var>());
                false
            }
            Some(pg_sys::NodeTag::T_Query) => {
                let query = node.cast::<pg_sys::Query>();
                for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                    if (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION {
                        if pg_sys::get_rel_relkind((*rte).relid).cast_unsigned()
                            == pg_sys::RELKIND_VIEW
                        {
                            walk.views.push((*rte).relid);
                        } else {
                            walk.relations.insert((*rte).relid.to_u32());
                        }
                    }
                }
                walk.levels.push(query);
                // Join aliases and GROUP entries hold every column under them,
                // read or not: a Var naming one is resolved through them by
                // `entry` instead.
                let flags = pg_sys::QTW_IGNORE_JOINALIASES | pg_sys::QTW_IGNORE_GROUPEXPRS;
                let done = pg_sys::query_tree_walker(query, Some(walker), ctx, flags.cast_signed());
                walk.levels.pop();
                done
            }
            // walk: leaf(descend) nothing of its own: its children are walked
            Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
        }
    }
}

/// What a backing view reads of its tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WholeReads {
    /// The tables it reads whole or through a system column, by OID.
    pub tables: BTreeMap<u32, WholeRead>,
    /// The columns it names of each table it reads, by OID: a table it reads
    /// only whole, or only by its `tableoid`, has an empty set.
    pub columns: BTreeMap<u32, BTreeSet<i16>>,
    /// The views walked: itself and every view it reads.
    pub views: Vec<Oid>,
    /// Every other relation it or a view it reads names, by OID.
    pub relations: BTreeSet<u32>,
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
        columns: BTreeMap::new(),
        views: vec![view_oid],
        relations: BTreeSet::new(),
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
        columns: walk.columns,
        views: seen,
        relations: walk.relations,
    })
}
