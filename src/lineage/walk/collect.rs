//! Read-only collectors over expression trees.

use super::{
    FromDatum, HashMap, HashSet, Oid, cstr, elements, in_clause, pg_guard, pg_sys, strip_relabel,
    tag,
};

/// The columns `query` reads from each of its range table entries, by rtindex:
/// `Some(columns)`, or `None` when it reads the whole row. An entry it doesn't
/// read is absent. Vars of nested subqueries that point at this level count; a
/// reference through a join alias counts as one to the columns behind it.
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn referenced_columns(
    query: *mut pg_sys::Query,
) -> HashMap<usize, Option<HashSet<i16>>> {
    struct Refs {
        depth: u32,
        vars: Vec<(usize, i16)>,
    }
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Refs passed below; `node` is valid.
        unsafe {
            let refs = &mut *ctx.cast::<Refs>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Var) => {
                    let var = node.cast::<pg_sys::Var>();
                    if (*var).varlevelsup == refs.depth {
                        refs.vars.push(((*var).varno as usize, (*var).varattno));
                    }
                    false
                }
                Some(pg_sys::NodeTag::T_Query) => {
                    refs.depth += 1;
                    let done = pg_sys::query_tree_walker(node.cast(), Some(walker), ctx, 0);
                    refs.depth -= 1;
                    done
                }
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    let mut refs = Refs {
        depth: 0,
        vars: Vec::new(),
    };
    // Join aliases (and PostgreSQL 18 GROUP entries) are resolved below, not
    // walked: they name every column of the join.
    #[cfg(feature = "pg18")]
    let flags = pg_sys::QTW_IGNORE_JOINALIASES | pg_sys::QTW_IGNORE_GROUPEXPRS;
    #[cfg(not(feature = "pg18"))]
    let flags = pg_sys::QTW_IGNORE_JOINALIASES;
    // SAFETY: a read-only walk of a valid Query.
    unsafe {
        pg_sys::query_tree_walker(
            query,
            Some(walker),
            std::ptr::from_mut(&mut refs).cast(),
            flags.cast_signed(),
        );
    }
    // SAFETY: the range table of a valid Query.
    let rtable = unsafe { elements::<pg_sys::RangeTblEntry>((*query).rtable) };
    let mut read: HashMap<usize, Option<HashSet<i16>>> = HashMap::new();
    let mut pending = refs.vars;
    let mut seen: HashSet<(usize, i16)> = HashSet::new();
    while let Some((varno, attno)) = pending.pop() {
        if !seen.insert((varno, attno)) {
            continue;
        }
        let Some(&rte) = rtable.get(varno.wrapping_sub(1)) else {
            continue;
        };
        // SAFETY: fields of a valid RTE of this level.
        let behind = unsafe {
            match (*rte).rtekind {
                pg_sys::RTEKind::RTE_JOIN => Some((*rte).joinaliasvars),
                #[cfg(feature = "pg18")]
                pg_sys::RTEKind::RTE_GROUP => Some((*rte).groupexprs),
                _ => None,
            }
        };
        if let Some(list) = behind {
            // SAFETY: the alias expressions of the entry; Vars there point at
            // this level.
            let exprs = unsafe { elements::<pg_sys::Node>(list) };
            let chosen: Vec<*mut pg_sys::Node> = if attno == 0 {
                exprs
            } else {
                exprs
                    .get(usize::try_from(attno - 1).unwrap_or(usize::MAX))
                    .copied()
                    .into_iter()
                    .collect()
            };
            for expr in chosen {
                let mut vars = Vec::new();
                // SAFETY: a valid expression.
                unsafe { collect_vars(expr, &mut vars) };
                for var in vars {
                    // SAFETY: a Var collected above.
                    unsafe {
                        if (*var).varlevelsup == 0 {
                            pending.push(((*var).varno as usize, (*var).varattno));
                        }
                    }
                }
            }
            continue;
        }
        let entry = read.entry(varno).or_insert_with(|| Some(HashSet::new()));
        if attno == 0 {
            *entry = None;
        } else if let Some(columns) = entry {
            columns.insert(attno);
        }
    }
    read
}

/// A text constant (a `jsonb_build_object` key): its value.
///
/// SAFETY: `node` is null or a valid expression.
pub(super) unsafe fn const_text(node: *mut pg_sys::Node) -> Option<String> {
    // SAFETY: checked by tag before the cast; a non-null text datum.
    unsafe {
        let node = strip_relabel(node);
        if tag(node) != Some(pg_sys::NodeTag::T_Const) {
            return None;
        }
        let c = node.cast::<pg_sys::Const>();
        if (*c).constisnull {
            return None;
        }
        match (*c).consttype {
            pg_sys::TEXTOID | pg_sys::VARCHAROID => String::from_datum((*c).constvalue, false),
            pg_sys::UNKNOWNOID => Some(cstr((*c).constvalue.cast_mut_ptr())),
            _ => None,
        }
    }
}

/// How many times `query` reads each base column of its own level, `(rtindex,
/// attno)`, through join aliases (and PostgreSQL 18 group entries), in every
/// clause and subquery.
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn column_read_counts(
    query: *mut pg_sys::Query,
    skip_group_entries: bool,
) -> HashMap<(usize, i16), usize> {
    struct Refs {
        depth: u32,
        vars: Vec<*mut pg_sys::Var>,
    }
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Refs passed below; `node` is valid.
        unsafe {
            let refs = &mut *ctx.cast::<Refs>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Var) => {
                    let var = node.cast::<pg_sys::Var>();
                    if (*var).varlevelsup == refs.depth {
                        refs.vars.push(var);
                    }
                    false
                }
                Some(pg_sys::NodeTag::T_Query) => {
                    refs.depth += 1;
                    let done = pg_sys::query_tree_walker(node.cast(), Some(walker), ctx, 0);
                    refs.depth -= 1;
                    done
                }
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    let mut refs = Refs {
        depth: 0,
        vars: Vec::new(),
    };
    #[cfg(feature = "pg18")]
    let flags = pg_sys::QTW_IGNORE_JOINALIASES | pg_sys::QTW_IGNORE_GROUPEXPRS;
    #[cfg(not(feature = "pg18"))]
    let flags = pg_sys::QTW_IGNORE_JOINALIASES;
    // SAFETY: a read-only walk of a valid Query.
    unsafe {
        pg_sys::query_tree_walker(
            query,
            Some(walker),
            std::ptr::from_mut(&mut refs).cast(),
            flags.cast_signed(),
        );
    }
    // The hidden target entries a GROUP BY adds only name the groups.
    let mut skipped: Vec<*mut pg_sys::Var> = Vec::new();
    if skip_group_entries {
        // SAFETY: the target list of a valid Query.
        unsafe {
            for tle in elements::<pg_sys::TargetEntry>((*query).targetList) {
                if (*tle).resjunk && in_clause((*tle).ressortgroupref, (*query).groupClause) {
                    collect_vars((*tle).expr.cast(), &mut skipped);
                }
            }
        }
    }
    let mut counts: HashMap<(usize, i16), usize> = HashMap::new();
    for var in refs.vars.into_iter().filter(|v| !skipped.contains(v)) {
        // SAFETY: a Var collected above, of `query`'s level as seen from its own.
        let (varno, attno) = unsafe { ((*var).varno as usize, (*var).varattno) };
        for read in base_reads(query, varno, attno) {
            *counts.entry(read).or_default() += 1;
        }
    }
    counts
}

/// Whether entry `rtindex` of `query`'s range table is a table (not a view, whose
/// other clauses may read the column too).
pub(super) fn relation_entry(query: *mut pg_sys::Query, rtindex: usize) -> bool {
    // SAFETY: the range table of a valid Query.
    unsafe {
        elements::<pg_sys::RangeTblEntry>((*query).rtable)
            .get(rtindex.wrapping_sub(1))
            .is_some_and(|rte| {
                (**rte).rtekind == pg_sys::RTEKind::RTE_RELATION
                    && matches!(
                        (**rte).relkind as u8,
                        pg_sys::RELKIND_RELATION | pg_sys::RELKIND_PARTITIONED_TABLE
                    )
            })
    }
}

/// The base column a Var of `query`'s own level reads, seen through join
/// aliases; `None` when it stands for several (or for none).
///
/// SAFETY: `var` is a valid Var.
pub(super) unsafe fn base_read(
    query: *mut pg_sys::Query,
    var: *mut pg_sys::Var,
) -> Option<(usize, i16)> {
    // SAFETY: fields of a valid Var.
    let (varno, attno) = unsafe { ((*var).varno as usize, (*var).varattno) };
    match base_reads(query, varno, attno)[..] {
        [read] => Some(read),
        _ => None,
    }
}

/// The base columns `(rtindex, attno)` of `query`'s level that `(varno, attno)`
/// stands for: itself, or what a join alias or group entry expands to.
pub(super) fn base_reads(query: *mut pg_sys::Query, varno: usize, attno: i16) -> Vec<(usize, i16)> {
    // SAFETY: the range table of a valid Query.
    let rtable = unsafe { elements::<pg_sys::RangeTblEntry>((*query).rtable) };
    let Some(&rte) = rtable.get(varno.wrapping_sub(1)) else {
        return Vec::new();
    };
    // SAFETY: fields of a valid RTE of this level.
    let behind = unsafe {
        match (*rte).rtekind {
            pg_sys::RTEKind::RTE_JOIN => Some((*rte).joinaliasvars),
            #[cfg(feature = "pg18")]
            pg_sys::RTEKind::RTE_GROUP => Some((*rte).groupexprs),
            _ => None,
        }
    };
    let Some(list) = behind else {
        return vec![(varno, attno)];
    };
    // SAFETY: the alias expressions of the entry; Vars there point at this level.
    let exprs = unsafe { elements::<pg_sys::Node>(list) };
    let chosen: Vec<*mut pg_sys::Node> = if attno <= 0 {
        exprs
    } else {
        exprs
            .get(usize::try_from(attno - 1).unwrap_or(usize::MAX))
            .copied()
            .into_iter()
            .collect()
    };
    let mut out = Vec::new();
    for expr in chosen {
        let mut vars = Vec::new();
        // SAFETY: a valid expression.
        unsafe { collect_vars(expr, &mut vars) };
        for var in vars {
            // SAFETY: a Var collected above.
            unsafe {
                if (*var).varlevelsup == 0 {
                    out.extend(base_reads(query, (*var).varno as usize, (*var).varattno));
                }
            }
        }
    }
    out
}

/// The Vars of an expression, in walk order, not descending into subqueries.
///
/// SAFETY: `node` is null or a valid expression.
pub(super) unsafe fn collect_vars(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Var>) {
    #[pg_guard]
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
pub(super) unsafe fn collect_params(node: *mut pg_sys::Node, out: &mut Vec<*mut pg_sys::Param>) {
    #[pg_guard]
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
pub(super) unsafe fn collect_sublinks(
    node: *mut pg_sys::Node,
    out: &mut Vec<*mut pg_sys::SubLink>,
) {
    #[pg_guard]
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
pub(super) unsafe fn has_sublink(node: *mut pg_sys::Node) -> bool {
    let mut found = Vec::new();
    // SAFETY: forwarded.
    unsafe { collect_sublinks(node, &mut found) };
    !found.is_empty()
}

/// The functions called by a query level's own expressions (`FuncExpr`, and the
/// functions of `RTE_FUNCTION` entries), not those of nested subqueries.
///
/// SAFETY: `node` is a valid Query.
pub(super) unsafe fn collect_functions(node: *mut pg_sys::Node, out: &mut Vec<Oid>) {
    #[pg_guard]
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
