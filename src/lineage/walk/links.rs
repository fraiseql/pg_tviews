//! The columns a TVIEW's rows are linked by (ADR 0207, #263): those that decide
//! which rows exist and which base rows each is computed from, as opposed to the
//! columns whose values a row only shows.
//!
//! A column is a link when a level reads it in a join or filter condition, a
//! GROUP BY (other than the identity's level grouped by the identity), a
//! DISTINCT, a window's PARTITION BY or ORDER BY, the ORDER BY of a level under
//! LIMIT/OFFSET, an aggregate's FILTER, the arguments of a function in FROM, or a
//! subquery expression compared in one of those; the TVIEW's key is one too. A
//! read this cannot follow to columns (an aggregate compared above its level, a
//! whole-row reference, a set operation that removes duplicates) makes every
//! column a link.

use super::{
    Flags, Piece, Resolved, Walker, collect_sublinks, collect_vars, elements, in_clause, pg_guard,
    pg_sys, tag,
};

impl Walker<'_> {
    /// Before a level is walked: remember the subquery expressions it compares in
    /// link positions, whose outputs are links too ([`Walker::sublink_links`]).
    ///
    /// SAFETY: `query` is a valid Query.
    pub(super) unsafe fn note_link_sublinks(&mut self, query: *mut pg_sys::Query, flags: &Flags) {
        if flags.unread {
            return;
        }
        // SAFETY: the clauses of a valid Query.
        unsafe {
            let mut nodes = link_nodes(query);
            nodes.extend(clause_exprs(query, (*query).groupClause));
            for node in nodes {
                let mut found = Vec::new();
                collect_sublinks(node, &mut found);
                self.link_sublinks
                    .extend(found.into_iter().map(|s| s as usize));
            }
        }
    }

    /// After a level is walked: record the columns it reads in link positions.
    /// `identity_level` is the backing view's own SELECT, whose GROUP BY only
    /// names its rows when it groups by the identity.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level, whose range table
    /// entries are known.
    pub(super) unsafe fn note_links(
        &mut self,
        query: *mut pg_sys::Query,
        flags: &Flags,
        identity_level: bool,
    ) {
        if flags.unread {
            return;
        }
        // SAFETY: the clauses and target list of a valid Query.
        unsafe {
            let mut nodes = link_nodes(query);
            let grouped_by_identity = identity_level
                && (*query).groupingSets.is_null()
                && match &self.graph.identity {
                    Some(Ok(identity)) => elements::<pg_sys::TargetEntry>((*query).targetList)
                        .get(identity.position)
                        .is_some_and(|tle| {
                            in_clause((**tle).ressortgroupref, (*query).groupClause)
                        }),
                    _ => false,
                };
            if !grouped_by_identity {
                nodes.extend(clause_exprs(query, (*query).groupClause));
            }
            // The left side of `x IN (…)` and the like, in a link position, is a
            // comparison of this level.
            let mut tests = Vec::new();
            for &node in &nodes {
                let mut found = Vec::new();
                collect_sublinks(node, &mut found);
                tests.extend(found.into_iter().map(|s| (*s).testexpr));
            }
            nodes.extend(tests);
            for node in nodes {
                let mut vars = Vec::new();
                collect_vars(node, &mut vars);
                for var in vars {
                    let resolved = self.resolve_var(var, (*var).varlevelsup as usize);
                    self.add_links(&resolved);
                }
            }
        }
    }

    /// A subquery expression just walked, with its outputs: when its level above
    /// compares it in a link position, what it returns is a link too. An
    /// `EXISTS` returns no column: its own conditions are its links.
    ///
    /// SAFETY: `sublink` is a valid `SubLink`.
    pub(super) unsafe fn sublink_links(
        &mut self,
        sublink: *mut pg_sys::SubLink,
        outputs: &[Resolved],
    ) {
        // SAFETY: a field of a valid SubLink.
        let exists = unsafe { (*sublink).subLinkType } == pg_sys::SubLinkType::EXISTS_SUBLINK;
        if exists || !self.link_sublinks.contains(&(sublink as usize)) {
            return;
        }
        for output in outputs {
            self.add_links(output);
        }
    }

    /// A set operation that removes duplicates (`UNION`, `INTERSECT`, `EXCEPT`)
    /// compares whole rows: every column is a link.
    ///
    /// SAFETY: `node` is null or a valid set-operation tree.
    pub(super) unsafe fn set_operation_links(&mut self, node: *mut pg_sys::Node) {
        // SAFETY: forwarded.
        if unsafe { removes_duplicates(node) } {
            self.graph.links_unknown = true;
        }
    }

    /// Record what a column read in a link position stands for.
    pub(super) fn add_links(&mut self, resolved: &Resolved) {
        match resolved {
            Resolved::Col(c) | Resolved::Inbound(c) => {
                self.graph.links.insert((c.occ, c.attnum));
            }
            Resolved::Expr(e) => {
                for piece in &e.sql.0 {
                    if let Piece::Column { occ, attnum } = piece {
                        self.graph.links.insert((*occ, *attnum));
                    }
                }
            }
            Resolved::Alt(terms, holes) => {
                if !holes.is_empty() {
                    self.graph.links_unknown = true;
                }
                for term in terms {
                    self.add_links(term);
                }
            }
            Resolved::Opaque => self.graph.links_unknown = true,
        }
    }
}

/// The expressions of a level read in link positions, but its GROUP BY: join and
/// filter conditions, HAVING, DISTINCT (ON) with the ORDER BY that picks its
/// rows, windows, the ORDER BY under LIMIT/OFFSET, aggregate FILTERs and the
/// arguments of functions in FROM.
///
/// SAFETY: `query` is a valid Query.
unsafe fn link_nodes(query: *mut pg_sys::Query) -> Vec<*mut pg_sys::Node> {
    // SAFETY: the clauses, target list and range table of a valid Query.
    unsafe {
        let mut nodes = Vec::new();
        jointree_quals((*query).jointree.cast(), &mut nodes);
        nodes.push((*query).havingQual);
        if (*query).hasDistinctOn {
            nodes.extend(clause_exprs(query, (*query).distinctClause));
            nodes.extend(clause_exprs(query, (*query).sortClause));
        } else if !(*query).distinctClause.is_null() {
            nodes.extend(
                elements::<pg_sys::TargetEntry>((*query).targetList)
                    .iter()
                    .filter(|tle| !(***tle).resjunk)
                    .map(|tle| (**tle).expr.cast()),
            );
        }
        for window in elements::<pg_sys::WindowClause>((*query).windowClause) {
            nodes.extend(clause_exprs(query, (*window).partitionClause));
            nodes.extend(clause_exprs(query, (*window).orderClause));
        }
        if !(*query).limitCount.is_null() || !(*query).limitOffset.is_null() {
            nodes.extend(clause_exprs(query, (*query).sortClause));
        }
        for tle in elements::<pg_sys::TargetEntry>((*query).targetList) {
            aggregate_filters((*tle).expr.cast(), &mut nodes);
        }
        aggregate_filters((*query).havingQual, &mut nodes);
        for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
            match (*rte).rtekind {
                pg_sys::RTEKind::RTE_FUNCTION => {
                    for function in elements::<pg_sys::RangeTblFunction>((*rte).functions) {
                        nodes.push((*function).funcexpr);
                    }
                }
                pg_sys::RTEKind::RTE_TABLEFUNC => nodes.push((*rte).tablefunc.cast()),
                pg_sys::RTEKind::RTE_VALUES => nodes.push((*rte).values_lists.cast()),
                _ => {}
            }
        }
        nodes.retain(|n| !n.is_null());
        nodes
    }
}

/// The conditions of a join tree: `WHERE` and every `JOIN … ON`.
///
/// SAFETY: `node` is null or a valid jointree node.
unsafe fn jointree_quals(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Node>) {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_FromExpr) => {
                let from = node.cast::<pg_sys::FromExpr>();
                for item in elements::<pg_sys::Node>((*from).fromlist) {
                    jointree_quals(item, out);
                }
                out.push((*from).quals);
            }
            Some(pg_sys::NodeTag::T_JoinExpr) => {
                let join = node.cast::<pg_sys::JoinExpr>();
                jointree_quals((*join).larg, out);
                jointree_quals((*join).rarg, out);
                out.push((*join).quals);
            }
            _ => {}
        }
    }
}

/// The expressions of the target entries a sort/group clause names.
///
/// SAFETY: `query` is a valid Query and `clause` null or one of its clauses.
unsafe fn clause_exprs(
    query: *mut pg_sys::Query,
    clause: *mut pg_sys::List,
) -> Vec<*mut pg_sys::Node> {
    // SAFETY: the target list of a valid Query.
    unsafe {
        elements::<pg_sys::TargetEntry>((*query).targetList)
            .iter()
            .filter(|tle| in_clause((***tle).ressortgroupref, clause))
            .map(|tle| (**tle).expr.cast())
            .collect()
    }
}

/// The `FILTER` conditions of the aggregates and window functions of an
/// expression, not those of subquery expressions inside it.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn aggregate_filters(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Node>) {
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid and checked by tag
        // before each cast.
        unsafe {
            let out = &mut *ctx.cast::<Vec<*mut pg_sys::Node>>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Aggref) => {
                    out.push((*node.cast::<pg_sys::Aggref>()).aggfilter.cast());
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                Some(pg_sys::NodeTag::T_WindowFunc) => {
                    out.push((*node.cast::<pg_sys::WindowFunc>()).aggfilter.cast());
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                Some(pg_sys::NodeTag::T_SubLink | pg_sys::NodeTag::T_Query) => false,
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: `out` outlives the walk.
    unsafe { walker(node, std::ptr::from_mut(out).cast()) };
}

/// Whether a set-operation tree holds an operation other than `UNION ALL`.
///
/// SAFETY: `node` is null or a valid set-operation tree.
unsafe fn removes_duplicates(node: *mut pg_sys::Node) -> bool {
    // SAFETY: checked by tag before the cast.
    unsafe {
        if tag(node) != Some(pg_sys::NodeTag::T_SetOperationStmt) {
            return false;
        }
        let op = node.cast::<pg_sys::SetOperationStmt>();
        (*op).op != pg_sys::SetOperation::SETOP_UNION
            || !(*op).all
            || removes_duplicates((*op).larg)
            || removes_duplicates((*op).rarg)
    }
}
