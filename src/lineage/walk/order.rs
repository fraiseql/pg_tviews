//! The order rule (dialect.md Axis D, D13): wherever the order of rows reaches a
//! definition's output, it is total.
//!
//! A *site* orders rows: an order-sensitive aggregate (`array_agg`, `jsonb_agg`,
//! `string_agg`… any user-defined aggregate, `sum`/`avg` of a float), a level with
//! `LIMIT` or a non-zero `OFFSET`, an `ARRAY(SELECT …)` (its rows become elements in
//! order), `DISTINCT ON`, an order-sensitive window function.
//! The rows it orders are its level's relations (the group's, for a grouped level;
//! the partition's, for a window). The *given* columns are the `GROUP BY` or
//! `PARTITION BY` keys, every outer reference and every constant; the closure of a
//! set of columns adds, to a fixpoint, the columns a level's equality conjunct
//! (`a.x = b.y`, `a.x = <known>`, from `WHERE` and inner joins; from an outer join's
//! condition, its nullable side only) equates with a known expression, every column
//! of a relation once a unique key of it is known, and the outputs of a view,
//! subquery or CTE not determined that its known outputs fix all the same (one value
//! in every row they reach: not a set-returning output, nor one of a relation reading
//! the level). A scalar subquery is known when every column of the level it reads is,
//! and it holds no aggregate of the level. A site is accepted when:
//!
//! 1. its sort keys and the given columns close over a unique key of every relation
//!    it orders (*total order*), or, for an aggregate, over every value it reads
//!    (rows that tie are then identical where it looks);
//! 2. the given columns alone do (*at most one row*);
//! 3. an aggregate is ordered by every non-constant argument, or is `DISTINCT`, and
//!    each such argument's type renders equal values identically;
//! 4. a `UNION ALL` of branches of at most one row each is ordered by an output
//!    holding a different constant in each branch (there, or in the subquery or CTE
//!    whose column the branch outputs).
//!
//! A relation's unique keys: a valid, immediate unique index without expressions,
//! when the level's conjuncts imply its predicate (`predicate_implied_by`) and each
//! nullable column's `IS NOT NULL` (a strict equality does; a sort key does not: rows
//! NULL in it tie) unless `NULLS NOT DISTINCT`; for a relation on the nullable side of
//! outer joins, their conditions count too (they hold for every row it joins); a function in `FROM` `WITH ORDINALITY`, by
//! its ordinality; a function returning one row, by its arguments; any relation,
//! by all its columns (the rows that tie are identical); a view, subquery
//! or CTE, by the outputs that determine its rows by this same analysis, or that fix
//! its `GROUP BY` or `DISTINCT` columns (either), or always when it returns one row
//! (an ungrouped aggregate, `LIMIT 1`, also over a set operation). A set operation is keyed by all its outputs when it
//! removes duplicates, and by rule 4's rank (with each branch's own key) otherwise.

use super::{Oid, cstr, elements, index, pg_guard, pg_sys, tag, view_query};
use crate::error::{TViewError, TViewResult};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A column of a level: its range-table index and attribute number.
type Col = (usize, i16);

/// A unique key of a table.
struct UniqueKey {
    columns: Vec<i16>,
    /// The partial index's predicate (implicit AND, `varno` 1), null when none.
    predicate: *mut pg_sys::List,
    /// `IS NOT NULL` of each nullable column (implicit AND, `varno` 1), null when
    /// none: rows NULL in one are not told apart, so the level must exclude them.
    not_null: *mut pg_sys::List,
    primary: bool,
}

/// The query levels around the one analyzed, outermost first: its CTEs are found
/// in them, and its outer references point at them.
type Stack = Vec<*mut pg_sys::Query>;

struct Checker {
    keys: HashMap<Oid, Vec<UniqueKey>>,
    views: Vec<Oid>,
    /// The view being walked, `None` for the backing view.
    view_name: Option<String>,
    refused: Option<String>,
}

/// Refuse a definition with a site whose order is not total: the backing view
/// `view_oid` and every view it reads.
///
/// # Errors
/// [`TViewError::DefinitionRefused`] (`0A000`) naming the first site refused and a
/// key to append; the error opening a view.
pub fn check(view_oid: Oid) -> TViewResult<()> {
    let mut checker = Checker {
        keys: HashMap::new(),
        views: vec![view_oid],
        view_name: None,
        refused: None,
    };
    let mut seen: Vec<Oid> = Vec::new();
    while let Some(view) = checker.views.pop() {
        if seen.contains(&view) {
            continue;
        }
        seen.push(view);
        checker.view_name = (view != view_oid).then(|| relation_name(view));
        // SAFETY: `view_query` returns a copy in the current memory context; the
        // analysis only reads it (index predicates are copied before they are
        // renumbered).
        unsafe {
            let query = view_query(view)?;
            checker.visit(query, false, &mut Vec::new());
        }
        if let Some(reason) = checker.refused.take() {
            return Err(TViewError::DefinitionRefused { reason });
        }
    }
    Ok(())
}

fn relation_name(relid: Oid) -> String {
    // SAFETY: a catalog lookup by OID; the string is copied.
    cstr(unsafe { pg_sys::format_type_be_qualified(pg_sys::get_rel_type_id(relid)) })
}

/// The OIDs of an OID list.
///
/// SAFETY: `list` is null or a valid OID list.
unsafe fn oids(list: *mut pg_sys::List) -> Vec<Oid> {
    if list.is_null() {
        return Vec::new();
    }
    // SAFETY: indexes stay below the list's length; the cells hold OIDs.
    unsafe {
        // walk: n/a(a list length is never negative)
        (0..usize::try_from((*list).length).unwrap_or(0))
            .map(|i| (*(*list).elements.add(i)).oid_value)
            .collect()
    }
}

/// `node` without the relabelings and collations around it.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn strip(mut node: *mut pg_sys::Node) -> *mut pg_sys::Node {
    // SAFETY: checked by tag before each cast.
    unsafe {
        loop {
            node = match tag(node) {
                Some(pg_sys::NodeTag::T_RelabelType) => {
                    (*node.cast::<pg_sys::RelabelType>()).arg.cast()
                }
                Some(pg_sys::NodeTag::T_CollateExpr) => {
                    (*node.cast::<pg_sys::CollateExpr>()).arg.cast()
                }
                // walk: n/a(anything else is the expression itself)
                _ => return node,
            };
        }
    }
}

/// The range-table entry `rti` of `query`.
///
/// SAFETY: `query` is a valid Query.
unsafe fn rte(query: *mut pg_sys::Query, rti: usize) -> Option<*mut pg_sys::RangeTblEntry> {
    // SAFETY: the range table of a valid Query.
    unsafe {
        elements::<pg_sys::RangeTblEntry>((*query).rtable)
            .get(rti.checked_sub(1)?)
            .copied()
    }
}

/// The target entry of `query` a sort or group clause names.
///
/// SAFETY: `query` is a valid `Query`, `clause` a `SortGroupClause` of it.
unsafe fn clause_target(
    query: *mut pg_sys::Query,
    clause: *mut pg_sys::SortGroupClause,
) -> *mut pg_sys::TargetEntry {
    // SAFETY: valid nodes of the same Query.
    unsafe {
        let wanted = (*clause).tleSortGroupRef;
        elements::<pg_sys::TargetEntry>((*query).targetList)
            .into_iter()
            .find(|&te| (*te).ressortgroupref == wanted)
            // walk: n/a(a clause names an entry of its own target list)
            .unwrap_or(std::ptr::null_mut())
    }
}

/// The expressions of a list of sort or group clauses of `query`.
///
/// SAFETY: `query` is a valid `Query`, `clauses` a list of its `SortGroupClause`s.
unsafe fn clause_exprs(
    query: *mut pg_sys::Query,
    clauses: *mut pg_sys::List,
) -> Vec<*mut pg_sys::Node> {
    // SAFETY: valid nodes of the same Query.
    unsafe {
        elements::<pg_sys::SortGroupClause>(clauses)
            .into_iter()
            .map(|c| clause_target(query, c))
            .filter(|te| !te.is_null())
            .map(|te| (*te).expr.cast())
            .collect()
    }
}

/// The range-table indexes of the relations a join tree reads.
///
/// SAFETY: `node` is null or a valid join tree node.
unsafe fn leaves(node: *mut pg_sys::Node) -> Vec<usize> {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_RangeTblRef) => {
                vec![index((*node.cast::<pg_sys::RangeTblRef>()).rtindex)]
            }
            Some(pg_sys::NodeTag::T_JoinExpr) => {
                let j = node.cast::<pg_sys::JoinExpr>();
                let mut out = leaves((*j).larg);
                out.extend(leaves((*j).rarg));
                out
            }
            Some(pg_sys::NodeTag::T_FromExpr) => {
                elements::<pg_sys::Node>((*node.cast::<pg_sys::FromExpr>()).fromlist)
                    .into_iter()
                    .flat_map(|n| leaves(n))
                    .collect()
            }
            // walk: n/a(a join tree holds only these)
            _ => Vec::new(),
        }
    }
}

/// The AND-ed conjuncts of a qual.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn conjuncts(node: *mut pg_sys::Node) -> Vec<*mut pg_sys::Node> {
    // SAFETY: checked by tag before each cast.
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
            // walk: n/a(any other qual is one conjunct)
            Some(_) => vec![node],
        }
    }
}

/// One level's facts: its equalities, by the relations whose columns they may
/// determine (`None`: any), the conjuncts every row it returns satisfies, and, for
/// a relation on the nullable side of outer joins, their conditions: they hold
/// whenever one of its rows is joined (a null-extended row is no row of it).
struct Facts {
    equalities: Vec<(*mut pg_sys::Node, *mut pg_sys::Node, Option<HashSet<usize>>)>,
    conjuncts: Vec<*mut pg_sys::Node>,
    joined: HashMap<usize, Vec<*mut pg_sys::Node>>,
}

impl Facts {
    /// The conjuncts every row of relation `rti` the level joins satisfies.
    fn of_relation(&self, rti: usize) -> Vec<*mut pg_sys::Node> {
        let mut all = self.conjuncts.clone();
        all.extend(self.joined.get(&rti).into_iter().flatten().copied());
        all
    }
}

/// SAFETY: `query` is a valid Query.
unsafe fn facts(query: *mut pg_sys::Query) -> Facts {
    let mut facts = Facts {
        equalities: Vec::new(),
        conjuncts: Vec::new(),
        joined: HashMap::new(),
    };
    // SAFETY: valid nodes of the Query's join tree.
    unsafe {
        visit_join((*query).jointree.cast(), None, true, &mut facts);
    }
    facts
}

/// The equalities of `quals` (determining the relations `allowed`), and, when
/// `every_row`, its conjuncts as facts of every row.
///
/// SAFETY: `quals` is null or a valid expression.
unsafe fn add_quals(
    quals: *mut pg_sys::Node,
    allowed: Option<&HashSet<usize>>,
    every_row: bool,
    facts: &mut Facts,
) {
    // SAFETY: checked by tag before each cast.
    unsafe {
        for c in conjuncts(quals) {
            if every_row {
                facts.conjuncts.push(c);
            }
            if tag(c) != Some(pg_sys::NodeTag::T_OpExpr) {
                continue;
            }
            let op = c.cast::<pg_sys::OpExpr>();
            let args = elements::<pg_sys::Node>((*op).args);
            // An equality of a btree operator family: a unique index's equality,
            // so a value equal to a key's names one row.
            if args.len() == 2 && pg_sys::op_mergejoinable((*op).opno, pg_sys::exprType(args[0])) {
                facts.equalities.push((args[0], args[1], allowed.cloned()));
            }
        }
    }
}

/// The facts of a join tree node: `allowed` the relations its equalities may
/// determine (`None`: any), `every_row` whether its quals hold for every row the
/// level returns (not on the nullable side of an outer join).
///
/// SAFETY: `node` is null or a valid join tree node.
unsafe fn visit_join(
    node: *mut pg_sys::Node,
    allowed: Option<&HashSet<usize>>,
    every_row: bool,
    facts: &mut Facts,
) {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_FromExpr) => {
                let f = node.cast::<pg_sys::FromExpr>();
                for n in elements::<pg_sys::Node>((*f).fromlist) {
                    visit_join(n, allowed, every_row, facts);
                }
                add_quals((*f).quals, allowed, every_row, facts);
            }
            Some(pg_sys::NodeTag::T_JoinExpr) => {
                let j = node.cast::<pg_sys::JoinExpr>();
                match (*j).jointype {
                    pg_sys::JoinType::JOIN_INNER => {
                        visit_join((*j).larg, allowed, every_row, facts);
                        visit_join((*j).rarg, allowed, every_row, facts);
                        add_quals((*j).quals, allowed, every_row, facts);
                    }
                    pg_sys::JoinType::JOIN_LEFT | pg_sys::JoinType::JOIN_RIGHT => {
                        let (kept, nullable) = if (*j).jointype == pg_sys::JoinType::JOIN_LEFT {
                            ((*j).larg, (*j).rarg)
                        } else {
                            ((*j).rarg, (*j).larg)
                        };
                        let side: HashSet<usize> = leaves(nullable).into_iter().collect();
                        let narrowed: HashSet<usize> = match allowed {
                            None => side,
                            Some(a) => side.intersection(a).copied().collect(),
                        };
                        visit_join(kept, allowed, every_row, facts);
                        visit_join(nullable, Some(&narrowed), false, facts);
                        // Its condition holds for the rows it joins, not for every row.
                        add_quals((*j).quals, Some(&narrowed), false, facts);
                        let conditions = conjuncts((*j).quals);
                        for rti in leaves(nullable) {
                            facts.joined.entry(rti).or_default().extend(&conditions);
                        }
                    }
                    // walk: n/a(a FULL join determines no side from the other)
                    _ => {
                        let none = HashSet::new();
                        visit_join((*j).larg, Some(&none), false, facts);
                        visit_join((*j).rarg, Some(&none), false, facts);
                    }
                }
            }
            // walk: n/a(a range-table reference has no qual)
            _ => {}
        }
    }
}

/// The analysis of one level: the columns known, the relations whose rows they
/// determine.
struct Level<'a> {
    query: *mut pg_sys::Query,
    stack: &'a Stack,
    facts: Facts,
}

impl Level<'_> {
    /// The column a level-0 Var stands for, through join aliases and group
    /// entries; `None` for an expression.
    ///
    /// SAFETY: `node` is null or a valid expression of this level.
    unsafe fn column(&self, node: *mut pg_sys::Node) -> Option<Col> {
        // SAFETY: checked by tag before each cast; entries of this level.
        unsafe {
            let mut node = strip(node);
            for _ in 0..64 {
                if tag(node) != Some(pg_sys::NodeTag::T_Var) {
                    return None;
                }
                let var = node.cast::<pg_sys::Var>();
                if (*var).varlevelsup != 0 {
                    return None;
                }
                let entry = rte(self.query, index((*var).varno))?;
                let attno = (*var).varattno;
                let list = match (*entry).rtekind {
                    pg_sys::RTEKind::RTE_JOIN => (*entry).joinaliasvars,
                    pg_sys::RTEKind::RTE_GROUP => (*entry).groupexprs,
                    // walk: n/a(a relation, subquery, CTE or function: the column itself)
                    _ => return Some((index((*var).varno), attno)),
                };
                if attno <= 0 {
                    return None;
                }
                node = strip(*elements::<pg_sys::Node>(list).get(index(attno) - 1)?);
            }
            None
        }
    }

    /// Whether `node` is known: every Var of this level in it is known or of a
    /// determined relation, with no aggregate, window function or sublink.
    ///
    /// SAFETY: `node` is null or a valid expression of this level.
    /// Whether every column of this level that `subselect` (a subquery of it) reads
    /// is known, and it holds no aggregate of this level (one value per group, not
    /// per known row).
    ///
    /// SAFETY: `subselect` is a valid Query nested in this level.
    unsafe fn reads_known(
        &self,
        subselect: *mut pg_sys::Node,
        known: &HashSet<Col>,
        det: &HashSet<usize>,
    ) -> bool {
        // SAFETY: nodes of a valid Query; the Vars are copied before they are changed.
        unsafe {
            if crate::utils::contain_aggs_of_level(subselect, 1) {
                return false;
            }
            if !pg_sys::contain_vars_of_level(subselect, 1) {
                return true;
            }
            elements::<pg_sys::Node>(pg_sys::pull_vars_of_level(subselect, 1))
                .into_iter()
                .all(|v| {
                    if tag(v) != Some(pg_sys::NodeTag::T_Var) {
                        return false;
                    }
                    let var = pg_sys::copyObjectImpl(v.cast()).cast::<pg_sys::Var>();
                    (*var).varlevelsup = 0;
                    self.known(var.cast(), known, det)
                })
        }
    }

    unsafe fn known(
        &self,
        node: *mut pg_sys::Node,
        known: &HashSet<Col>,
        det: &HashSet<usize>,
    ) -> bool {
        struct Ctx<'b, 'c> {
            level: &'b Level<'c>,
            known: &'b HashSet<Col>,
            det: &'b HashSet<usize>,
        }
        #[pg_guard]
        unsafe extern "C-unwind" fn walker(
            node: *mut pg_sys::Node,
            ctx: *mut std::ffi::c_void,
        ) -> bool {
            // SAFETY: `ctx` is the Ctx below; `node` is valid. Returns true to stop
            // at the first node not known.
            unsafe {
                let ctx = &*ctx.cast::<Ctx<'_, '_>>();
                match tag(node) {
                    None => false,
                    // A scalar subquery is one value when every column of this level
                    // it reads is known (its own sites are checked where it is visited).
                    Some(pg_sys::NodeTag::T_SubLink) => {
                        let link = node.cast::<pg_sys::SubLink>();
                        (*link).subLinkType != pg_sys::SubLinkType::EXPR_SUBLINK
                            || !ctx.level.reads_known((*link).subselect, ctx.known, ctx.det)
                    }
                    Some(pg_sys::NodeTag::T_Aggref | pg_sys::NodeTag::T_WindowFunc) => true,
                    Some(pg_sys::NodeTag::T_Var) => {
                        let var = node.cast::<pg_sys::Var>();
                        if (*var).varlevelsup != 0 {
                            return false;
                        }
                        if let Some(col) = ctx.level.column(node) {
                            return !(ctx.known.contains(&col) || ctx.det.contains(&col.0));
                        }
                        // An alias or group entry over an expression: its own Vars.
                        let Some(entry) = rte(ctx.level.query, index((*var).varno)) else {
                            // walk: R an unknown column leaves its relation undetermined: the site is refused
                            return true;
                        };
                        let list = match (*entry).rtekind {
                            pg_sys::RTEKind::RTE_JOIN => (*entry).joinaliasvars,
                            pg_sys::RTEKind::RTE_GROUP => (*entry).groupexprs,
                            // walk: n/a(column() resolved every other entry)
                            _ => return true,
                        };
                        let attno = index((*var).varattno);
                        let exprs = elements::<pg_sys::Node>(list);
                        let Some(&expr) = attno.checked_sub(1).and_then(|i| exprs.get(i)) else {
                            // walk: R an unknown column leaves its relation undetermined: the site is refused
                            return true;
                        };
                        walker(expr, std::ptr::from_ref(ctx).cast_mut().cast())
                    }
                    // walk: leaf(descend) its children decide
                    Some(_) => pg_sys::expression_tree_walker(
                        node,
                        Some(walker),
                        std::ptr::from_ref(ctx).cast_mut().cast(),
                    ),
                }
            }
        }
        let ctx = Ctx {
            level: self,
            known,
            det,
        };
        // SAFETY: a read-only walk of a valid expression.
        unsafe { !walker(node, std::ptr::from_ref(&ctx).cast_mut().cast()) }
    }
}

impl Checker {
    /// The unique keys of table `relid`.
    fn keys(&mut self, relid: Oid) -> &Vec<UniqueKey> {
        self.keys.entry(relid).or_insert_with(|| {
            let mut out = Vec::new();
            // SAFETY: the relation and its indexes are opened and closed here; the
            // lock is kept until the transaction ends, as for any relation a query
            // reads. The predicates are the relcache's copies.
            unsafe {
                let rel = pg_sys::relation_open(relid, pg_sys::AccessShareLock.cast_signed());
                let desc = pgrx::PgTupleDesc::from_pg_unchecked((*rel).rd_att);
                // `IS NOT NULL` of a nullable column, `None` for a NOT NULL one.
                let not_null = |attno: i16| -> Option<*mut pg_sys::Node> {
                    let a = index(attno).checked_sub(1).and_then(|i| desc.get(i))?;
                    if a.attnotnull {
                        return None;
                    }
                    let var = pg_sys::makeVar(1, attno, a.atttypid, a.atttypmod, a.attcollation, 0);
                    let mut test =
                        pgrx::PgBox::<pg_sys::NullTest>::alloc_node(pg_sys::NodeTag::T_NullTest);
                    test.arg = var.cast();
                    test.nulltesttype = pg_sys::NullTestType::IS_NOT_NULL;
                    test.argisrow = false;
                    test.location = -1;
                    Some(test.into_pg().cast())
                };
                for index_oid in oids(pg_sys::RelationGetIndexList(rel)) {
                    let irel = pg_sys::index_open(index_oid, pg_sys::AccessShareLock.cast_signed());
                    let form = (*irel).rd_index;
                    let usable = (*form).indisunique
                        && (*form).indisvalid
                        && (*form).indimmediate
                        && pg_sys::RelationGetIndexExpressions(irel).is_null();
                    if usable {
                        // walk: n/a(a key count is never negative)
                        let n = usize::try_from((*form).indnkeyatts).unwrap_or(0);
                        let columns: Vec<i16> = (*form).indkey.values.as_slice(n).to_vec();
                        let mut tests: *mut pg_sys::List = std::ptr::null_mut();
                        if !(*form).indnullsnotdistinct {
                            for test in columns.iter().filter_map(|&a| not_null(a)) {
                                tests = pg_sys::lappend(tests, test.cast());
                            }
                        }
                        out.push(UniqueKey {
                            columns,
                            predicate: pg_sys::RelationGetIndexPredicate(irel),
                            not_null: tests,
                            primary: (*form).indisprimary,
                        });
                    }
                    pg_sys::index_close(irel, pg_sys::NoLock.cast_signed());
                }
                pg_sys::relation_close(rel, pg_sys::NoLock.cast_signed());
            }
            // The primary key first: the one a hint names.
            out.sort_by_key(|k| !k.primary);
            out
        })
    }

    /// Whether knowing the columns `known` of `rti` (with the level's facts)
    /// determines its row.
    ///
    /// SAFETY: `level` is valid.
    unsafe fn determined(
        &mut self,
        level: &Level<'_>,
        rti: usize,
        known: &HashSet<Col>,
        det: &HashSet<usize>,
    ) -> bool {
        crate::utils::guard_depth();
        let katt: BTreeSet<i16> = known
            .iter()
            .filter(|(r, _)| *r == rti)
            .map(|&(_, a)| a)
            .collect();
        // SAFETY: entries and subqueries of a valid Query.
        unsafe {
            let Some(entry) = rte(level.query, rti) else {
                // walk: R an entry not found is not determined: the site is refused
                return false;
            };
            // Every column known: the rows that tie are identical, and their order
            // changes nothing (an unnest joined on its element, a table without key).
            let width = elements::<pg_sys::Node>((*(*entry).eref).colnames).len();
            if width > 0 && (1..=width).all(|a| i16::try_from(a).is_ok_and(|a| katt.contains(&a))) {
                return true;
            }
            match (*entry).rtekind {
                pg_sys::RTEKind::RTE_RELATION => {
                    let relid = (*entry).relid;
                    if pg_sys::get_rel_relkind(relid).cast_unsigned() == pg_sys::RELKIND_VIEW {
                        let Ok(query) = view_query(relid) else {
                            // walk: R a view not read is not determined: the site is refused
                            return false;
                        };
                        return self.sub_determined(query, &katt, &Vec::new());
                    }
                    let conjuncts = level.facts.of_relation(rti);
                    self.keys(relid).iter().any(|key| {
                        key.columns.iter().all(|a| katt.contains(a))
                            && (key.predicate.is_null() || implied(key.predicate, rti, &conjuncts))
                            && (key.not_null.is_null() || implied(key.not_null, rti, &conjuncts))
                    })
                }
                pg_sys::RTEKind::RTE_SUBQUERY => {
                    let mut stack = level.stack.clone();
                    stack.push(level.query);
                    self.sub_determined((*entry).subquery, &katt, &stack)
                }
                pg_sys::RTEKind::RTE_CTE => {
                    let Some((cte, stack)) = find_cte(level, entry) else {
                        // walk: R a CTE not found is not determined: the site is refused
                        return false;
                    };
                    !(*cte).cterecursive
                        && self.sub_determined((*cte).ctequery.cast(), &katt, &stack)
                }
                pg_sys::RTEKind::RTE_FUNCTION => {
                    let functions = elements::<pg_sys::RangeTblFunction>((*entry).functions);
                    let columns = elements::<pg_sys::Node>((*(*entry).eref).colnames).len();
                    if (*entry).funcordinality
                        && i16::try_from(columns).is_ok_and(|c| katt.contains(&c))
                    {
                        return true;
                    }
                    // One function returning one row: determined by its arguments.
                    if let [function] = functions.as_slice() {
                        let expr = (**function).funcexpr;
                        return tag(expr) == Some(pg_sys::NodeTag::T_FuncExpr)
                            && !(*expr.cast::<pg_sys::FuncExpr>()).funcretset
                            && level.known(expr, known, det);
                    }
                    false
                }
                pg_sys::RTEKind::RTE_RESULT => true,
                // walk: n/a(VALUES, a table function, a tuplestore: no key)
                _ => false,
            }
        }
    }

    /// Whether knowing the outputs `katt` of `query` determines its row.
    ///
    /// SAFETY: `query` is a valid Query; `stack` the levels around it.
    unsafe fn sub_determined(
        &mut self,
        query: *mut pg_sys::Query,
        katt: &BTreeSet<i16>,
        stack: &Stack,
    ) -> bool {
        crate::utils::guard_depth();
        if stack.len() > super::MAX_DEPTH {
            return false;
        }
        // SAFETY: nodes of a valid Query.
        unsafe {
            let outputs: Vec<*mut pg_sys::TargetEntry> =
                elements::<pg_sys::TargetEntry>((*query).targetList)
                    .into_iter()
                    .filter(|&te| !(*te).resjunk)
                    .collect();
            let refs_known = |clauses: *mut pg_sys::List| {
                elements::<pg_sys::SortGroupClause>(clauses)
                    .into_iter()
                    .all(|c| {
                        let te = clause_target(query, c);
                        !te.is_null() && !(*te).resjunk && katt.contains(&(*te).resno)
                    })
            };
            // At most one row, whatever it reads (a UNION ALL picked by LIMIT 1 too).
            if limit_one(query) {
                return true;
            }
            if !(*query).setOperations.is_null() {
                let op = (*query).setOperations.cast::<pg_sys::SetOperationStmt>();
                if !(*op).all {
                    return outputs.iter().all(|&te| katt.contains(&(*te).resno));
                }
                return katt
                    .iter()
                    .any(|&a| self.ranked_branches(query, a, katt, stack));
            }
            if !(*query).groupingSets.is_null() {
                return false;
            }
            // Groups or DISTINCT rows (either fixed names one row): determined when the
            // known outputs fix every key, there or through the rows they determine.
            let clauses = [(*query).distinctClause, (*query).groupClause];
            if clauses.iter().any(|c| !c.is_null()) {
                return clauses
                    .into_iter()
                    .filter(|c| !c.is_null())
                    .any(|c| refs_known(c) || self.fix_keys(query, &outputs, katt, c, stack));
            }
            // A set-returning output known: the rows sharing every output are
            // identical, so their order changes nothing a site reads.
            if (*query).hasTargetSRFs
                && outputs.iter().any(|&te| {
                    pg_sys::expression_returns_set((*te).expr.cast())
                        && !katt.contains(&(*te).resno)
                })
            {
                return false;
            }
            if (*query).hasAggs {
                return true;
            }
            let level = Level {
                query,
                stack,
                facts: facts(query),
            };
            let mut known = HashSet::new();
            for &te in &outputs {
                if katt.contains(&(*te).resno)
                    && let Some(col) = level.column((*te).expr.cast())
                {
                    known.insert(col);
                }
            }
            let (_, det) = self.closure(&level, known, HashSet::new());
            leaves((*query).jointree.cast())
                .iter()
                .all(|r| det.contains(r))
        }
    }

    /// The outputs of relation `rti` of `level` (a subquery, CTE or view it does not
    /// determine) that its known outputs fix: one value in every row they reach,
    /// however many rows that is. Not a set-returning output, and not of a relation
    /// reading this level (a LATERAL one's values are those of rows of this level).
    ///
    /// SAFETY: `level` is valid.
    unsafe fn fixed_outputs(
        &mut self,
        level: &Level<'_>,
        rti: usize,
        known: &HashSet<Col>,
    ) -> Vec<i16> {
        crate::utils::guard_depth();
        let katt: BTreeSet<i16> = known
            .iter()
            .filter(|(r, _)| *r == rti)
            .map(|&(_, a)| a)
            .collect();
        if katt.is_empty() || level.stack.len() > super::MAX_DEPTH {
            return Vec::new();
        }
        // SAFETY: entries and subqueries of a valid Query.
        unsafe {
            let Some(entry) = rte(level.query, rti) else {
                // walk: R an entry not found fixes nothing: its columns stay unknown
                return Vec::new();
            };
            let (query, stack) = match (*entry).rtekind {
                pg_sys::RTEKind::RTE_SUBQUERY => {
                    let mut stack = level.stack.clone();
                    stack.push(level.query);
                    ((*entry).subquery, stack)
                }
                pg_sys::RTEKind::RTE_CTE => match find_cte(level, entry) {
                    Some((cte, stack)) if !(*cte).cterecursive => ((*cte).ctequery.cast(), stack),
                    // walk: n/a(a recursive or unknown CTE fixes nothing)
                    _ => return Vec::new(),
                },
                pg_sys::RTEKind::RTE_RELATION
                    if pg_sys::get_rel_relkind((*entry).relid).cast_unsigned()
                        == pg_sys::RELKIND_VIEW =>
                {
                    match view_query((*entry).relid) {
                        Ok(query) => (query, Vec::new()),
                        // walk: R a view not read fixes nothing: its columns stay unknown
                        Err(_) => return Vec::new(),
                    }
                }
                // walk: n/a(a table, function or VALUES list: determined or not, no outputs to fix)
                _ => return Vec::new(),
            };
            if query.is_null()
                || pg_sys::contain_vars_of_level(query.cast(), 1)
                || !(*query).setOperations.is_null()
                || !(*query).groupClause.is_null()
                || !(*query).groupingSets.is_null()
                || !(*query).distinctClause.is_null()
                || (*query).hasAggs
            {
                return Vec::new();
            }
            let inner = Level {
                query,
                stack: &stack,
                facts: facts(query),
            };
            let outputs: Vec<*mut pg_sys::TargetEntry> =
                elements::<pg_sys::TargetEntry>((*query).targetList)
                    .into_iter()
                    .filter(|&te| !(*te).resjunk)
                    .collect();
            let mut start = HashSet::new();
            for &te in &outputs {
                if katt.contains(&(*te).resno)
                    && let Some(col) = inner.column((*te).expr.cast())
                {
                    start.insert(col);
                }
            }
            let (inner_known, inner_det) = self.closure(&inner, start, HashSet::new());
            outputs
                .into_iter()
                .filter(|&te| {
                    !katt.contains(&(*te).resno)
                        && !pg_sys::expression_returns_set((*te).expr.cast())
                        && inner.known((*te).expr.cast(), &inner_known, &inner_det)
                })
                .map(|te| (*te).resno)
                .collect()
        }
    }

    /// Whether the outputs `katt` of `query` fix every expression of `clauses` (its
    /// GROUP BY or DISTINCT): then they name one group.
    ///
    /// SAFETY: `query` is a valid Query; `stack` the levels around it.
    unsafe fn fix_keys(
        &mut self,
        query: *mut pg_sys::Query,
        outputs: &[*mut pg_sys::TargetEntry],
        katt: &BTreeSet<i16>,
        clauses: *mut pg_sys::List,
        stack: &Stack,
    ) -> bool {
        // SAFETY: nodes of a valid Query.
        unsafe {
            let level = Level {
                query,
                stack,
                facts: facts(query),
            };
            let mut known = HashSet::new();
            for &te in outputs {
                if katt.contains(&(*te).resno)
                    && let Some(col) = level.column((*te).expr.cast())
                {
                    known.insert(col);
                }
            }
            let (known, det) = self.closure(&level, known, HashSet::new());
            clause_exprs(query, clauses)
                .into_iter()
                .all(|e| level.known(e, &known, &det))
        }
    }

    /// Rule 4: `query` is a `UNION ALL` whose branches hold different constants in
    /// output `attno`, each branch determined by its outputs `katt` (empty: at most
    /// one row each). The rank tells the branches apart; within a branch, `katt`
    /// tells the rows apart.
    ///
    /// SAFETY: `query` is a valid Query with a set operation; `stack` the levels
    /// around it.
    unsafe fn ranked_branches(
        &mut self,
        query: *mut pg_sys::Query,
        attno: i16,
        katt: &BTreeSet<i16>,
        stack: &Stack,
    ) -> bool {
        // SAFETY: nodes of a valid Query.
        unsafe {
            if !all_union_all((*query).setOperations) {
                return false;
            }
            let mut inner = stack.clone();
            inner.push(query);
            let mut ranks: Vec<*mut pg_sys::Const> = Vec::new();
            for rti in setop_leaves((*query).setOperations) {
                let Some(entry) = rte(query, rti) else {
                    // walk: R a branch not found ranks nothing: the site is refused
                    return false;
                };
                let branch = (*entry).subquery;
                if branch.is_null() || !self.sub_determined(branch, katt, &inner) {
                    return false;
                }
                let Some(&te) = elements::<pg_sys::TargetEntry>((*branch).targetList)
                    .get(index(attno).wrapping_sub(1))
                else {
                    // walk: R a branch without the rank column ranks nothing: the site is refused
                    return false;
                };
                let value = rank_value(branch, (*te).expr.cast(), &inner);
                if tag(value) != Some(pg_sys::NodeTag::T_Const)
                    || (*value.cast::<pg_sys::Const>()).constisnull
                {
                    return false;
                }
                let value = value.cast::<pg_sys::Const>();
                if ranks.iter().any(|&other| {
                    (*other).consttype == (*value).consttype
                        && crate::utils::datum_is_equal(
                            (*other).constvalue,
                            (*value).constvalue,
                            (*value).constbyval,
                            (*value).constlen,
                        )
                }) {
                    return false;
                }
                ranks.push(value);
            }
            ranks.len() > 1
        }
    }

    /// The columns known and the relations determined, from `known`, to a fixpoint.
    ///
    /// SAFETY: `level` is valid.
    unsafe fn closure(
        &mut self,
        level: &Level<'_>,
        mut known: HashSet<Col>,
        mut det: HashSet<usize>,
    ) -> (HashSet<Col>, HashSet<usize>) {
        // SAFETY: nodes of a valid Query.
        unsafe {
            let relations = leaves((*level.query).jointree.cast());
            loop {
                let mut changed = false;
                for (a, b, allowed) in &level.facts.equalities {
                    for (x, y) in [(*a, *b), (*b, *a)] {
                        if let Some(col) = level.column(x)
                            && !known.contains(&col)
                            && !det.contains(&col.0)
                            && allowed.as_ref().is_none_or(|s| s.contains(&col.0))
                            && level.known(y, &known, &det)
                        {
                            known.insert(col);
                            changed = true;
                        }
                    }
                }
                for &r in &relations {
                    if det.contains(&r) {
                        continue;
                    }
                    if self.determined(level, r, &known, &det) {
                        det.insert(r);
                        changed = true;
                        continue;
                    }
                    for attno in self.fixed_outputs(level, r, &known) {
                        changed |= known.insert((r, attno));
                    }
                }
                if !changed {
                    return (known, det);
                }
            }
        }
    }

    /// The relations of `level` that `given` and `sort` leave undetermined (rule 1),
    /// or that `given` alone does (rule 2), and the expressions of `target` (a
    /// grouped level's groups) left unknown; empty when the site is accepted.
    ///
    /// SAFETY: `level` is valid; the expressions are of it.
    unsafe fn undetermined(
        &mut self,
        level: &Level<'_>,
        sort: &[*mut pg_sys::Node],
        given: &[*mut pg_sys::Node],
        target: Option<&[*mut pg_sys::Node]>,
    ) -> Vec<String> {
        // SAFETY: nodes of a valid Query.
        unsafe {
            let columns = |exprs: &[*mut pg_sys::Node]| -> HashSet<Col> {
                exprs.iter().filter_map(|&e| level.column(e)).collect()
            };
            let mut with_sort = columns(given);
            with_sort.extend(columns(&usable_sort_keys(sort)));
            // The first attempt's refusal is the one reported: it names what the
            // sort keys leave undetermined.
            let mut first: Option<Vec<String>> = None;
            for start in [with_sort, columns(given)] {
                let (known, det) = self.closure(level, start.clone(), HashSet::new());
                let last: Vec<String> = match target {
                    // Each relation still undetermined once the ones named before it
                    // are: the keys to append, and no more.
                    None => {
                        let relations = leaves((*level.query).jointree.cast());
                        let (mut det, mut named, mut forced) = (det, Vec::new(), HashSet::new());
                        while let Some(&r) = relations.iter().find(|r| !det.contains(r)) {
                            named.push(self.label(level, r));
                            forced.insert(r);
                            det = self.closure(level, start.clone(), forced.clone()).1;
                        }
                        named
                    }
                    Some(exprs) => {
                        let open = exprs
                            .iter()
                            .filter(|&&e| !level.known(e, &known, &det))
                            .count();
                        if open == 0 {
                            Vec::new()
                        } else {
                            vec![format!(
                                "its groups ({open} of its {} grouping or DISTINCT keys \
                                 neither ordered by nor fixed by what is)",
                                exprs.len()
                            )]
                        }
                    }
                };
                if last.is_empty() {
                    return last;
                }
                first.get_or_insert(last);
            }
            // walk: n/a(no attempt left undetermined relations: the site is accepted above)
            first.unwrap_or_default()
        }
    }

    /// How relation `rti` of `level` is named in a refusal, with a key to append.
    fn label(&mut self, level: &Level<'_>, rti: usize) -> String {
        // SAFETY: entries of a valid Query; catalog lookups by OID.
        unsafe {
            let Some(entry) = rte(level.query, rti) else {
                // walk: n/a(the label of a refusal already decided)
                return String::from("a relation");
            };
            let alias = cstr((*(*entry).eref).aliasname);
            if (*entry).rtekind != pg_sys::RTEKind::RTE_RELATION {
                return format!("{alias} (a subquery, CTE or function without a key)");
            }
            let relid = (*entry).relid;
            let name = relation_name(relid);
            if pg_sys::get_rel_relkind(relid).cast_unsigned() == pg_sys::RELKIND_VIEW {
                return format!(
                    "view {name} {alias} (none of its outputs read here keys its rows)"
                );
            }
            let columns = elements::<pg_sys::Node>((*(*entry).eref).colnames);
            let key = self
                .keys(relid)
                .iter()
                .find(|k| k.predicate.is_null())
                .map(|k| {
                    k.columns
                        .iter()
                        .map(|&a| {
                            let column = index(a)
                                .checked_sub(1)
                                .and_then(|i| columns.get(i))
                                .map_or_else(String::new, |&c| {
                                    cstr((*c.cast::<pg_sys::String>()).sval)
                                });
                            format!("{alias}.{column}")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            match key {
                Some(key) => format!("{name} {alias} (append {key})"),
                None => format!("{name} {alias} (it has no unique key on NOT NULL columns)"),
            }
        }
    }

    /// Check every site of `query`, then of every level under it.
    ///
    /// SAFETY: `query` is a valid Query; `stack` the levels around it.
    unsafe fn visit(&mut self, query: *mut pg_sys::Query, array: bool, stack: &mut Stack) {
        crate::utils::guard_depth();
        if self.refused.is_some() || stack.len() > super::MAX_DEPTH {
            return;
        }
        // SAFETY: nodes of a valid Query.
        unsafe {
            for entry in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                if (*entry).rtekind == pg_sys::RTEKind::RTE_RELATION
                    && pg_sys::get_rel_relkind((*entry).relid).cast_unsigned()
                        == pg_sys::RELKIND_VIEW
                {
                    self.views.push((*entry).relid);
                }
            }
            if let Some(reason) = self.sites(query, array, stack) {
                self.refused = Some(reason);
                return;
            }
            stack.push(query);
            let mut inner: Vec<(*mut pg_sys::Query, bool)> = Vec::new();
            for entry in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                if (*entry).rtekind == pg_sys::RTEKind::RTE_SUBQUERY {
                    inner.push(((*entry).subquery, false));
                }
            }
            for cte in elements::<pg_sys::CommonTableExpr>((*query).cteList) {
                inner.push(((*cte).ctequery.cast(), false));
            }
            inner.extend(sublinks(query));
            for (q, array) in inner {
                self.visit(q, array, stack);
            }
            stack.pop();
        }
    }

    /// The refusal of the first site of `query` whose order is not total; `array`
    /// when `query` is an `ARRAY(SELECT …)`, whose rows become elements in order.
    ///
    /// SAFETY: `query` is a valid Query; `stack` the levels around it.
    unsafe fn sites(
        &mut self,
        query: *mut pg_sys::Query,
        array: bool,
        stack: &Stack,
    ) -> Option<String> {
        // SAFETY: nodes of a valid Query.
        unsafe {
            let level = Level {
                query,
                stack,
                facts: facts(query),
            };
            let grouped = !(*query).groupClause.is_null();
            let groups = if grouped {
                clause_exprs(query, (*query).groupClause)
            } else {
                Vec::new()
            };
            let group_target = grouped.then_some(groups.as_slice());
            let place = self.place();
            let (aggregates, windows) = level_functions(query);
            for agg in aggregates {
                if let Some(reason) = self.aggregate_site(&level, agg, &groups, &place) {
                    return Some(reason);
                }
            }
            for win in windows {
                let Some(name) = sensitive_window(win) else {
                    // walk: n/a(an order-insensitive window function is not a site)
                    continue;
                };
                let Some(clause) = elements::<pg_sys::WindowClause>((*query).windowClause)
                    .into_iter()
                    .find(|&w| (*w).winref == (*win).winref)
                else {
                    // walk: n/a(a window function names a clause of its own level)
                    continue;
                };
                let partition = clause_exprs(query, (*clause).partitionClause);
                let order = clause_exprs(query, (*clause).orderClause);
                let undetermined = self.undetermined(&level, &order, &partition, group_target);
                if !undetermined.is_empty() {
                    return Some(format!(
                        "{name}() in {place} numbers or picks rows in an order nothing fixes: give \
                         its window an ORDER BY that determines every row: {}",
                        undetermined.join("; ")
                    ));
                }
            }
            let sort = clause_exprs(query, (*query).sortClause);
            if (*query).hasDistinctOn {
                let mut keys = clause_exprs(query, (*query).distinctClause);
                keys.extend(sort.iter().copied());
                let undetermined = self.undetermined(&level, &keys, &[], group_target);
                if !undetermined.is_empty() && !self.ties_alike(&level, &keys, &[], &outputs(query))
                {
                    return Some(format!(
                        "DISTINCT ON in {place} picks one row of several in an order nothing fixes: \
                         append to its ORDER BY what determines every row: {}",
                        undetermined.join("; ")
                    ));
                }
            }
            if limits(query) || array {
                let what = if array { "ARRAY(SELECT …)" } else { "LIMIT" };
                return self.limit_site(&level, &groups, &sort, &place, what);
            }
            None
        }
    }

    /// Whether the rows `sort` leaves tied (with `given`) are identical in `read`:
    /// each expression of `read` is known, through columns of types whose equal
    /// values are the same value. The site's result is then the same in any order.
    ///
    /// SAFETY: `level` is valid; the expressions are its own.
    unsafe fn ties_alike(
        &mut self,
        level: &Level<'_>,
        sort: &[*mut pg_sys::Node],
        given: &[*mut pg_sys::Node],
        read: &[*mut pg_sys::Node],
    ) -> bool {
        // SAFETY: expressions of a valid Query.
        unsafe {
            let flags = pg_sys::PVC_INCLUDE_AGGREGATES
                | pg_sys::PVC_INCLUDE_WINDOWFUNCS
                | pg_sys::PVC_INCLUDE_PLACEHOLDERS;
            let identical = read.iter().all(|&e| {
                elements::<pg_sys::Node>(pg_sys::pull_var_clause(e, flags.cast_signed()))
                    .into_iter()
                    .all(|v| {
                        tag(v) == Some(pg_sys::NodeTag::T_Var) && {
                            let var = v.cast::<pg_sys::Var>();
                            equal_is_identical(
                                pg_sys::getBaseType((*var).vartype),
                                (*var).varcollid,
                            )
                        }
                    })
            });
            identical && self.undetermined(level, sort, given, Some(read)).is_empty()
        }
    }

    /// The refusal of aggregate `agg` of `level`, `None` when its order is total.
    ///
    /// SAFETY: `agg` is a valid `Aggref` of `level`.
    unsafe fn aggregate_site(
        &mut self,
        level: &Level<'_>,
        agg: *mut pg_sys::Aggref,
        groups: &[*mut pg_sys::Node],
        place: &str,
    ) -> Option<String> {
        // SAFETY: nodes of a valid Aggref.
        unsafe {
            let name = sensitive_aggregate((*agg).aggfnoid, &agg_arg_types(agg))?;
            if (*agg).agglevelsup != 0 {
                // An aggregate over the rows of an outer level, written in a
                // sublink: rare; refused when it is order-sensitive.
                return Some(format!(
                    "{name} in {place} aggregates the rows of an outer query level: move it \
                     into that level and give it an ORDER BY on a key"
                ));
            }
            // An ordered-set or hypothetical-set aggregate orders by its WITHIN GROUP.
            if (*agg).aggkind != b'n'.cast_signed() {
                return None;
            }
            let args = elements::<pg_sys::TargetEntry>((*agg).args);
            let mut keys = Vec::new();
            for clause in elements::<pg_sys::SortGroupClause>((*agg).aggorder)
                .into_iter()
                .chain(elements::<pg_sys::SortGroupClause>((*agg).aggdistinct))
            {
                if let Some(&te) = args
                    .iter()
                    .find(|&&te| (*te).ressortgroupref == (*clause).tleSortGroupRef)
                {
                    keys.push((*te).expr.cast::<pg_sys::Node>());
                }
            }
            let undetermined = self.undetermined(level, &keys, groups, None);
            if undetermined.is_empty() {
                return None;
            }
            // The rows that tie may differ, but not in what the aggregate reads: its
            // input is the same in any order.
            let mut read: Vec<*mut pg_sys::Node> =
                args.iter().map(|&te| (*te).expr.cast()).collect();
            if !(*agg).aggfilter.is_null() {
                read.push((*agg).aggfilter.cast());
            }
            if self.ties_alike(level, &keys, groups, &read) {
                return None;
            }
            match own_order(agg) {
                OwnOrder::Renders => None,
                OwnOrder::Ties(types) => Some(format!(
                    "{name}() in {place} orders its values by themselves, but equal {types} \
                     values render differently (1.0 and 1.00): order it by a key instead, \
                     e.g. {}",
                    undetermined.join("; ")
                )),
                OwnOrder::No => Some(format!(
                    "{name}() in {place} aggregates rows in an order nothing fixes, so the stored \
                     document is one of several the view may return: give it an ORDER BY that \
                     determines every row: {}",
                    undetermined.join("; ")
                )),
            }
        }
    }

    /// The refusal of the `LIMIT` (or `OFFSET`) of `level`, `None` when its order
    /// is total.
    ///
    /// SAFETY: `level` is valid; `groups` and `sort` are its expressions.
    unsafe fn limit_site(
        &mut self,
        level: &Level<'_>,
        groups: &[*mut pg_sys::Node],
        sort: &[*mut pg_sys::Node],
        place: &str,
        what: &str,
    ) -> Option<String> {
        let query = level.query;
        // SAFETY: nodes of a valid Query.
        unsafe {
            let undetermined = if !(*query).setOperations.is_null() {
                let first = elements::<pg_sys::SortGroupClause>((*query).sortClause)
                    .first()
                    .map(|&c| clause_target(query, c))
                    .filter(|te| !te.is_null())
                    .map(|te| (*te).resno);
                if first
                    .is_some_and(|a| self.ranked_branches(query, a, &BTreeSet::new(), level.stack))
                {
                    Vec::new()
                } else {
                    vec![
                        "its UNION ALL branches (select a different constant rank in each, \
                          one row per branch, and order by it)"
                            .to_string(),
                    ]
                }
            } else if (*query).hasAggs && groups.is_empty() {
                Vec::new()
            } else if !groups.is_empty() {
                self.undetermined(level, sort, &[], Some(groups))
            } else if !(*query).distinctClause.is_null() && !(*query).hasDistinctOn {
                let distinct = clause_exprs(query, (*query).distinctClause);
                self.undetermined(level, sort, &[], Some(&distinct))
            } else {
                let rows = self.undetermined(level, sort, &[], None);
                if rows.is_empty() || self.ties_alike(level, sort, &[], &outputs(query)) {
                    Vec::new()
                } else {
                    rows
                }
            };
            (!undetermined.is_empty()).then(|| {
                format!(
                    "{what} in {place} keeps or lists rows in an order nothing fixes: give \
                     it an ORDER BY that determines every row: {}",
                    undetermined.join("; ")
                )
            })
        }
    }

    fn place(&self) -> String {
        self.view_name
            .as_ref()
            .map_or_else(|| "the definition".to_string(), |v| format!("view {v}"))
    }
}

/// Whether `query` keeps a subset of its rows: a `LIMIT`, or an `OFFSET` other than
/// a constant 0 (an `OFFSET 0` fence drops none).
///
/// SAFETY: `query` is a valid Query.
unsafe fn limits(query: *mut pg_sys::Query) -> bool {
    // SAFETY: nodes of a valid Query.
    unsafe {
        let count = strip((*query).limitCount);
        let count_all = count.is_null()
            || (tag(count) == Some(pg_sys::NodeTag::T_Const)
                && (*count.cast::<pg_sys::Const>()).constisnull);
        let offset = strip((*query).limitOffset);
        let no_offset = offset.is_null() || const_int(offset) == Some(0);
        !(count_all && no_offset)
    }
}

/// Whether `query` returns at most one row by its `LIMIT 1`.
///
/// SAFETY: `query` is a valid Query.
unsafe fn limit_one(query: *mut pg_sys::Query) -> bool {
    // SAFETY: nodes of a valid Query.
    unsafe { const_int(strip((*query).limitCount)).is_some_and(|n| n <= 1) }
}

/// A branch's rank: its output `expr`, followed through the subqueries and CTEs
/// whose column it is (a rank written in a CTE the branch reads), stripped.
///
/// SAFETY: `query` is a valid Query, `expr` one of its expressions, `stack` the
/// levels around it.
unsafe fn rank_value(
    mut query: *mut pg_sys::Query,
    expr: *mut pg_sys::Node,
    stack: &Stack,
) -> *mut pg_sys::Node {
    // SAFETY: nodes of valid Queries.
    unsafe {
        let mut stack = stack.clone();
        let mut node = strip(expr);
        for _ in 0..super::MAX_DEPTH {
            if tag(node) != Some(pg_sys::NodeTag::T_Var)
                || (*node.cast::<pg_sys::Var>()).varlevelsup != 0
            {
                break;
            }
            let var = node.cast::<pg_sys::Var>();
            let Some(entry) = rte(query, index((*var).varno)) else {
                // walk: n/a(a column of no entry: no rank to follow)
                break;
            };
            let (sub, sub_stack) = match (*entry).rtekind {
                pg_sys::RTEKind::RTE_SUBQUERY => {
                    let mut inner = stack.clone();
                    inner.push(query);
                    ((*entry).subquery, inner)
                }
                pg_sys::RTEKind::RTE_CTE => {
                    let level = Level {
                        query,
                        stack: &stack,
                        facts: facts(query),
                    };
                    match find_cte(&level, entry) {
                        Some((cte, cte_stack)) if !(*cte).cterecursive => {
                            ((*cte).ctequery.cast(), cte_stack)
                        }
                        // walk: n/a(a recursive or unknown CTE: no rank to follow)
                        _ => break,
                    }
                }
                // walk: n/a(a table, function or join column: no rank to follow)
                _ => break,
            };
            if sub.is_null() || !(*sub).setOperations.is_null() {
                break;
            }
            let Some(&te) = elements::<pg_sys::TargetEntry>((*sub).targetList)
                .get(index((*var).varattno).wrapping_sub(1))
            else {
                // walk: n/a(a column beyond the target list: no rank to follow)
                break;
            };
            query = sub;
            stack = sub_stack;
            node = strip((*te).expr.cast());
        }
        node
    }
}

/// The value of an integer constant (through a cast), `None` for anything else.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn const_int(node: *mut pg_sys::Node) -> Option<i64> {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_Const) => {
                let c = node.cast::<pg_sys::Const>();
                if (*c).constisnull {
                    return None;
                }
                match (*c).consttype {
                    // A row count is not negative: its datum's bits are its value.
                    pg_sys::INT8OID | pg_sys::INT4OID | pg_sys::INT2OID => {
                        i64::try_from((*c).constvalue.value()).ok()
                    }
                    // walk: n/a(another constant is no row count)
                    _ => None,
                }
            }
            Some(pg_sys::NodeTag::T_FuncExpr) => {
                match elements::<pg_sys::Node>((*node.cast::<pg_sys::FuncExpr>()).args).as_slice() {
                    [arg] => const_int(*arg),
                    // walk: n/a(a function of several arguments is no constant)
                    _ => None,
                }
            }
            // walk: n/a(anything else is no constant)
            _ => None,
        }
    }
}

/// Whether every set operation of a tree is `UNION ALL`.
///
/// SAFETY: `node` is null or a valid set-operation tree.
unsafe fn all_union_all(node: *mut pg_sys::Node) -> bool {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_SetOperationStmt) => {
                let op = node.cast::<pg_sys::SetOperationStmt>();
                (*op).op == pg_sys::SetOperation::SETOP_UNION
                    && (*op).all
                    && all_union_all((*op).larg)
                    && all_union_all((*op).rarg)
            }
            Some(pg_sys::NodeTag::T_RangeTblRef) => true,
            // walk: n/a(a set-operation tree holds only these)
            _ => false,
        }
    }
}

/// The range-table indexes of a set-operation tree's branches.
///
/// SAFETY: `node` is null or a valid set-operation tree.
unsafe fn setop_leaves(node: *mut pg_sys::Node) -> Vec<usize> {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_SetOperationStmt) => {
                let op = node.cast::<pg_sys::SetOperationStmt>();
                let mut out = setop_leaves((*op).larg);
                out.extend(setop_leaves((*op).rarg));
                out
            }
            Some(pg_sys::NodeTag::T_RangeTblRef) => {
                vec![index((*node.cast::<pg_sys::RangeTblRef>()).rtindex)]
            }
            // walk: n/a(a set-operation tree holds only these)
            _ => Vec::new(),
        }
    }
}

/// The CTE an entry reads and the levels around it.
///
/// SAFETY: `entry` is a CTE entry of `level`.
unsafe fn find_cte(
    level: &Level<'_>,
    entry: *mut pg_sys::RangeTblEntry,
) -> Option<(*mut pg_sys::CommonTableExpr, Stack)> {
    // SAFETY: nodes of valid Queries.
    unsafe {
        let up = index((*entry).ctelevelsup);
        let mut levels: Stack = level.stack.clone();
        levels.push(level.query);
        let at = levels.len().checked_sub(up + 1)?;
        let owner = levels[at];
        let name = cstr((*entry).ctename);
        let cte = elements::<pg_sys::CommonTableExpr>((*owner).cteList)
            .into_iter()
            .find(|&c| cstr((*c).ctename) == name)?;
        levels.truncate(at + 1);
        Some((cte, levels))
    }
}

/// Whether a partial index's `predicate` (on `varno` 1) holds for every row of
/// relation `rti` that the level's `conjuncts` keep.
///
/// SAFETY: `predicate` is a valid implicit-AND list.
unsafe fn implied(
    predicate: *mut pg_sys::List,
    rti: usize,
    conjuncts: &[*mut pg_sys::Node],
) -> bool {
    #[pg_guard]
    unsafe extern "C-unwind" fn renumber(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `node` is valid, of the copy made below; `ctx` the new varno.
        unsafe {
            if tag(node) == Some(pg_sys::NodeTag::T_Var) {
                let var = node.cast::<pg_sys::Var>();
                let varno = *ctx.cast::<i32>();
                (*var).varno = varno;
                (*var).varnosyn = varno.cast_unsigned();
                return false;
            }
            pg_sys::expression_tree_walker(node, Some(renumber), ctx)
        }
    }
    let Ok(mut varno) = i32::try_from(rti) else {
        // walk: R a range-table index beyond i32 implies nothing: the site is refused
        return false;
    };
    // SAFETY: the predicate is copied before its Vars are renumbered; the clause
    // list is built in the current memory context.
    unsafe {
        let copy = pg_sys::copyObjectImpl(predicate.cast()).cast::<pg_sys::Node>();
        renumber(copy, std::ptr::from_mut(&mut varno).cast());
        let mut clauses: *mut pg_sys::List = std::ptr::null_mut();
        for &c in conjuncts {
            clauses = pg_sys::lappend(clauses, c.cast());
        }
        !clauses.is_null() && pg_sys::predicate_implied_by(copy.cast(), clauses, false)
    }
}

/// The sort keys that are a key's values: those of a deterministic collation (with
/// a nondeterministic one, two keys a unique index tells apart may tie).
///
/// SAFETY: the expressions are valid.
unsafe fn usable_sort_keys(sort: &[*mut pg_sys::Node]) -> Vec<*mut pg_sys::Node> {
    // SAFETY: valid expressions.
    unsafe {
        sort.iter()
            .copied()
            .filter(|&e| {
                let collation = pg_sys::exprCollation(e);
                collation == pg_sys::InvalidOid || pg_sys::get_collation_isdeterministic(collation)
            })
            .collect()
    }
}

/// The aggregates and window functions of `query`'s own level: not those of the
/// sublinks under it.
///
/// SAFETY: `query` is a valid Query.
unsafe fn level_functions(
    query: *mut pg_sys::Query,
) -> (Vec<*mut pg_sys::Aggref>, Vec<*mut pg_sys::WindowFunc>) {
    struct Found {
        aggregates: Vec<*mut pg_sys::Aggref>,
        windows: Vec<*mut pg_sys::WindowFunc>,
    }
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Found below; `node` is valid.
        unsafe {
            let found = &mut *ctx.cast::<Found>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_Aggref) => {
                    found.aggregates.push(node.cast());
                    false
                }
                Some(pg_sys::NodeTag::T_WindowFunc) => {
                    found.windows.push(node.cast());
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                // A sublink's query is a level of its own; an aggregate of this
                // level written in it is found there (agglevelsup).
                Some(pg_sys::NodeTag::T_SubLink) => {
                    walker((*node.cast::<pg_sys::SubLink>()).testexpr, ctx)
                }
                // walk: leaf(descend) its children are of the same level
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    let mut found = Found {
        aggregates: Vec::new(),
        windows: Vec::new(),
    };
    // SAFETY: a read-only walk of a valid Query's own expressions.
    unsafe {
        let ctx = std::ptr::from_mut(&mut found).cast();
        walker((*query).targetList.cast(), ctx);
        walker((*query).havingQual, ctx);
    }
    (found.aggregates, found.windows)
}

/// The queries of the sublinks of `query`'s own level, and whether each is an
/// `ARRAY(SELECT …)`.
///
/// SAFETY: `query` is a valid Query.
unsafe fn sublinks(query: *mut pg_sys::Query) -> Vec<(*mut pg_sys::Query, bool)> {
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec below; `node` is valid.
        unsafe {
            let out = &mut *ctx.cast::<Vec<(*mut pg_sys::Query, bool)>>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_SubLink) => {
                    let link = node.cast::<pg_sys::SubLink>();
                    out.push((
                        (*link).subselect.cast(),
                        (*link).subLinkType == pg_sys::SubLinkType::ARRAY_SUBLINK,
                    ));
                    walker((*link).testexpr, ctx)
                }
                // A subquery or CTE entry is visited as a level of its own.
                Some(pg_sys::NodeTag::T_Query) => false,
                // walk: leaf(descend) its children are of the same level
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    let mut out = Vec::new();
    // SAFETY: a read-only walk of a valid Query; subqueries and CTEs are skipped.
    unsafe {
        let flags = pg_sys::QTW_IGNORE_RT_SUBQUERIES
            | pg_sys::QTW_IGNORE_CTE_SUBQUERIES
            | pg_sys::QTW_IGNORE_JOINALIASES
            | pg_sys::QTW_IGNORE_GROUPEXPRS;
        pg_sys::query_tree_walker(
            query,
            Some(walker),
            std::ptr::from_mut(&mut out).cast(),
            flags.cast_signed(),
        );
    }
    out
}

/// The types of an aggregate's arguments.
///
/// SAFETY: `agg` is a valid `Aggref`.
unsafe fn agg_arg_types(agg: *mut pg_sys::Aggref) -> Vec<Oid> {
    // SAFETY: nodes of a valid Aggref.
    unsafe {
        elements::<pg_sys::TargetEntry>((*agg).args)
            .into_iter()
            .filter(|&te| !(*te).resjunk)
            .map(|te| pg_sys::exprType((*te).expr.cast()))
            .collect()
    }
}

/// The name of an aggregate whose result depends on the order of its rows, `None`
/// for one that does not.
fn sensitive_aggregate(funcid: Oid, arg_types: &[Oid]) -> Option<String> {
    // SAFETY: catalog lookups by OID.
    let (schema, name) = unsafe {
        (
            cstr(pg_sys::get_namespace_name(pg_sys::get_func_namespace(
                funcid,
            ))),
            cstr(pg_sys::get_func_name(funcid)),
        )
    };
    if schema != "pg_catalog" {
        return Some(format!("{schema}.{name}"));
    }
    let sensitive = matches!(
        name.as_str(),
        "array_agg"
            | "jsonb_agg"
            | "json_agg"
            | "jsonb_agg_strict"
            | "json_agg_strict"
            | "string_agg"
            | "xmlagg"
            | "jsonb_object_agg"
            | "json_object_agg"
            | "jsonb_object_agg_strict"
            | "json_object_agg_strict"
            | "jsonb_object_agg_unique"
            | "json_object_agg_unique"
            | "jsonb_object_agg_unique_strict"
            | "json_object_agg_unique_strict"
    ) || (matches!(name.as_str(), "sum" | "avg")
        // SAFETY: a catalog lookup by OID.
        && arg_types.first().is_some_and(|&t| {
            // SAFETY: a catalog lookup by OID.
            let base = unsafe { pg_sys::getBaseType(t) };
            base == pg_sys::FLOAT4OID || base == pg_sys::FLOAT8OID
        }));
    sensitive.then(|| {
        if matches!(name.as_str(), "sum" | "avg") {
            format!("{name} of a float")
        } else {
            name
        }
    })
}

/// The name of a window function whose result depends on the order of its rows.
///
/// SAFETY: `win` is a valid `WindowFunc`.
unsafe fn sensitive_window(win: *mut pg_sys::WindowFunc) -> Option<String> {
    // SAFETY: a valid node; catalog lookups by OID.
    unsafe {
        let funcid = (*win).winfnoid;
        if (*win).winagg {
            let types: Vec<Oid> = elements::<pg_sys::Node>((*win).args)
                .into_iter()
                .map(|a| pg_sys::exprType(a))
                .collect();
            return sensitive_aggregate(funcid, &types);
        }
        let schema = cstr(pg_sys::get_namespace_name(pg_sys::get_func_namespace(
            funcid,
        )));
        let name = cstr(pg_sys::get_func_name(funcid));
        if schema != "pg_catalog" {
            return Some(format!("{schema}.{name}"));
        }
        matches!(
            name.as_str(),
            "row_number" | "ntile" | "first_value" | "last_value" | "nth_value" | "lag" | "lead"
        )
        .then_some(name)
    }
}

/// Whether an aggregate is ordered by its own values (rule 3).
enum OwnOrder {
    /// Ordered by (or `DISTINCT` on) every non-constant argument, each of a type
    /// that renders equal values identically.
    Renders,
    /// Ordered so, but equal values of these types may render differently.
    Ties(String),
    /// Not ordered by its own values.
    No,
}

/// SAFETY: `agg` is a valid `Aggref`.
unsafe fn own_order(agg: *mut pg_sys::Aggref) -> OwnOrder {
    // SAFETY: nodes of a valid Aggref; catalog lookups by OID.
    unsafe {
        let ordered: HashSet<u32> = elements::<pg_sys::SortGroupClause>((*agg).aggorder)
            .into_iter()
            .map(|c| (*c).tleSortGroupRef)
            .collect();
        let distinct = !(*agg).aggdistinct.is_null();
        let mut ties = Vec::new();
        let mut any = false;
        for te in elements::<pg_sys::TargetEntry>((*agg).args) {
            if (*te).resjunk {
                continue;
            }
            let expr = strip((*te).expr.cast());
            let constant = matches!(
                tag(expr),
                Some(pg_sys::NodeTag::T_Const | pg_sys::NodeTag::T_Param)
            ) || (tag(expr) == Some(pg_sys::NodeTag::T_Var)
                && (*expr.cast::<pg_sys::Var>()).varlevelsup > 0);
            if constant {
                continue;
            }
            any = true;
            if !distinct
                && ((*te).ressortgroupref == 0 || !ordered.contains(&(*te).ressortgroupref))
            {
                return OwnOrder::No;
            }
            let typid = pg_sys::getBaseType(pg_sys::exprType((*te).expr.cast()));
            if !equal_is_identical(typid, (*agg).inputcollid) {
                ties.push(cstr(pg_sys::format_type_be(typid)));
            }
        }
        match (any, ties.is_empty()) {
            (false, _) => OwnOrder::No,
            (true, true) => OwnOrder::Renders,
            (true, false) => OwnOrder::Ties(ties.join(", ")),
        }
    }
}

/// The expressions of `query`'s output columns (not those only sorted by).
///
/// SAFETY: `query` is a valid Query.
unsafe fn outputs(query: *mut pg_sys::Query) -> Vec<*mut pg_sys::Node> {
    // SAFETY: the target list of a valid Query.
    unsafe {
        elements::<pg_sys::TargetEntry>((*query).targetList)
            .into_iter()
            .filter(|&te| !(*te).resjunk)
            .map(|te| (*te).expr.cast())
            .collect()
    }
}

/// Whether two equal values of type `typid` (compared under `collation`) are the
/// same value, rendered the same: not `numeric` (1.0 = 1.00), floats (0 = -0),
/// `jsonb`, or text under a nondeterministic collation.
///
/// SAFETY: catalog lookups by OID.
unsafe fn equal_is_identical(typid: Oid, collation: Oid) -> bool {
    // SAFETY: a catalog lookup by OID.
    unsafe {
        matches!(
            typid,
            pg_sys::INT2OID
                | pg_sys::INT4OID
                | pg_sys::INT8OID
                | pg_sys::BOOLOID
                | pg_sys::UUIDOID
                | pg_sys::DATEOID
                | pg_sys::TIMESTAMPOID
                | pg_sys::TIMESTAMPTZOID
                | pg_sys::BYTEAOID
        ) || (matches!(
            typid,
            pg_sys::TEXTOID | pg_sys::VARCHAROID | pg_sys::BPCHAROID
        ) && (collation == pg_sys::InvalidOid
            || pg_sys::get_collation_isdeterministic(collation)))
    }
}
