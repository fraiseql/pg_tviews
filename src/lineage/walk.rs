//! Read a backing view's analyzed query into a [`Graph`] (ADR 0157).
//!
//! The only module that walks `pg_sys` nodes. The view's query comes from the
//! relcache (`get_view_query`) and is copied before it is walked; nothing here
//! modifies a node. Views are expanded by OID, CTEs and subqueries in place, to a
//! depth of [`MAX_DEPTH`] query levels.

#![allow(clippy::cast_ptr_alignment)] // Reason: `Node *` is cast to the node type its tag names, as PostgreSQL does; palloc aligns every node for its own type.

use super::{Column, Conjunct, Graph, Occurrence, Root, Sql};
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;

/// Query levels (views, subqueries, CTEs) followed before giving up.
pub const MAX_DEPTH: usize = 32;

/// What the walk needs to know about the registered TVIEWs.
pub struct Context<'a> {
    /// Tables of TVIEWs (`tv_*`): not base tables, as in `pg_tview_reads`.
    pub tview_tables: &'a HashSet<Oid>,
    /// Backing views of other TVIEWs → their entity.
    pub tview_views: &'a HashMap<Oid, String>,
    /// The TVIEW's key column, `pk_<entity>`.
    pub key_column: &'a str,
}

/// Read the backing view `view_oid` into a [`Graph`].
///
/// # Errors
/// Returns an error if the view cannot be opened, nests deeper than
/// [`MAX_DEPTH`] levels, or reads a view the current user may not read.
pub fn analyze(view_oid: Oid, ctx: &Context<'_>) -> TViewResult<Graph> {
    let mut walker = Walker {
        ctx,
        graph: Graph::default(),
        levels: Vec::new(),
        catalog: CatalogNames::default(),
    };
    // SAFETY: `view_query` returns a copy owned by the current memory context; the
    // walk only reads it.
    let query = unsafe { view_query(view_oid)? };
    let flags = Flags::default();
    // SAFETY: `query` is a valid, copied Query.
    unsafe { walker.top(query, &flags)? };
    Ok(walker.graph)
}

/// The analyzed query of view `view_oid`, copied out of the relcache.
///
/// SAFETY: must run inside a transaction; the copy lives in the current memory context.
unsafe fn view_query(view_oid: Oid) -> TViewResult<*mut pg_sys::Query> {
    // SAFETY: the relation is opened and closed here; the lock is kept until the
    // transaction ends, as for any relation a query reads.
    unsafe {
        let rel = pg_sys::try_relation_open(view_oid, pg_sys::AccessShareLock.cast_signed());
        if rel.is_null() {
            return Err(TViewError::CatalogError {
                operation: format!("Open view {view_oid:?}"),
                pg_error: "relation does not exist".to_string(),
            });
        }
        let query = pg_sys::get_view_query(rel);
        let copy = pg_sys::copyObjectImpl(query.cast()).cast::<pg_sys::Query>();
        pg_sys::relation_close(rel, pg_sys::NoLock.cast_signed());
        Ok(copy)
    }
}

/// How a query level sits inside the occurrence's path from the top.
#[derive(Debug, Clone, Default)]
struct Flags {
    branch: usize,
    via_view: Option<String>,
    via_tview: Option<String>,
    in_sublink: bool,
    opaque_level: Option<String>,
}

/// A column a Var stands for once views and subqueries are seen through.
#[derive(Debug, Clone)]
enum Resolved {
    Col(Column),
    /// One column per UNION branch of a subquery.
    Alt(Vec<Column>),
    Opaque,
}

#[derive(Debug, Clone)]
enum RteInfo {
    Base(usize),
    Outputs(Vec<Resolved>),
    Join(*mut pg_sys::List),
    Other,
}

/// How a query level is nested in the level above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Top,
    /// A subquery in FROM, a view, a CTE.
    From,
    /// A subquery expression; `required` when the row above exists only if it
    /// returns a matching row (a positive `EXISTS` / `IN` in a top-level `WHERE` conjunct).
    Sublink {
        required: bool,
    },
}

struct Level {
    query: *mut pg_sys::Query,
    rtes: Vec<RteInfo>,
    link: Link,
}

/// Where a predicate comes from, which says in which directions it must hold.
#[derive(Clone, Copy)]
enum Origin<'a> {
    /// `WHERE`, or the condition of an inner join.
    Required,
    /// The condition of an outer join: holds for rows of the nullable side.
    Outer { nullable: &'a HashSet<usize> },
    /// Nothing can be assumed (a FULL join).
    None,
}

/// A Var of a predicate: the level it belongs to and the column it stands for.
struct Site {
    levelsup: usize,
    candidates: Vec<Column>,
}

#[derive(Default)]
struct CatalogNames {
    operators: HashMap<u32, String>,
    functions: HashMap<u32, (String, bool)>,
    types: HashMap<u32, String>,
}

struct Walker<'c> {
    ctx: &'c Context<'c>,
    graph: Graph,
    levels: Vec<Level>,
    catalog: CatalogNames,
}

fn cstr(ptr: *const std::ffi::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: a non-null, NUL-terminated C string from PostgreSQL.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// The elements of a `List *` of pointers.
///
/// SAFETY: `list` is null or a valid pointer list.
unsafe fn elements<T>(list: *mut pg_sys::List) -> Vec<*mut T> {
    if list.is_null() {
        return Vec::new();
    }
    // SAFETY: indexes stay below the list's length.
    unsafe {
        (0..(*list).length)
            .map(|i| pg_sys::list_nth(list, i).cast::<T>())
            .collect()
    }
}

/// SAFETY: `node` is null or a valid node.
unsafe fn tag(node: *const pg_sys::Node) -> Option<pg_sys::NodeTag> {
    // SAFETY: checked for null.
    unsafe { (!node.is_null()).then(|| (*node).type_) }
}

/// The AND-ed conjuncts of a qual.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn conjuncts(node: *mut pg_sys::Node) -> Vec<*mut pg_sys::Node> {
    // SAFETY: each pointer is checked by `tag` before it is cast.
    unsafe {
        match tag(node) {
            None => Vec::new(),
            Some(pg_sys::NodeTag::T_List) => elements::<pg_sys::Node>(node.cast())
                .into_iter()
                .flat_map(|n| conjuncts(n))
                .collect(),
            Some(pg_sys::NodeTag::T_BoolExpr)
                if (*node.cast::<pg_sys::BoolExpr>()).boolop == pg_sys::BoolExprType::AND_EXPR =>
            {
                elements::<pg_sys::Node>((*node.cast::<pg_sys::BoolExpr>()).args)
                    .into_iter()
                    .flat_map(|n| conjuncts(n))
                    .collect()
            }
            Some(_) => vec![node],
        }
    }
}

impl Walker<'_> {
    // ── query levels ────────────────────────────────────────────────────────

    /// The top level: the backing view itself, or each branch of its UNION.
    ///
    /// SAFETY: `query` is a valid Query.
    unsafe fn top(&mut self, query: *mut pg_sys::Query, flags: &Flags) -> TViewResult<()> {
        // SAFETY: fields of a valid Query.
        unsafe {
            let key_position = elements::<pg_sys::TargetEntry>((*query).targetList)
                .iter()
                .position(|tle| cstr((**tle).resname) == self.ctx.key_column);
            if (*query).setOperations.is_null() {
                // A window function, LIMIT/OFFSET, a set-returning function or
                // GROUPING SETS here change rows other than the written one: no
                // row maps to its own key, so nothing gets a key root.
                let opaque = top_opaque_reason(query);
                let flags = Flags {
                    opaque_level: opaque.clone().or_else(|| flags.opaque_level.clone()),
                    ..flags.clone()
                };
                let outputs = self.level(query, &flags, Link::Top)?;
                if opaque.is_none()
                    && let Some(Resolved::Col(key)) = key_position.and_then(|p| outputs.get(p))
                {
                    self.graph.roots.push(Root {
                        branch: flags.branch,
                        key: key.clone(),
                    });
                }
                return Ok(());
            }
            // LIMIT/OFFSET over the whole set operation applies to every branch.
            let whole = top_opaque_reason(query);
            // UNION: each leaf is a branch with its own root. The leaves sit in
            // the rtable as subqueries, referenced from the set-operation tree.
            self.levels.push(Level {
                query,
                rtes: vec![RteInfo::Other; list_len((*query).rtable)],
                link: Link::Top,
            });
            let mut leaves = Vec::new();
            setop_leaves((*query).setOperations, &mut leaves);
            let result: TViewResult<()> = (|| {
                for (branch, rtindex) in leaves.into_iter().enumerate() {
                    let Some(&rte) = elements::<pg_sys::RangeTblEntry>((*query).rtable)
                        .get(rtindex.wrapping_sub(1))
                    else {
                        continue;
                    };
                    let opaque = whole.clone().or_else(|| top_opaque_reason((*rte).subquery));
                    let leaf_flags = Flags {
                        branch,
                        opaque_level: opaque.clone().or_else(|| flags.opaque_level.clone()),
                        ..flags.clone()
                    };
                    let outputs = self.level((*rte).subquery, &leaf_flags, Link::Top)?;
                    if opaque.is_none()
                        && let Some(Resolved::Col(key)) = key_position.and_then(|p| outputs.get(p))
                    {
                        self.graph.roots.push(Root {
                            branch,
                            key: key.clone(),
                        });
                    }
                }
                Ok(())
            })();
            self.levels.pop();
            result
        }
    }

    /// Walk one query level and return what each of its output columns stands for.
    ///
    /// SAFETY: `query` is a valid Query.
    unsafe fn level(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
        link: Link,
    ) -> TViewResult<Vec<Resolved>> {
        if self.levels.len() >= MAX_DEPTH {
            return Err(TViewError::InvalidInput {
                parameter: "tview definition".to_string(),
                reason: format!(
                    "the backing view nests views, subqueries and CTEs more than {MAX_DEPTH} levels deep"
                ),
            });
        }
        // SAFETY: fields of a valid Query.
        unsafe {
            if !(*query).setOperations.is_null() {
                return self.union_outputs(query, flags);
            }
            let opaque = (link != Link::Top).then(|| opaque_reason(query)).flatten();
            let flags = Flags {
                opaque_level: opaque.clone().or_else(|| flags.opaque_level.clone()),
                ..flags.clone()
            };
            self.levels.push(Level {
                query,
                rtes: Vec::new(),
                link,
            });
            let result = self.level_body(query, &flags, link, opaque.is_some());
            self.levels.pop();
            result
        }
    }

    /// SAFETY: `query` is the valid Query of the innermost level.
    unsafe fn level_body(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
        link: Link,
        opaque: bool,
    ) -> TViewResult<Vec<Resolved>> {
        // SAFETY: fields of a valid Query; RTEs and expressions belong to it.
        unsafe {
            for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                let info = self.rte(rte, flags)?;
                self.current().rtes.push(info);
            }
            let jointree = (*query).jointree;
            if !jointree.is_null() {
                self.join_item(jointree.cast(), flags)?;
            }
            // Subquery expressions anywhere else in this level.
            for tle in elements::<pg_sys::TargetEntry>((*query).targetList) {
                self.sublinks((*tle).expr.cast(), flags, false)?;
            }
            self.sublinks((*query).havingQual, flags, false)?;
            self.note_functions(query.cast());

            let grouped = (*query).hasAggs || !(*query).groupClause.is_null();
            let mut outputs = Vec::new();
            for tle in elements::<pg_sys::TargetEntry>((*query).targetList) {
                let pass_through = link == Link::Top
                    || (!opaque
                        && (!grouped || in_clause((*tle).ressortgroupref, (*query).groupClause))
                        && (!(*query).hasDistinctOn
                            || in_clause((*tle).ressortgroupref, (*query).distinctClause)));
                outputs.push(if pass_through {
                    self.resolve_expr((*tle).expr.cast())
                } else {
                    Resolved::Opaque
                });
            }
            Ok(outputs)
        }
    }

    /// The outputs of a UNION subquery: each column stands for the matching column
    /// of every branch.
    ///
    /// SAFETY: `query` is a valid set-operation Query.
    unsafe fn union_outputs(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
    ) -> TViewResult<Vec<Resolved>> {
        // SAFETY: fields of a valid Query.
        unsafe {
            self.levels.push(Level {
                query,
                rtes: vec![RteInfo::Other; list_len((*query).rtable)],
                link: Link::From,
            });
            let mut leaves = Vec::new();
            setop_leaves((*query).setOperations, &mut leaves);
            let width = list_len((*query).targetList);
            let mut columns: Vec<Vec<Column>> = vec![Vec::new(); width];
            let result: TViewResult<()> = (|| {
                for rtindex in leaves {
                    let Some(&rte) = elements::<pg_sys::RangeTblEntry>((*query).rtable)
                        .get(rtindex.wrapping_sub(1))
                    else {
                        continue;
                    };
                    let outputs = self.level((*rte).subquery, flags, Link::From)?;
                    for (i, out) in outputs.into_iter().take(width).enumerate() {
                        match out {
                            Resolved::Col(c) => columns[i].push(c),
                            Resolved::Alt(cs) => columns[i].extend(cs),
                            Resolved::Opaque => {}
                        }
                    }
                }
                Ok(())
            })();
            self.levels.pop();
            result?;
            Ok(columns
                .into_iter()
                .map(|cs| {
                    if cs.is_empty() {
                        Resolved::Opaque
                    } else {
                        Resolved::Alt(cs)
                    }
                })
                .collect())
        }
    }

    fn current(&mut self) -> &mut Level {
        self.levels.last_mut().expect("inside a query level")
    }

    // ── range table ─────────────────────────────────────────────────────────

    /// SAFETY: `rte` is a valid RTE of the innermost level.
    unsafe fn rte(
        &mut self,
        rte: *mut pg_sys::RangeTblEntry,
        flags: &Flags,
    ) -> TViewResult<RteInfo> {
        // SAFETY: fields of a valid RTE.
        unsafe {
            match (*rte).rtekind {
                pg_sys::RTEKind::RTE_RELATION => self.relation((*rte).relid, flags),
                pg_sys::RTEKind::RTE_SUBQUERY => Ok(RteInfo::Outputs(self.level(
                    (*rte).subquery,
                    flags,
                    Link::From,
                )?)),
                pg_sys::RTEKind::RTE_CTE => {
                    let Some(cte) = self.cte(&cstr((*rte).ctename), (*rte).ctelevelsup as usize)
                    else {
                        return Ok(RteInfo::Other);
                    };
                    let copy = pg_sys::copyObjectImpl(cte.cast()).cast::<pg_sys::Query>();
                    Ok(RteInfo::Outputs(self.level(copy, flags, Link::From)?))
                }
                pg_sys::RTEKind::RTE_JOIN => Ok(RteInfo::Join((*rte).joinaliasvars)),
                // PostgreSQL 18: grouped Vars point at the GROUP entry, whose
                // expressions are those of the level.
                #[cfg(feature = "pg18")]
                pg_sys::RTEKind::RTE_GROUP => Ok(RteInfo::Join((*rte).groupexprs)),
                _ => Ok(RteInfo::Other),
            }
        }
    }

    /// A relation in FROM: a base table occurrence, or a view to expand.
    fn relation(&mut self, relid: Oid, flags: &Flags) -> TViewResult<RteInfo> {
        // SAFETY: catalog lookups by OID.
        let (relkind, relname, qualified) = unsafe {
            let relkind = pg_sys::get_rel_relkind(relid) as u8;
            let relname = cstr(pg_sys::get_rel_name(relid));
            let nsp = cstr(pg_sys::get_namespace_name(pg_sys::get_rel_namespace(relid)));
            let qualified = format!("{}.{}", quote_ident(&nsp), quote_ident(&relname));
            (relkind, relname, qualified)
        };
        match relkind {
            b'r' | b'p' if !self.ctx.tview_tables.contains(&relid) => {
                self.graph.occurrences.push(Occurrence {
                    relid: relid.to_u32(),
                    relname,
                    qualified,
                    branch: flags.branch,
                    via_view: flags.via_view.clone(),
                    via_tview: flags.via_tview.clone(),
                    in_sublink: flags.in_sublink,
                    opaque_level: flags.opaque_level.clone(),
                });
                Ok(RteInfo::Base(self.graph.occurrences.len() - 1))
            }
            b'v' => {
                // SAFETY: an ACL check by OID for the current user.
                let readable = unsafe {
                    pg_sys::pg_class_aclcheck(
                        relid,
                        pg_sys::GetUserId(),
                        pg_sys::AclMode::from(pg_sys::ACL_SELECT),
                    ) == pg_sys::AclResult::ACLCHECK_OK
                };
                if !readable {
                    return Err(TViewError::InvalidInput {
                        parameter: "tview definition".to_string(),
                        reason: format!("permission denied to read view {qualified}"),
                    });
                }
                let inner = match self.ctx.tview_views.get(&relid) {
                    Some(entity) => Flags {
                        via_tview: flags.via_tview.clone().or_else(|| Some(entity.clone())),
                        ..flags.clone()
                    },
                    None => Flags {
                        via_view: flags.via_view.clone().or(Some(qualified)),
                        ..flags.clone()
                    },
                };
                // SAFETY: the view query is a fresh copy.
                let outputs = unsafe { self.level(view_query(relid)?, &inner, Link::From)? };
                Ok(RteInfo::Outputs(outputs))
            }
            _ => Ok(RteInfo::Other),
        }
    }

    /// The query of CTE `name`, defined `levelsup` levels above the innermost one.
    fn cte(&self, name: &str, levelsup: usize) -> Option<*mut pg_sys::Query> {
        let level = self.levels.len().checked_sub(1 + levelsup)?;
        let query = self.levels[level].query;
        // SAFETY: the CTE list of a valid Query.
        unsafe {
            elements::<pg_sys::CommonTableExpr>((*query).cteList)
                .into_iter()
                .find(|cte| cstr((**cte).ctename) == *name)
                .map(|cte| (*cte).ctequery.cast())
        }
    }

    // ── join tree ───────────────────────────────────────────────────────────

    /// Walk a FROM item, adding its conditions; return the occurrences under it.
    ///
    /// SAFETY: `node` is a valid jointree node of the innermost level.
    unsafe fn join_item(
        &mut self,
        node: *mut pg_sys::Node,
        flags: &Flags,
    ) -> TViewResult<HashSet<usize>> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(node) {
                Some(pg_sys::NodeTag::T_RangeTblRef) => {
                    let rtindex = (*node.cast::<pg_sys::RangeTblRef>()).rtindex as usize;
                    Ok(self.occurrences_of(rtindex))
                }
                Some(pg_sys::NodeTag::T_FromExpr) => {
                    let from = node.cast::<pg_sys::FromExpr>();
                    let mut under = HashSet::new();
                    for item in elements::<pg_sys::Node>((*from).fromlist) {
                        under.extend(self.join_item(item, flags)?);
                    }
                    for qual in conjuncts((*from).quals) {
                        self.predicate(qual, Origin::Required);
                    }
                    // A positive EXISTS / IN conjunct of WHERE must hold for every row.
                    for qual in conjuncts((*from).quals) {
                        let required = is_required_sublink(qual);
                        self.sublinks(qual, flags, required)?;
                    }
                    Ok(under)
                }
                Some(pg_sys::NodeTag::T_JoinExpr) => {
                    let join = node.cast::<pg_sys::JoinExpr>();
                    let left = self.join_item((*join).larg, flags)?;
                    let right = self.join_item((*join).rarg, flags)?;
                    for qual in conjuncts((*join).quals) {
                        let origin = match (*join).jointype {
                            pg_sys::JoinType::JOIN_INNER => Origin::Required,
                            pg_sys::JoinType::JOIN_LEFT => Origin::Outer { nullable: &right },
                            pg_sys::JoinType::JOIN_RIGHT => Origin::Outer { nullable: &left },
                            _ => Origin::None,
                        };
                        self.predicate(qual, origin);
                    }
                    self.sublinks((*join).quals, flags, false)?;
                    Ok(left.union(&right).copied().collect())
                }
                _ => Ok(HashSet::new()),
            }
        }
    }

    /// The occurrences a range table entry of the innermost level stands for.
    fn occurrences_of(&self, rtindex: usize) -> HashSet<usize> {
        let level = self.levels.last().expect("inside a query level");
        match level.rtes.get(rtindex.wrapping_sub(1)) {
            Some(RteInfo::Base(occ)) => HashSet::from([*occ]),
            Some(RteInfo::Outputs(outputs)) => outputs
                .iter()
                .flat_map(|r| match r {
                    Resolved::Col(c) => vec![c.occ],
                    Resolved::Alt(cs) => cs.iter().map(|c| c.occ).collect(),
                    Resolved::Opaque => vec![],
                })
                .collect(),
            _ => HashSet::new(),
        }
    }

    // ── subquery expressions ────────────────────────────────────────────────

    /// Walk the subquery expressions in `node` (not those nested in them: their
    /// own level does).
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    unsafe fn sublinks(
        &mut self,
        node: *mut pg_sys::Node,
        flags: &Flags,
        required: bool,
    ) -> TViewResult<()> {
        let mut found: Vec<*mut pg_sys::SubLink> = Vec::new();
        // SAFETY: a read-only walk of a valid expression.
        unsafe {
            collect_sublinks(node, &mut found);
            for sublink in found {
                self.sublink(sublink, flags, required)?;
            }
        }
        Ok(())
    }

    /// SAFETY: `sublink` is a valid `SubLink` of the innermost level.
    unsafe fn sublink(
        &mut self,
        sublink: *mut pg_sys::SubLink,
        flags: &Flags,
        required: bool,
    ) -> TViewResult<()> {
        // SAFETY: fields of a valid SubLink.
        unsafe {
            let subselect = (*sublink).subselect.cast::<pg_sys::Query>();
            if subselect.is_null() {
                return Ok(());
            }
            let required = required
                && matches!(
                    (*sublink).subLinkType,
                    pg_sys::SubLinkType::EXISTS_SUBLINK | pg_sys::SubLinkType::ANY_SUBLINK
                );
            let inner = Flags {
                in_sublink: true,
                ..flags.clone()
            };
            let outputs = self.level(subselect, &inner, Link::Sublink { required })?;
            // `x IN (SELECT y …)`: x = y holds for the matching row.
            if (*sublink).subLinkType == pg_sys::SubLinkType::ANY_SUBLINK {
                for test in conjuncts((*sublink).testexpr) {
                    self.test_predicate(test, &outputs, required);
                }
            }
            Ok(())
        }
    }

    // ── predicates ──────────────────────────────────────────────────────────

    /// Record a predicate of the innermost level if it links two occurrences and
    /// can be used: strict, immutable, written with supported nodes.
    ///
    /// SAFETY: `qual` is a valid expression of the innermost level.
    unsafe fn predicate(&mut self, qual: *mut pg_sys::Node, origin: Origin<'_>) {
        // SAFETY: read-only checks of a valid expression.
        unsafe {
            if matches!(origin, Origin::None)
                || pg_sys::contain_nonstrict_functions(qual)
                || pg_sys::contain_mutable_functions(qual)
                || has_sublink(qual)
            {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(qual, &mut sites) {
                return;
            }
            self.add_conjuncts(qual, &sites, &|_| None, origin, true);
        }
    }

    /// `lhs op Param` of an `IN (SELECT …)`: the Param stands for the subquery's
    /// output column.
    ///
    /// SAFETY: `test` is a valid expression of the innermost level.
    unsafe fn test_predicate(
        &mut self,
        test: *mut pg_sys::Node,
        outputs: &[Resolved],
        required: bool,
    ) {
        // SAFETY: read-only checks of a valid expression.
        unsafe {
            if pg_sys::contain_nonstrict_functions(test) || pg_sys::contain_mutable_functions(test)
            {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(test, &mut sites) {
                return;
            }
            // The subquery's columns sit one level below the innermost one.
            let param_column = |param: *mut pg_sys::Param| -> Option<Vec<Column>> {
                let p = &*param;
                if p.paramkind != pg_sys::ParamKind::PARAM_SUBLINK {
                    return None;
                }
                match outputs.get(usize::try_from(p.paramid).ok()?.checked_sub(1)?)? {
                    Resolved::Col(c) => Some(vec![c.clone()]),
                    Resolved::Alt(cs) => Some(cs.clone()),
                    Resolved::Opaque => None,
                }
            };
            let mut param_sites = Vec::new();
            collect_params(test, &mut param_sites);
            for param in &param_sites {
                let Some(candidates) = param_column(*param) else {
                    return;
                };
                sites.push(Site {
                    // Below the innermost level: always "inner".
                    levelsup: usize::MAX,
                    candidates,
                });
            }
            let lookup = |node: *mut pg_sys::Node| -> Option<usize> {
                param_sites
                    .iter()
                    .position(|p| p.cast::<pg_sys::Node>() == node)
            };
            self.add_conjuncts(test, &sites, &lookup, Origin::Required, required);
        }
    }

    /// Resolve every Var of `expr`; false if one is opaque or `expr` holds a node
    /// the deparser does not write.
    ///
    /// SAFETY: `expr` is a valid expression of the innermost level.
    unsafe fn sites(&self, expr: *mut pg_sys::Node, sites: &mut Vec<Site>) -> bool {
        let mut vars = Vec::new();
        // SAFETY: a read-only walk of a valid expression.
        unsafe {
            collect_vars(expr, &mut vars);
            for var in vars {
                let v = &*var;
                let levelsup = v.varlevelsup as usize;
                let candidates = match self.resolve_var(var, levelsup) {
                    Resolved::Col(c) => vec![c],
                    Resolved::Alt(cs) => cs,
                    Resolved::Opaque => return false,
                };
                sites.push(Site {
                    levelsup,
                    candidates,
                });
            }
        }
        true
    }

    /// Add one conjunct per choice of column for each site, when it links exactly
    /// two occurrences. `param_site(node)` maps a Param node to its site index past
    /// the Vars.
    ///
    /// SAFETY: `expr` is a valid expression of the innermost level and `sites` are
    /// its Vars (in walk order), then its Params.
    unsafe fn add_conjuncts(
        &mut self,
        expr: *mut pg_sys::Node,
        sites: &[Site],
        param_site: &dyn Fn(*mut pg_sys::Node) -> Option<usize>,
        origin: Origin<'_>,
        outer_to_inner: bool,
    ) {
        if sites.is_empty() {
            return;
        }
        let combinations: usize = sites.iter().map(|s| s.candidates.len()).product();
        if combinations == 0 || combinations > 16 {
            return;
        }
        let var_count = sites.iter().filter(|s| s.levelsup != usize::MAX).count();
        for n in 0..combinations {
            let mut rest = n;
            let chosen: Vec<&Column> = sites
                .iter()
                .map(|s| {
                    let c = &s.candidates[rest % s.candidates.len()];
                    rest /= s.candidates.len();
                    c
                })
                .collect();
            let mut occs: Vec<usize> = chosen.iter().map(|c| c.occ).collect();
            occs.sort_unstable();
            occs.dedup();
            let [a, b] = occs[..] else { continue };
            let Some((a_to_b, b_to_a)) =
                self.directions(sites, &chosen, a, b, origin, outer_to_inner)
            else {
                continue;
            };
            let var_index = std::cell::Cell::new(0_usize);
            let column_of = |node: *mut pg_sys::Node| -> Option<Column> {
                // SAFETY: `node` is a Var or Param of `expr`.
                match unsafe { tag(node) } {
                    Some(pg_sys::NodeTag::T_Var) => {
                        chosen.get(var_index.get()).map(|c| (*c).clone())
                    }
                    Some(pg_sys::NodeTag::T_Param) => param_site(node)
                        .and_then(|i| chosen.get(var_count + i).map(|c| (*c).clone())),
                    _ => None,
                }
            };
            let mut next_var = || var_index.set(var_index.get() + 1);
            // SAFETY: deparse reads the same valid expression.
            let Some(sql) = (unsafe { self.deparse(expr, &column_of, &mut next_var) }) else {
                return;
            };
            // SAFETY: the same expression.
            let equality = unsafe { equality(expr, &chosen) };
            self.graph.conjuncts.push(Conjunct {
                sql,
                a,
                b,
                a_to_b,
                b_to_a,
                equality,
            });
        }
    }

    /// In which directions a predicate between occurrences `a` and `b` must hold.
    fn directions(
        &self,
        sites: &[Site],
        chosen: &[&Column],
        a: usize,
        b: usize,
        origin: Origin<'_>,
        outer_to_inner: bool,
    ) -> Option<(bool, bool)> {
        // The level of each occurrence relative to the predicate's: 0 here, n above,
        // usize::MAX below (a subquery's output).
        let level_of = |occ: usize| {
            sites
                .iter()
                .zip(chosen)
                .filter(|(_, c)| c.occ == occ)
                .map(|(s, _)| s.levelsup)
                .min()
        };
        let (la, lb) = (level_of(a)?, level_of(b)?);
        let depth = |l: usize| {
            if l == usize::MAX {
                -1_i64
            } else {
                i64::try_from(l).unwrap_or(i64::MAX)
            }
        };
        let (da, db) = (depth(la), depth(lb));
        // A predicate always holds for rows of the deeper occurrence; for rows of
        // the outer one only if every level in between must find a row.
        let outer_ok = |outer_levelsup: i64| outer_to_inner && self.levels_required(outer_levelsup);
        let (mut a_to_b, mut b_to_a) = match da.cmp(&db) {
            std::cmp::Ordering::Equal => (true, true),
            std::cmp::Ordering::Less => (true, outer_ok(db)),
            std::cmp::Ordering::Greater => (outer_ok(da), true),
        };
        if let Origin::Outer { nullable } = origin {
            a_to_b &= nullable.contains(&a);
            b_to_a &= nullable.contains(&b);
        }
        (a_to_b || b_to_a).then_some((a_to_b, b_to_a))
    }

    /// Whether a row of the level `outer` levels above the innermost one exists
    /// only if every level below it, down to the innermost, returns a row: each is
    /// a required subquery expression of the level above.
    fn levels_required(&self, outer: i64) -> bool {
        let top = i64::try_from(self.levels.len()).unwrap_or(i64::MAX) - 1;
        (0..outer).all(|up| {
            usize::try_from(top - up).is_ok_and(|index| {
                matches!(self.levels[index].link, Link::Sublink { required: true })
            })
        })
    }

    // ── Var resolution ──────────────────────────────────────────────────────

    /// What `var`, `levelsup` levels above the innermost one, stands for.
    ///
    /// SAFETY: `var` is a valid Var.
    unsafe fn resolve_var(&self, var: *mut pg_sys::Var, levelsup: usize) -> Resolved {
        // SAFETY: fields of a valid Var; RTE lists of valid levels.
        unsafe {
            let Some(index) = self.levels.len().checked_sub(1 + levelsup) else {
                return Resolved::Opaque;
            };
            let level = &self.levels[index];
            let (Ok(rtindex), attno) = (usize::try_from((*var).varno), (*var).varattno) else {
                return Resolved::Opaque;
            };
            if attno <= 0 {
                return Resolved::Opaque;
            }
            match level.rtes.get(rtindex.wrapping_sub(1)) {
                Some(RteInfo::Base(occ)) => {
                    let relid = Oid::from(self.graph.occurrences[*occ].relid);
                    Resolved::Col(Column {
                        occ: *occ,
                        attnum: attno,
                        name: cstr(pg_sys::get_attname(relid, attno, true)),
                    })
                }
                Some(RteInfo::Outputs(outputs)) => outputs
                    .get(attno as usize - 1)
                    .cloned()
                    .unwrap_or(Resolved::Opaque),
                Some(RteInfo::Join(aliases)) => {
                    let alias = elements::<pg_sys::Node>(*aliases)
                        .get(attno as usize - 1)
                        .copied()
                        .unwrap_or(std::ptr::null_mut());
                    self.resolve_alias(alias, index)
                }
                _ => Resolved::Opaque,
            }
        }
    }

    /// A join alias variable (or a PostgreSQL 18 group expression): a Var of the
    /// entry's own level, maybe behind a cast.
    ///
    /// SAFETY: `node` is null or a valid expression of level `index`.
    unsafe fn resolve_alias(&self, node: *mut pg_sys::Node, index: usize) -> Resolved {
        // SAFETY: checked by tag before each cast.
        unsafe {
            let node = strip_relabel(node);
            if tag(node) != Some(pg_sys::NodeTag::T_Var) {
                return Resolved::Opaque;
            }
            let var = node.cast::<pg_sys::Var>();
            let levelsup = self.levels.len() - 1 - index + (*var).varlevelsup as usize;
            self.resolve_var(var, levelsup)
        }
    }

    /// What an output expression stands for: a Var (maybe behind a cast) or opaque.
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    unsafe fn resolve_expr(&self, node: *mut pg_sys::Node) -> Resolved {
        // SAFETY: checked by tag before each cast.
        unsafe {
            let node = strip_relabel(node);
            if tag(node) == Some(pg_sys::NodeTag::T_Var) {
                let var = node.cast::<pg_sys::Var>();
                self.resolve_var(var, (*var).varlevelsup as usize)
            } else {
                Resolved::Opaque
            }
        }
    }

    // ── deparsing ───────────────────────────────────────────────────────────

    /// Write `expr` as SQL that resolves no name through `search_path`: relations,
    /// operators, functions and types are schema-qualified. `None` for a node it
    /// does not write. `column_of` gives the column of each Var / Param in walk
    /// order; `next_var` is called after each Var.
    ///
    /// SAFETY: `expr` is a valid expression.
    unsafe fn deparse(
        &mut self,
        expr: *mut pg_sys::Node,
        column_of: &dyn Fn(*mut pg_sys::Node) -> Option<Column>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(expr)? {
                pg_sys::NodeTag::T_Var => {
                    let column = column_of(expr)?;
                    next_var();
                    Some(column.sql())
                }
                pg_sys::NodeTag::T_Param => Some(column_of(expr)?.sql()),
                pg_sys::NodeTag::T_Const => {
                    let c = expr.cast::<pg_sys::Const>();
                    let ty = self.type_name((*c).consttype);
                    if (*c).constisnull {
                        return Some(Sql::text(format!("NULL::{ty}")));
                    }
                    let mut output: Oid = Oid::INVALID;
                    let mut varlena = false;
                    pg_sys::getTypeOutputInfo((*c).consttype, &raw mut output, &raw mut varlena);
                    let text = cstr(pg_sys::OidOutputFunctionCall(output, (*c).constvalue));
                    Some(Sql::text(format!("{}::{ty}", quote_literal(&text)?)))
                }
                pg_sys::NodeTag::T_OpExpr => {
                    let op = expr.cast::<pg_sys::OpExpr>();
                    let name = self.operator_name((*op).opno);
                    let args = elements::<pg_sys::Node>((*op).args);
                    let mut sql = Sql::text("(");
                    match args[..] {
                        [arg] => {
                            sql.push_text(&format!("OPERATOR({name}) "));
                            sql.push_sql(self.deparse(arg, column_of, next_var)?);
                        }
                        [l, r] => {
                            sql.push_sql(self.deparse(l, column_of, next_var)?);
                            sql.push_text(&format!(" OPERATOR({name}) "));
                            sql.push_sql(self.deparse(r, column_of, next_var)?);
                        }
                        _ => return None,
                    }
                    sql.push_text(")");
                    Some(sql)
                }
                pg_sys::NodeTag::T_ScalarArrayOpExpr => {
                    let op = expr.cast::<pg_sys::ScalarArrayOpExpr>();
                    let name = self.operator_name((*op).opno);
                    let [l, r] = elements::<pg_sys::Node>((*op).args)[..] else {
                        return None;
                    };
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse(l, column_of, next_var)?);
                    sql.push_text(&format!(
                        " OPERATOR({name}) {} (",
                        if (*op).useOr { "ANY" } else { "ALL" }
                    ));
                    sql.push_sql(self.deparse(r, column_of, next_var)?);
                    sql.push_text("))");
                    Some(sql)
                }
                pg_sys::NodeTag::T_FuncExpr => {
                    let f = expr.cast::<pg_sys::FuncExpr>();
                    let args = elements::<pg_sys::Node>((*f).args);
                    let mut sql;
                    if (*f).funcformat == pg_sys::CoercionForm::COERCE_EXPLICIT_CALL
                        || (*f).funcformat == pg_sys::CoercionForm::COERCE_SQL_SYNTAX
                    {
                        let (name, _) = self.function((*f).funcid);
                        sql = Sql::text(format!("{name}("));
                        for (i, arg) in args.into_iter().enumerate() {
                            if i > 0 {
                                sql.push_text(", ");
                            }
                            sql.push_sql(self.deparse(arg, column_of, next_var)?);
                        }
                        sql.push_text(")");
                    } else {
                        let [arg, ..] = args[..] else { return None };
                        sql = Sql::text("(");
                        sql.push_sql(self.deparse(arg, column_of, next_var)?);
                        sql.push_text(&format!(")::{}", self.type_name((*f).funcresulttype)));
                    }
                    Some(sql)
                }
                pg_sys::NodeTag::T_RelabelType => {
                    let r = expr.cast::<pg_sys::RelabelType>();
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse((*r).arg.cast(), column_of, next_var)?);
                    sql.push_text(&format!(")::{}", self.type_name((*r).resulttype)));
                    Some(sql)
                }
                pg_sys::NodeTag::T_CoerceViaIO => {
                    let r = expr.cast::<pg_sys::CoerceViaIO>();
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse((*r).arg.cast(), column_of, next_var)?);
                    sql.push_text(&format!(")::{}", self.type_name((*r).resulttype)));
                    Some(sql)
                }
                pg_sys::NodeTag::T_BoolExpr => {
                    let b = expr.cast::<pg_sys::BoolExpr>();
                    let args = elements::<pg_sys::Node>((*b).args);
                    let mut sql = Sql::text("(");
                    match (*b).boolop {
                        pg_sys::BoolExprType::NOT_EXPR => {
                            sql.push_text("NOT ");
                            sql.push_sql(self.deparse(*args.first()?, column_of, next_var)?);
                        }
                        op => {
                            let joiner = if op == pg_sys::BoolExprType::AND_EXPR {
                                " AND "
                            } else {
                                " OR "
                            };
                            for (i, arg) in args.into_iter().enumerate() {
                                if i > 0 {
                                    sql.push_text(joiner);
                                }
                                sql.push_sql(self.deparse(arg, column_of, next_var)?);
                            }
                        }
                    }
                    sql.push_text(")");
                    Some(sql)
                }
                _ => None,
            }
        }
    }

    fn type_name(&mut self, ty: Oid) -> String {
        self.catalog
            .types
            .entry(ty.to_u32())
            // SAFETY: a catalog lookup by OID.
            .or_insert_with(|| cstr(unsafe { pg_sys::format_type_be_qualified(ty) }))
            .clone()
    }

    fn operator_name(&mut self, opno: Oid) -> String {
        self.catalog
            .operators
            .entry(opno.to_u32())
            .or_insert_with(|| {
                Spi::get_one_with_args::<String>(
                    "SELECT pg_catalog.quote_ident(n.nspname) || '.' || o.oprname \
                     FROM pg_catalog.pg_operator o \
                     JOIN pg_catalog.pg_namespace n ON n.oid = o.oprnamespace \
                     WHERE o.oid = $1",
                    // SAFETY: the datum is a plain OID.
                    &[unsafe {
                        pgrx::datum::DatumWithOid::new(
                            opno,
                            PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
                        )
                    }],
                )
                .ok()
                .flatten()
                .unwrap_or_default()
            })
            .clone()
    }

    /// The qualified name of a function, and whether it is immutable.
    fn function(&mut self, funcid: Oid) -> (String, bool) {
        self.catalog
            .functions
            .entry(funcid.to_u32())
            .or_insert_with(|| {
                // SAFETY: catalog lookups by OID.
                unsafe {
                    let name = cstr(pg_sys::get_func_name(funcid));
                    let nsp = cstr(pg_sys::get_namespace_name(pg_sys::get_func_namespace(
                        funcid,
                    )));
                    let immutable = pg_sys::func_volatile(funcid)
                        == pg_sys::PROVOLATILE_IMMUTABLE.cast_signed();
                    (
                        format!("{}.{}", quote_ident(&nsp), quote_ident(&name)),
                        immutable,
                    )
                }
            })
            .clone()
    }

    // ── functions that may read tables ──────────────────────────────────────

    /// Record the non-immutable functions outside `pg_catalog` that this level
    /// calls: tables they read are invisible to `pg_tviews`.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level.
    unsafe fn note_functions(&mut self, query: *mut pg_sys::Node) {
        let mut funcids = Vec::new();
        // SAFETY: a read-only walk of a valid Query.
        unsafe { collect_functions(query, &mut funcids) };
        for funcid in funcids {
            let (name, immutable) = self.function(funcid);
            if !immutable
                && !name.starts_with("pg_catalog.")
                && !self.graph.untracked_functions.contains(&name)
            {
                self.graph.untracked_functions.push(name);
            }
        }
    }
}

impl Clone for Level {
    fn clone(&self) -> Self {
        Self {
            query: self.query,
            rtes: self.rtes.clone(),
            link: self.link,
        }
    }
}

fn list_len(list: *mut pg_sys::List) -> usize {
    if list.is_null() {
        0
    } else {
        // SAFETY: a valid, non-null List.
        usize::try_from(unsafe { (*list).length }).unwrap_or(0)
    }
}

/// Why a level's columns cannot be seen through from the level above.
///
/// SAFETY: `query` is a valid Query.
unsafe fn opaque_reason(query: *mut pg_sys::Query) -> Option<String> {
    // SAFETY: fields of a valid Query.
    unsafe {
        if (*query).hasWindowFuncs {
            Some("read under a window function".to_string())
        } else if !(*query).limitCount.is_null() || !(*query).limitOffset.is_null() {
            Some("read under LIMIT/OFFSET".to_string())
        } else if (*query).hasTargetSRFs {
            Some("read under a set-returning function".to_string())
        } else if !(*query).groupingSets.is_null() {
            Some("read under GROUPING SETS".to_string())
        } else {
            None
        }
    }
}

/// [`opaque_reason`] for a level whose output is the TVIEW itself.
///
/// SAFETY: `query` is a valid Query.
unsafe fn top_opaque_reason(query: *mut pg_sys::Query) -> Option<String> {
    // SAFETY: forwarded.
    unsafe { opaque_reason(query) }.map(|why| format!("{why} in the top-level SELECT"))
}

/// Whether sort/group reference `sortref` appears in a GROUP BY / DISTINCT clause.
///
/// SAFETY: `clause` is null or a valid list of `SortGroupClause`.
unsafe fn in_clause(sortref: pg_sys::Index, clause: *mut pg_sys::List) -> bool {
    // SAFETY: elements of a valid list.
    sortref != 0
        && unsafe { elements::<pg_sys::SortGroupClause>(clause) }
            .iter()
            // SAFETY: valid SortGroupClause pointers.
            .any(|c| unsafe { (**c).tleSortGroupRef } == sortref)
}

/// The leaf range table indexes of a set-operation tree, left to right.
///
/// SAFETY: `node` is a valid `SetOperationStmt` or `RangeTblRef`.
unsafe fn setop_leaves(node: *mut pg_sys::Node, leaves: &mut Vec<usize>) {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_RangeTblRef) => {
                leaves.push((*node.cast::<pg_sys::RangeTblRef>()).rtindex as usize);
            }
            Some(pg_sys::NodeTag::T_SetOperationStmt) => {
                let op = node.cast::<pg_sys::SetOperationStmt>();
                setop_leaves((*op).larg, leaves);
                setop_leaves((*op).rarg, leaves);
            }
            _ => {}
        }
    }
}

/// SAFETY: `node` is null or a valid expression.
unsafe fn strip_relabel(mut node: *mut pg_sys::Node) -> *mut pg_sys::Node {
    // SAFETY: checked by tag before each cast.
    unsafe {
        while tag(node) == Some(pg_sys::NodeTag::T_RelabelType) {
            node = (*node.cast::<pg_sys::RelabelType>()).arg.cast();
        }
    }
    node
}

/// A positive `EXISTS (…)` or `x IN (…)`.
///
/// SAFETY: `node` is a valid expression.
unsafe fn is_required_sublink(node: *mut pg_sys::Node) -> bool {
    // SAFETY: checked by tag before the cast.
    unsafe {
        tag(node) == Some(pg_sys::NodeTag::T_SubLink)
            && matches!(
                (*node.cast::<pg_sys::SubLink>()).subLinkType,
                pg_sys::SubLinkType::EXISTS_SUBLINK | pg_sys::SubLinkType::ANY_SUBLINK
            )
    }
}

/// `a.col = b.col` with `=`, as written.
///
/// SAFETY: `expr` is a valid expression.
unsafe fn equality(expr: *mut pg_sys::Node, chosen: &[&Column]) -> Option<(Column, Column)> {
    // SAFETY: checked by tag before each cast.
    unsafe {
        if tag(expr) != Some(pg_sys::NodeTag::T_OpExpr) || chosen.len() != 2 {
            return None;
        }
        let op = expr.cast::<pg_sys::OpExpr>();
        let args = elements::<pg_sys::Node>((*op).args);
        let plain = args.len() == 2
            && args.iter().all(|a| {
                matches!(
                    tag(strip_relabel(*a)),
                    Some(pg_sys::NodeTag::T_Var | pg_sys::NodeTag::T_Param)
                )
            });
        (plain && cstr(pg_sys::get_opname((*op).opno)) == "=")
            .then(|| (chosen[0].clone(), chosen[1].clone()))
    }
}

/// `name` quoted only where SQL needs it, as `quote_ident()` does.
fn quote_ident(name: &str) -> String {
    let Ok(c) = std::ffi::CString::new(name) else {
        return crate::utils::quote_identifier(name);
    };
    // SAFETY: a NUL-terminated string; the result is copied before `c` drops.
    cstr(unsafe { pg_sys::quote_identifier(c.as_ptr()) })
}

/// SQL string literal of `text`, quoted by PostgreSQL (`quote_literal()`).
fn quote_literal(text: &str) -> Option<String> {
    let c = std::ffi::CString::new(text).ok()?;
    // SAFETY: a NUL-terminated string; the palloc'd result is copied.
    Some(cstr(unsafe { pg_sys::quote_literal_cstr(c.as_ptr()) }))
}

// ── read-only collectors over expression trees ──────────────────────────────

/// The Vars of an expression, in walk order, not descending into subqueries.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn collect_vars(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Var>) {
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid.
        unsafe {
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Var) => {
                    (*ctx.cast::<Vec<*mut pg_sys::Var>>()).push(node.cast());
                    false
                }
                Some(pg_sys::NodeTag::T_SubLink | pg_sys::NodeTag::T_Query) => false,
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: `out` outlives the walk.
    unsafe { walker(node, std::ptr::from_mut(out).cast()) };
}

/// The `PARAM_SUBLINK` Params of an expression, in walk order.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn collect_params(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Param>) {
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid.
        unsafe {
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Param) => {
                    (*ctx.cast::<Vec<*mut pg_sys::Param>>()).push(node.cast());
                    false
                }
                Some(pg_sys::NodeTag::T_SubLink | pg_sys::NodeTag::T_Query) => false,
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: `out` outlives the walk.
    unsafe { walker(node, std::ptr::from_mut(out).cast()) };
}

/// The `SubLink`s of an expression, not those nested inside them.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn collect_sublinks(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::SubLink>) {
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid.
        unsafe {
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_SubLink) => {
                    (*ctx.cast::<Vec<*mut pg_sys::SubLink>>()).push(node.cast());
                    false
                }
                Some(pg_sys::NodeTag::T_Query) => false,
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: `out` outlives the walk.
    unsafe { walker(node, std::ptr::from_mut(out).cast()) };
}

/// Whether an expression holds a `SubLink`.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn has_sublink(node: *mut pg_sys::Node) -> bool {
    let mut found = Vec::new();
    // SAFETY: forwarded.
    unsafe { collect_sublinks(node, &mut found) };
    !found.is_empty()
}

/// The functions called by a query level's own expressions (`FuncExpr`, and the
/// functions of `RTE_FUNCTION` entries), not those of nested subqueries.
///
/// SAFETY: `node` is a valid Query.
unsafe fn collect_functions(node: *mut pg_sys::Node, out: &mut Vec<Oid>) {
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid.
        unsafe {
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_FuncExpr) => {
                    (*ctx.cast::<Vec<Oid>>()).push((*node.cast::<pg_sys::FuncExpr>()).funcid);
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                Some(pg_sys::NodeTag::T_SubLink | pg_sys::NodeTag::T_Query) => false,
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: walk the level's expressions; QTW flags skip subqueries and range
    // table subqueries (their own level records theirs).
    unsafe {
        pg_sys::query_tree_walker(
            node.cast(),
            Some(walker),
            std::ptr::from_mut(out).cast(),
            (pg_sys::QTW_IGNORE_RT_SUBQUERIES | pg_sys::QTW_IGNORE_CTE_SUBQUERIES).cast_signed(),
        );
    }
}
