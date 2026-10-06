//! Read a backing view's analyzed query into a [`Graph`] (ADR 0157).
//!
//! The only module that walks `pg_sys` nodes. The view's query comes from the
//! relcache (`get_view_query`) and is copied before it is walked; nothing here
//! modifies a node. Views are expanded by OID, CTEs and subqueries in place, to a
//! depth of [`MAX_DEPTH`] query levels.

#![allow(clippy::cast_ptr_alignment)] // Reason: `Node *` is cast to the node type its tag names, as PostgreSQL does; palloc aligns every node for its own type.

use super::{
    Column, Conjunct, Graph, IdentityKind, Lookup, Maps, Occurrence, OutputColumn, Piece, Root,
    Sql, WalkedIdentity,
};
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
    /// The TVIEW's entity.
    pub entity: &'a str,
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
        cte_parent: None,
        read_ctes: HashSet::new(),
        wanted: None,
        identity_level: false,
        nullable: HashSet::new(),
    };
    // SAFETY: `view_query` returns a copy owned by the current memory context; the
    // walk only reads it.
    let query = unsafe { view_query(view_oid)? };
    let flags = Flags::default();
    // SAFETY: `query` is a valid, copied Query.
    unsafe { walker.top(query, &flags)? };
    walker.note_virtual_columns();
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
    /// Inside a CTE the view never uses: its tables cannot change the output.
    unread: bool,
}

/// A column a Var stands for once views and subqueries are seen through.
#[derive(Debug, Clone)]
enum Resolved {
    Col(Column),
    /// One column per UNION branch of a subquery.
    Alt(Vec<Column>),
    /// An output computed from columns (#182).
    Expr(Computed),
    Opaque,
}

/// An immutable output computed from base columns, written over them: never a key,
/// a group key or a root, only what predicates above it compare with.
#[derive(Debug, Clone)]
struct Computed {
    sql: Sql,
    /// `unnest(<array>)`: `sql` is the array, and the output one of its elements.
    /// It can only be compared with `=`, as `x = ANY (<array>)`.
    element: bool,
    /// NULL inputs make it NULL: no NULL-extended row turns it into a value.
    strict: bool,
    /// Its type (the array's, for an element).
    type_oid: Oid,
}

impl Resolved {
    /// The occurrences a column or computed output reads.
    fn occs(&self) -> Vec<usize> {
        match self {
            Self::Col(c) => vec![c.occ],
            Self::Alt(cs) => cs.iter().map(|c| c.occ).collect(),
            Self::Expr(e) => sql_occs(&e.sql),
            Self::Opaque => vec![],
        }
    }
}

/// The occurrences whose columns a piece of SQL reads, in order, once each.
fn sql_occs(sql: &Sql) -> Vec<usize> {
    let mut occs = Vec::new();
    for piece in &sql.0 {
        if let Piece::Column { occ, .. } = piece
            && !occs.contains(occ)
        {
            occs.push(*occ);
        }
    }
    occs
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
    /// The level a `ctelevelsup` of 1 names: the enclosing level, except for a
    /// CTE body, whose references count from the level that defines the CTE.
    cte_parent: Option<usize>,
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

/// A Var of a predicate: the level it belongs to and what it stands for, a column
/// (one per UNION branch) or a computed output.
struct Site {
    levelsup: usize,
    candidates: Vec<Resolved>,
}

/// One side of a comparison, written.
struct Operand {
    sql: Sql,
    type_oid: Oid,
    /// The side is a column as it is (an index on the column serves it).
    column: bool,
    /// The side is an element of the array `sql` (an `unnest` output).
    element: bool,
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
    /// The CTE parent of the next level entered (see [`Level::cte_parent`]).
    cte_parent: Option<usize>,
    /// `(defining query, name)` of every CTE walked, by reference or as unread.
    read_ctes: HashSet<(usize, String)>,
    /// The output columns the level above reads from the next level entered
    /// (`None`: every column).
    wanted: Option<HashSet<i16>>,
    /// The next level entered is the backing view's own SELECT (no UNION): it
    /// chooses the TVIEW's identity.
    identity_level: bool,
    /// Occurrences on the nullable side of an outer join walked so far: their
    /// columns may be NULL-extended where a predicate above reads them.
    nullable: HashSet<usize>,
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
    /// Record the virtual generated columns among the keys and equalities (#179).
    fn note_virtual_columns(&mut self) {
        let graph = &self.graph;
        let columns: Vec<&Column> = graph
            .roots
            .iter()
            .map(|r| &r.key)
            .chain(
                graph
                    .conjuncts
                    .iter()
                    .filter_map(|c| c.equality.as_ref())
                    .flat_map(|(x, y)| [x, y]),
            )
            .collect();
        let found: Vec<(usize, i16)> = columns
            .into_iter()
            .filter(|c| {
                let relid = Oid::from(graph.occurrences[c.occ].relid);
                // SAFETY: a catalog lookup by OID and attribute number.
                unsafe { pg_sys::get_attgenerated(relid, c.attnum) as u8 == b'v' }
            })
            .map(|c| (c.occ, c.attnum))
            .collect();
        self.graph.virtual_columns.extend(found);
    }

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
                self.identity_level = true;
                let outputs = self.level(query, &flags, Link::Top)?;
                // The key root is the identity's column (ADR 0169).
                let root_position = match &self.graph.identity {
                    Some(Ok(identity)) => Some(identity.position),
                    _ => key_position,
                };
                if opaque.is_none()
                    && let Some(Resolved::Col(key)) = root_position.and_then(|p| outputs.get(p))
                {
                    self.graph.roots.push(Root {
                        branch: flags.branch,
                        key: key.clone(),
                    });
                }
                return Ok(());
            }
            self.graph.set_operation = true;
            // LIMIT/OFFSET over the whole set operation applies to every branch.
            let whole = top_opaque_reason(query);
            // UNION: each leaf is a branch with its own root. The leaves sit in
            // the rtable as subqueries, referenced from the set-operation tree.
            self.push_level(
                query,
                vec![RteInfo::Other; list_len((*query).rtable)],
                Link::Top,
            );
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
                self.unread_ctes(flags)
            })();
            self.levels.pop();
            // A UNION's rows are named by pk_<entity> in every branch.
            let identity = key_position
                .and_then(|p| {
                    elements::<pg_sys::TargetEntry>((*query).targetList)
                        .get(p)
                        .copied()
                })
                .zip(key_position)
                .map(|(tle, position)| WalkedIdentity {
                    name: self.ctx.key_column.to_string(),
                    position,
                    type_oid: pg_sys::exprType((*tle).expr.cast()).to_u32(),
                    kind: IdentityKind::Pk,
                    columns: self.graph.roots.iter().map(|r| r.key.clone()).collect(),
                })
                .ok_or(super::IdentityError::Missing);
            self.graph.identity = Some(identity);
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
        let wanted = self.wanted.take();
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
            self.push_level(query, Vec::new(), link);
            let result = self
                .level_body(query, &flags, link, opaque.is_some(), wanted.as_ref())
                .and_then(|outputs| self.unread_ctes(&flags).map(|()| outputs));
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
        wanted: Option<&HashSet<i16>>,
    ) -> TViewResult<Vec<Resolved>> {
        // Taken before the levels below are walked.
        let identity_level = std::mem::take(&mut self.identity_level);
        // SAFETY: fields of a valid Query; RTEs and expressions belong to it.
        unsafe {
            // A view, subquery or CTE in FROM is walked only for the columns this
            // level reads of it (#166).
            let read = referenced_columns(query);
            for (i, rte) in elements::<pg_sys::RangeTblEntry>((*query).rtable)
                .into_iter()
                .enumerate()
            {
                self.wanted = match read.get(&(i + 1)) {
                    Some(Some(columns)) => Some(columns.clone()),
                    Some(None) => None,
                    None => Some(HashSet::new()),
                };
                let info = self.rte(rte, flags);
                self.wanted = None;
                self.current().rtes.push(info?);
            }
            let jointree = (*query).jointree;
            if !jointree.is_null() {
                self.join_item(jointree.cast(), flags)?;
            }
            // Subquery expressions anywhere else in this level. Those of an output
            // column nothing above reads cannot change the TVIEW: their tables are
            // only recorded (#166).
            let unread = Flags {
                unread: true,
                ..flags.clone()
            };
            let skipped: Vec<bool> = elements::<pg_sys::TargetEntry>((*query).targetList)
                .iter()
                .map(|tle| {
                    wanted.is_some_and(|w| !w.contains(&(**tle).resno))
                        && (**tle).ressortgroupref == 0
                        && !pg_sys::expression_returns_set((**tle).expr.cast())
                })
                .collect();
            for (tle, skip) in elements::<pg_sys::TargetEntry>((*query).targetList)
                .into_iter()
                .zip(&skipped)
            {
                let tle_flags = if *skip { &unread } else { flags };
                self.sublinks((*tle).expr.cast(), tle_flags, false)?;
            }
            self.sublinks((*query).havingQual, flags, false)?;
            self.note_functions(query.cast());
            if identity_level {
                let tles = elements::<pg_sys::TargetEntry>((*query).targetList);
                self.graph.identity = Some(self.identity(query, &tles));
            }

            let grouped = (*query).hasAggs || !(*query).groupClause.is_null();
            let tles = elements::<pg_sys::TargetEntry>((*query).targetList);
            // The columns of a GROUP BY or DISTINCT ON key, and whether an output
            // column is one of them or equal to one on every row it can match
            // (#162): `DISTINCT ON (l.fk_order) o.pk_order` with
            // `l.fk_order = o.pk_order`.
            let key_columns = |clause: *mut pg_sys::List| -> Vec<Column> {
                tles.iter()
                    .filter(|tle| in_clause((***tle).ressortgroupref, clause))
                    .filter_map(|tle| match self.resolve_expr((**tle).expr.cast()) {
                        Resolved::Col(c) => Some(c),
                        _ => None,
                    })
                    .collect()
            };
            let group_keys = key_columns((*query).groupClause);
            let distinct_keys = key_columns((*query).distinctClause);
            let keyed =
                |tle: *mut pg_sys::TargetEntry, clause: *mut pg_sys::List, keys: &[Column]| {
                    in_clause((*tle).ressortgroupref, clause)
                        || matches!(self.resolve_expr((*tle).expr.cast()),
                                Resolved::Col(c) if self.equal_to_key(&c, keys))
                };
            let pass_through: Vec<bool> = tles
                .iter()
                .zip(skipped)
                .map(|(&tle, skip)| {
                    !skip
                        && (link == Link::Top
                            || (!opaque
                                && (!grouped || keyed(tle, (*query).groupClause, &group_keys))
                                && (!(*query).hasDistinctOn
                                    || keyed(tle, (*query).distinctClause, &distinct_keys))))
                })
                .collect();
            Ok(tles
                .iter()
                .zip(pass_through)
                .map(|(&tle, pass)| {
                    if pass {
                        self.output((*tle).expr.cast())
                    } else {
                        Resolved::Opaque
                    }
                })
                .collect())
        }
    }

    /// Whether an equality of the graph makes `column` equal to one of `keys` on
    /// every row of `column`'s occurrence that contributes (a WHERE or inner join
    /// condition, or an outer join's with `column` on the nullable side: NULL where
    /// it has no match).
    fn equal_to_key(&self, column: &Column, keys: &[Column]) -> bool {
        self.graph.conjuncts.iter().any(|c| {
            c.equality.as_ref().is_some_and(|(x, y)| {
                let (toward, key) = if x == column {
                    (if c.a == x.occ { c.a_to_b } else { c.b_to_a }, y)
                } else if y == column {
                    (if c.a == y.occ { c.a_to_b } else { c.b_to_a }, x)
                } else {
                    return false;
                };
                toward == Maps::Yes && keys.contains(key)
            })
        })
    }

    /// The identity of the backing view's own SELECT (ADR 0169): its DISTINCT ON
    /// key, or `pk_<entity>`.
    ///
    /// SAFETY: `query` is the valid Query of the innermost level and `tles` its
    /// target list.
    unsafe fn identity(
        &self,
        query: *mut pg_sys::Query,
        tles: &[*mut pg_sys::TargetEntry],
    ) -> Result<WalkedIdentity, super::IdentityError> {
        // SAFETY: fields of a valid Query and of its target entries.
        unsafe {
            let outputs: Vec<OutputColumn> = tles
                .iter()
                .map(|&tle| OutputColumn {
                    name: cstr((*tle).resname),
                    junk: (*tle).resjunk,
                    sortgroupref: (*tle).ressortgroupref,
                    column: match self.resolve_expr((*tle).expr.cast()) {
                        Resolved::Col(c) => Some(c),
                        _ => None,
                    },
                    type_oid: pg_sys::exprType((*tle).expr.cast()).to_u32(),
                })
                .collect();
            let distinct_on: Option<Vec<u32>> = (*query).hasDistinctOn.then(|| {
                elements::<pg_sys::SortGroupClause>((*query).distinctClause)
                    .iter()
                    .map(|c| (**c).tleSortGroupRef)
                    .collect()
            });
            let selected = super::select_identity(
                self.ctx.entity,
                &outputs,
                distinct_on.as_deref(),
                &|column, key| self.equal_to_key(column, std::slice::from_ref(key)),
            )?;
            let chosen = &outputs[selected.position];
            Ok(WalkedIdentity {
                name: chosen.name.clone(),
                position: selected.position,
                type_oid: chosen.type_oid,
                kind: selected.kind,
                columns: chosen.column.iter().cloned().collect(),
            })
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
            self.push_level(
                query,
                vec![RteInfo::Other; list_len((*query).rtable)],
                Link::From,
            );
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
                            Resolved::Expr(_) | Resolved::Opaque => {}
                        }
                    }
                }
                self.unread_ctes(flags)
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

    /// Enter a query level. Its CTE parent is the level a pending CTE lookup
    /// found the CTE in, else the enclosing level.
    fn push_level(&mut self, query: *mut pg_sys::Query, rtes: Vec<RteInfo>, link: Link) {
        let cte_parent = self
            .cte_parent
            .take()
            .or_else(|| self.levels.len().checked_sub(1));
        self.levels.push(Level {
            query,
            rtes,
            link,
            cte_parent,
        });
    }

    /// Walk the CTEs the innermost level defines and nothing used, so that the
    /// tables they read are known (they are not tracked: they cannot change the
    /// output).
    ///
    /// SAFETY: the innermost level's query is valid.
    unsafe fn unread_ctes(&mut self, flags: &Flags) -> TViewResult<()> {
        let index = self.levels.len() - 1;
        let query = self.levels[index].query;
        // SAFETY: the CTE list of a valid Query.
        let ctes = unsafe { elements::<pg_sys::CommonTableExpr>((*query).cteList) };
        for cte in ctes {
            // SAFETY: a valid CommonTableExpr of that list.
            let (name, body) = unsafe { (cstr((*cte).ctename), (*cte).ctequery) };
            if self.read_ctes.contains(&(query as usize, name.clone())) {
                continue;
            }
            self.read_ctes.insert((query as usize, name));
            let unread = Flags {
                unread: true,
                ..flags.clone()
            };
            self.cte_parent = Some(index);
            // SAFETY: a copy of the CTE's query.
            unsafe {
                let copy = pg_sys::copyObjectImpl(body.cast()).cast::<pg_sys::Query>();
                self.level(copy, &unread, Link::From)?;
            }
        }
        Ok(())
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
                // The recursive term's reference to its own CTE: the CTE is being
                // walked (#183).
                pg_sys::RTEKind::RTE_CTE if (*rte).self_reference => Ok(RteInfo::Other),
                pg_sys::RTEKind::RTE_CTE => {
                    let name = cstr((*rte).ctename);
                    let Some((cte, defined_at)) = self.cte(&name, (*rte).ctelevelsup as usize)
                    else {
                        return Ok(RteInfo::Other);
                    };
                    self.read_ctes
                        .insert((self.levels[defined_at].query as usize, name));
                    let copy =
                        pg_sys::copyObjectImpl((*cte).ctequery.cast()).cast::<pg_sys::Query>();
                    self.cte_parent = Some(defined_at);
                    if !(*cte).cterecursive {
                        return Ok(RteInfo::Outputs(self.level(copy, flags, Link::From)?));
                    }
                    // A row of a recursive CTE comes from rows of the step before:
                    // nothing links it to the rows of the tables it reads (#183).
                    let recursive = Flags {
                        opaque_level: Some(format!(
                            "read in a recursive CTE ({})",
                            flags.via_view.as_deref().unwrap_or("the definition")
                        )),
                        ..flags.clone()
                    };
                    let width = self.level(copy, &recursive, Link::From)?.len();
                    Ok(RteInfo::Outputs(vec![Resolved::Opaque; width]))
                }
                pg_sys::RTEKind::RTE_JOIN => Ok(RteInfo::Join((*rte).joinaliasvars)),
                pg_sys::RTEKind::RTE_FUNCTION => Ok(self.function_rte(rte)),
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
            b'r' | b'p' if flags.unread => {
                self.graph.unread_tables.insert(relid.to_u32());
                Ok(RteInfo::Other)
            }
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

    /// A function in FROM: `unnest(<array>)` alone, without ordinality, stands for
    /// the elements of the array (#182); anything else is opaque.
    ///
    /// SAFETY: `rte` is a valid `RTE_FUNCTION` entry of the innermost level, whose
    /// earlier entries are known.
    unsafe fn function_rte(&mut self, rte: *mut pg_sys::RangeTblEntry) -> RteInfo {
        // SAFETY: fields of a valid RTE and of its function list.
        unsafe {
            let functions = elements::<pg_sys::RangeTblFunction>((*rte).functions);
            let [function] = functions[..] else {
                return RteInfo::Other;
            };
            if (*rte).funcordinality {
                return RteInfo::Other;
            }
            match unnest_array((*function).funcexpr) {
                Some(array) => RteInfo::Outputs(vec![self.computed(array, true)]),
                None => RteInfo::Other,
            }
        }
    }

    /// CTE `name`, defined `levelsup` levels above the innermost one, and the index
    /// of the level that defines it. Levels are counted along CTE parents: the
    /// references inside a CTE body count from where it is defined.
    fn cte(&self, name: &str, levelsup: usize) -> Option<(*mut pg_sys::CommonTableExpr, usize)> {
        let mut level = self.levels.len().checked_sub(1)?;
        for _ in 0..levelsup {
            level = self.levels[level].cte_parent?;
        }
        let query = self.levels[level].query;
        // SAFETY: the CTE list of a valid Query.
        unsafe {
            elements::<pg_sys::CommonTableExpr>((*query).cteList)
                .into_iter()
                .find(|cte| cstr((**cte).ctename) == *name)
                .map(|cte| (cte, level))
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
                    // Above this join, its nullable side may be NULL-extended.
                    match (*join).jointype {
                        pg_sys::JoinType::JOIN_INNER => {}
                        pg_sys::JoinType::JOIN_LEFT => self.nullable.extend(&right),
                        pg_sys::JoinType::JOIN_RIGHT => self.nullable.extend(&left),
                        _ => self.nullable.extend(left.iter().chain(&right)),
                    }
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
            Some(RteInfo::Outputs(outputs)) => outputs.iter().flat_map(Resolved::occs).collect(),
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
    /// can be used: immutable, written with supported nodes, and never true on a
    /// NULL-extended row ([`Walker::null_safe`]).
    ///
    /// SAFETY: `qual` is a valid expression of the innermost level.
    unsafe fn predicate(&mut self, qual: *mut pg_sys::Node, origin: Origin<'_>) {
        // SAFETY: read-only checks of a valid expression.
        unsafe {
            if matches!(origin, Origin::None)
                || pg_sys::contain_mutable_functions(qual)
                || has_sublink(qual)
            {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(qual, &mut sites) || !self.null_safe(qual, &sites) {
                return;
            }
            self.add_conjuncts(qual, &sites, &|_| None, origin, true);
        }
    }

    /// Whether a NULL-extended row cannot make `expr` true: `expr` is strict, or no
    /// occurrence it reads is on the nullable side of an outer join walked so far
    /// (one below the predicate). `x = ANY (string_to_array(n.path, '.'))` is not
    /// strict, and is used over inner joins (#182).
    ///
    /// SAFETY: `expr` is a valid expression and `sites` its Vars and Params.
    unsafe fn null_safe(&self, expr: *mut pg_sys::Node, sites: &[Site]) -> bool {
        let terms = || sites.iter().flat_map(|s| &s.candidates);
        // SAFETY: a read-only check of a valid expression.
        let strict = unsafe { !pg_sys::contain_nonstrict_functions(expr) }
            && terms().all(|t| !matches!(t, Resolved::Expr(e) if !e.strict));
        strict
            || terms()
                .flat_map(Resolved::occs)
                .all(|occ| !self.nullable.contains(&occ))
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
            if pg_sys::contain_mutable_functions(test) {
                return;
            }
            let mut sites = Vec::new();
            if !self.sites(test, &mut sites) {
                return;
            }
            // The subquery's columns sit one level below the innermost one.
            let param_column = |param: *mut pg_sys::Param| -> Option<Vec<Resolved>> {
                let p = &*param;
                if p.paramkind != pg_sys::ParamKind::PARAM_SUBLINK {
                    return None;
                }
                match outputs.get(usize::try_from(p.paramid).ok()?.checked_sub(1)?)? {
                    Resolved::Alt(cs) => Some(cs.iter().cloned().map(Resolved::Col).collect()),
                    Resolved::Opaque => None,
                    term => Some(vec![term.clone()]),
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
            if !self.null_safe(test, &sites) {
                return;
            }
            let lookup = |node: *mut pg_sys::Node| -> Option<usize> {
                param_sites
                    .iter()
                    .position(|p| p.cast::<pg_sys::Node>() == node)
            };
            self.add_conjuncts(test, &sites, &lookup, Origin::Required, required);
        }
    }

    /// Resolve every Var of `expr`; false if one is opaque.
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
                    Resolved::Alt(cs) => cs.into_iter().map(Resolved::Col).collect(),
                    Resolved::Opaque => return false,
                    term => vec![term],
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
            let chosen: Vec<&Resolved> = sites
                .iter()
                .map(|s| {
                    let c = &s.candidates[rest % s.candidates.len()];
                    rest /= s.candidates.len();
                    c
                })
                .collect();
            let mut occs: Vec<usize> = chosen.iter().flat_map(|c| c.occs()).collect();
            occs.sort_unstable();
            occs.dedup();
            let [a, b] = occs[..] else { continue };
            let Some((a_to_b, b_to_a)) =
                self.directions(sites, &chosen, a, b, origin, outer_to_inner)
            else {
                continue;
            };
            let var_index = std::cell::Cell::new(0_usize);
            let term_of = |node: *mut pg_sys::Node| -> Option<Resolved> {
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
            let Some((sql, lookups)) =
                (unsafe { self.conjunct_sql(expr, &term_of, &mut next_var) })
            else {
                return;
            };
            // SAFETY: the same expression.
            let equality = unsafe { equality(expr, &chosen) };
            // Only an equality is known to fail on the NULLs of an unmatched row.
            let checked = |m: Maps| {
                if m == Maps::IfMatched && equality.is_none() {
                    Maps::No
                } else {
                    m
                }
            };
            let (a_to_b, b_to_a) = (checked(a_to_b), checked(b_to_a));
            if a_to_b == Maps::No && b_to_a == Maps::No {
                continue;
            }
            self.graph.conjuncts.push(Conjunct {
                sql,
                a,
                b,
                a_to_b,
                b_to_a,
                equality,
                lookups,
            });
        }
    }

    /// In which directions a predicate between occurrences `a` and `b` must hold.
    fn directions(
        &self,
        sites: &[Site],
        chosen: &[&Resolved],
        a: usize,
        b: usize,
        origin: Origin<'_>,
        outer_to_inner: bool,
    ) -> Option<(Maps, Maps)> {
        // The level of each occurrence relative to the predicate's: 0 here, n above,
        // usize::MAX below (a subquery's output).
        let level_of = |occ: usize| {
            sites
                .iter()
                .zip(chosen)
                .filter(|(_, c)| c.occs().contains(&occ))
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
        let (a_to_b, b_to_a) = match da.cmp(&db) {
            std::cmp::Ordering::Equal => (true, true),
            std::cmp::Ordering::Less => (true, outer_ok(db)),
            std::cmp::Ordering::Greater => (outer_ok(da), true),
        };
        // An outer join's condition holds only for rows of its nullable side. Toward
        // that side it still maps a row that has a match (#165).
        let maps = |holds: bool, from: usize, to: usize| match origin {
            Origin::Outer { nullable } if holds && !nullable.contains(&from) => {
                if nullable.contains(&to) {
                    Maps::IfMatched
                } else {
                    Maps::No
                }
            }
            _ if holds => Maps::Yes,
            _ => Maps::No,
        };
        let (a_to_b, b_to_a) = (maps(a_to_b, a, b), maps(b_to_a, b, a));
        (a_to_b != Maps::No || b_to_a != Maps::No).then_some((a_to_b, b_to_a))
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

    /// What an output column of the innermost level stands for: a column, an
    /// expression computed from columns, an element of `unnest(<array>)`, or
    /// opaque (another set-returning function: its value is not computed from the
    /// row it comes from).
    ///
    /// SAFETY: `node` is null or a valid expression of the innermost level.
    unsafe fn output(&mut self, node: *mut pg_sys::Node) -> Resolved {
        // SAFETY: read-only checks of a valid expression, then forwarded.
        unsafe {
            if tag(strip_relabel(node)) == Some(pg_sys::NodeTag::T_Var) {
                self.resolve_expr(node)
            } else if pg_sys::expression_returns_set(node) {
                unnest_array(node).map_or(Resolved::Opaque, |array| self.computed(array, true))
            } else {
                self.computed(node, false)
            }
        }
    }

    /// `expr` of the innermost level written over the columns it reads: opaque
    /// when it is not immutable, reads no column (or an opaque one), holds a
    /// subquery or a node [`Walker::deparse`] does not write (an aggregate).
    ///
    /// SAFETY: `expr` is null or a valid expression of the innermost level.
    unsafe fn computed(&mut self, expr: *mut pg_sys::Node, element: bool) -> Resolved {
        // SAFETY: read-only checks and a walk of a valid expression.
        unsafe {
            if expr.is_null()
                || pg_sys::contain_mutable_functions(expr)
                || has_sublink(expr)
                || pg_sys::expression_returns_set(expr)
            {
                return Resolved::Opaque;
            }
            let mut vars = Vec::new();
            collect_vars(expr, &mut vars);
            if vars.is_empty() {
                return Resolved::Opaque;
            }
            let mut terms = Vec::new();
            for var in vars {
                match self.resolve_var(var, (*var).varlevelsup as usize) {
                    term @ (Resolved::Col(_) | Resolved::Expr(_)) => terms.push(term),
                    _ => return Resolved::Opaque,
                }
            }
            let strict = !pg_sys::contain_nonstrict_functions(expr)
                && terms
                    .iter()
                    .all(|t| !matches!(t, Resolved::Expr(e) if !e.strict));
            let index = std::cell::Cell::new(0_usize);
            let term_of = |node: *mut pg_sys::Node| {
                (tag(node) == Some(pg_sys::NodeTag::T_Var))
                    .then(|| terms.get(index.get()).cloned())
                    .flatten()
            };
            let mut next_var = || index.set(index.get() + 1);
            match self.deparse(expr, &term_of, &mut next_var) {
                Some(sql) => Resolved::Expr(Computed {
                    sql,
                    element,
                    strict,
                    type_oid: pg_sys::exprType(expr),
                }),
                None => Resolved::Opaque,
            }
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

    /// Write a predicate as SQL, with the expressions it looks rows up by. As
    /// [`Walker::deparse`], except that `=` with an element of `unnest(<array>)` is
    /// written as array membership, `x = ANY (<array>)`, and membership with the
    /// element type's equality also as containment, `<array> @> ARRAY[x]`, which a
    /// GIN index on the array serves (#182).
    ///
    /// SAFETY: `expr` is a valid expression.
    unsafe fn conjunct_sql(
        &mut self,
        expr: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<(Sql, Vec<Lookup>)> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(expr)? {
                pg_sys::NodeTag::T_OpExpr => {
                    let op = expr.cast::<pg_sys::OpExpr>();
                    if let [l, r] = elements::<pg_sys::Node>((*op).args)[..]
                        && cstr(pg_sys::get_opname((*op).opno)) == "="
                    {
                        let l = self.operand(l, term_of, next_var)?;
                        let r = self.operand(r, term_of, next_var)?;
                        return match (l.element, r.element) {
                            (false, false) => Some(self.comparison((*op).opno, &l, &r)),
                            (false, true) => self.membership((*op).opno, &l, &r),
                            (true, false) => {
                                let commutator = pg_sys::get_commutator((*op).opno);
                                (commutator != Oid::INVALID)
                                    .then(|| self.membership(commutator, &r, &l))
                                    .flatten()
                            }
                            (true, true) => None,
                        };
                    }
                }
                pg_sys::NodeTag::T_ScalarArrayOpExpr => {
                    let op = expr.cast::<pg_sys::ScalarArrayOpExpr>();
                    if let [l, r] = elements::<pg_sys::Node>((*op).args)[..]
                        && (*op).useOr
                    {
                        let l = self.operand(l, term_of, next_var)?;
                        let r = self.operand(r, term_of, next_var)?;
                        if l.element || r.element {
                            return None;
                        }
                        return self.membership((*op).opno, &l, &r);
                    }
                }
                _ => {}
            }
            Some((self.deparse(expr, term_of, next_var)?, Vec::new()))
        }
    }

    /// One side of a comparison: written by [`Walker::deparse`], or the array of an
    /// `unnest` element.
    ///
    /// SAFETY: `arg` is a valid expression; its Vars are the next ones `term_of`
    /// gives.
    unsafe fn operand(
        &mut self,
        arg: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Operand> {
        // SAFETY: checked by tag; `term_of` only reads the node.
        unsafe {
            let bare = strip_relabel(arg);
            let term = match tag(bare) {
                Some(pg_sys::NodeTag::T_Var | pg_sys::NodeTag::T_Param) => term_of(bare),
                _ => None,
            };
            if let Some(Resolved::Expr(e)) = &term
                && e.element
            {
                if tag(bare) == Some(pg_sys::NodeTag::T_Var) {
                    next_var();
                }
                return Some(Operand {
                    sql: e.sql.clone(),
                    type_oid: e.type_oid,
                    column: false,
                    element: true,
                });
            }
            Some(Operand {
                sql: self.deparse(arg, term_of, next_var)?,
                type_oid: pg_sys::exprType(arg),
                column: matches!(term, Some(Resolved::Col(_))),
                element: false,
            })
        }
    }

    /// `l = r`; a computed side compared with a column is looked up by (btree).
    fn comparison(&mut self, opno: Oid, l: &Operand, r: &Operand) -> (Sql, Vec<Lookup>) {
        let name = self.operator_name(opno);
        let mut sql = Sql::text("(");
        sql.push_sql(l.sql.clone());
        sql.push_text(&format!(" OPERATOR({name}) "));
        sql.push_sql(r.sql.clone());
        sql.push_text(")");
        let mut lookups = Vec::new();
        for (side, other) in [(l, r), (r, l)] {
            if !side.column
                && other.column
                && let [occ] = sql_occs(&side.sql)[..]
            {
                lookups.push(Lookup {
                    occ,
                    expr: side.sql.clone(),
                    gin: false,
                });
            }
        }
        (sql, lookups)
    }

    /// `scalar op ANY (array)`. With the element type's own equality it also
    /// writes `array @> ARRAY[scalar]`: true whenever the membership is, and served
    /// by a GIN index on the array.
    fn membership(
        &mut self,
        opno: Oid,
        scalar: &Operand,
        array: &Operand,
    ) -> Option<(Sql, Vec<Lookup>)> {
        let name = self.operator_name(opno);
        let mut sql = Sql::text("(");
        sql.push_sql(scalar.sql.clone());
        sql.push_text(&format!(" OPERATOR({name}) ANY ("));
        sql.push_sql(array.sql.clone());
        sql.push_text("))");
        // SAFETY: catalog lookups by type OID.
        let containment = unsafe {
            let element = pg_sys::get_element_type(array.type_oid);
            element != Oid::INVALID
                && element == scalar.type_oid
                && (*pg_sys::lookup_type_cache(element, pg_sys::TYPECACHE_EQ_OPR.cast_signed()))
                    .eq_opr
                    == opno
        };
        let mut lookups = Vec::new();
        if containment {
            let mut both = Sql::text("(");
            both.push_sql(sql);
            both.push_text(" AND (");
            both.push_sql(array.sql.clone());
            both.push_text(") OPERATOR(pg_catalog.@>) ARRAY[");
            both.push_sql(scalar.sql.clone());
            both.push_text("])");
            sql = both;
            if let [occ] = sql_occs(&array.sql)[..] {
                lookups.push(Lookup {
                    occ,
                    expr: array.sql.clone(),
                    gin: true,
                });
            }
        }
        Some((sql, lookups))
    }

    /// Write `expr` as SQL that resolves no name through `search_path`: relations,
    /// operators, functions and types are schema-qualified. `None` for a node it
    /// does not write, or an `unnest` element outside a comparison. `term_of` gives
    /// what each Var / Param stands for (Vars in walk order); `next_var` is called
    /// after each Var.
    ///
    /// SAFETY: `expr` is a valid expression.
    unsafe fn deparse(
        &mut self,
        expr: *mut pg_sys::Node,
        term_of: &dyn Fn(*mut pg_sys::Node) -> Option<Resolved>,
        next_var: &mut dyn FnMut(),
    ) -> Option<Sql> {
        // SAFETY: each node is checked by tag before it is cast.
        unsafe {
            match tag(expr)? {
                pg_sys::NodeTag::T_Var => {
                    let term = term_of(expr)?;
                    next_var();
                    term_sql(&term)
                }
                pg_sys::NodeTag::T_Param => term_sql(&term_of(expr)?),
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
                            sql.push_sql(self.deparse(arg, term_of, next_var)?);
                        }
                        [l, r] => {
                            sql.push_sql(self.deparse(l, term_of, next_var)?);
                            sql.push_text(&format!(" OPERATOR({name}) "));
                            sql.push_sql(self.deparse(r, term_of, next_var)?);
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
                    sql.push_sql(self.deparse(l, term_of, next_var)?);
                    sql.push_text(&format!(
                        " OPERATOR({name}) {} (",
                        if (*op).useOr { "ANY" } else { "ALL" }
                    ));
                    sql.push_sql(self.deparse(r, term_of, next_var)?);
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
                            sql.push_sql(self.deparse(arg, term_of, next_var)?);
                        }
                        sql.push_text(")");
                    } else {
                        let [arg, ..] = args[..] else { return None };
                        sql = Sql::text("(");
                        sql.push_sql(self.deparse(arg, term_of, next_var)?);
                        sql.push_text(&format!(")::{}", self.type_name((*f).funcresulttype)));
                    }
                    Some(sql)
                }
                pg_sys::NodeTag::T_RelabelType => {
                    let r = expr.cast::<pg_sys::RelabelType>();
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse((*r).arg.cast(), term_of, next_var)?);
                    sql.push_text(&format!(")::{}", self.type_name((*r).resulttype)));
                    Some(sql)
                }
                pg_sys::NodeTag::T_ArrayCoerceExpr => {
                    let r = expr.cast::<pg_sys::ArrayCoerceExpr>();
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse((*r).arg.cast(), term_of, next_var)?);
                    sql.push_text(&format!(")::{}", self.type_name((*r).resulttype)));
                    Some(sql)
                }
                pg_sys::NodeTag::T_CoerceViaIO => {
                    let r = expr.cast::<pg_sys::CoerceViaIO>();
                    let mut sql = Sql::text("(");
                    sql.push_sql(self.deparse((*r).arg.cast(), term_of, next_var)?);
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
                            sql.push_sql(self.deparse(*args.first()?, term_of, next_var)?);
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
                                sql.push_sql(self.deparse(arg, term_of, next_var)?);
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
            cte_parent: self.cte_parent,
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

/// Why none of a level's columns can be seen through from the level above: a
/// window function, LIMIT/OFFSET or GROUPING SETS decide which rows exist, or what
/// they hold, from rows other than their own.
///
/// A set-returning function in the select list only multiplies rows: the other
/// output columns keep the values of the row they come from, so only the outputs
/// that return a set are opaque (see `Walker::output`).
///
/// SAFETY: `query` is a valid Query.
unsafe fn opaque_reason(query: *mut pg_sys::Query) -> Option<String> {
    // SAFETY: fields of a valid Query.
    unsafe {
        if (*query).hasWindowFuncs {
            Some("read under a window function".to_string())
        } else if !(*query).limitCount.is_null() || !(*query).limitOffset.is_null() {
            Some("read under LIMIT/OFFSET".to_string())
        } else if !(*query).groupingSets.is_null() {
            Some("read under GROUPING SETS".to_string())
        } else {
            None
        }
    }
}

/// [`opaque_reason`] for a level whose output is the TVIEW itself, where a
/// set-returning function is one too: the rows it makes share one key.
///
/// SAFETY: `query` is a valid Query.
unsafe fn top_opaque_reason(query: *mut pg_sys::Query) -> Option<String> {
    // SAFETY: fields of a valid Query.
    unsafe {
        opaque_reason(query)
            .or_else(|| {
                (*query)
                    .hasTargetSRFs
                    .then(|| "read under a set-returning function".to_string())
            })
            .map(|why| format!("{why} in the top-level SELECT"))
    }
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

/// The SQL a Var or Param stands for: a column, or a computed output in
/// parentheses; `None` for an `unnest` element.
fn term_sql(term: &Resolved) -> Option<Sql> {
    match term {
        Resolved::Col(c) => Some(c.sql()),
        Resolved::Expr(e) if !e.element => {
            let mut sql = Sql::text("(");
            sql.push_sql(e.sql.clone());
            sql.push_text(")");
            Some(sql)
        }
        _ => None,
    }
}

/// The array of a one-argument `unnest(<array>)` call.
///
/// SAFETY: `node` is null or a valid expression.
unsafe fn unnest_array(node: *mut pg_sys::Node) -> Option<*mut pg_sys::Node> {
    // SAFETY: checked by tag before the cast.
    unsafe {
        let node = strip_relabel(node);
        if tag(node) != Some(pg_sys::NodeTag::T_FuncExpr) {
            return None;
        }
        let f = node.cast::<pg_sys::FuncExpr>();
        if (*f).funcid != Oid::from(pg_sys::F_UNNEST_ANYARRAY) {
            return None;
        }
        let [array] = elements::<pg_sys::Node>((*f).args)[..] else {
            return None;
        };
        Some(array)
    }
}

/// `a.col = b.col` with `=`, as written.
///
/// SAFETY: `expr` is a valid expression.
unsafe fn equality(expr: *mut pg_sys::Node, chosen: &[&Resolved]) -> Option<(Column, Column)> {
    // SAFETY: checked by tag before each cast.
    unsafe {
        let [Resolved::Col(x), Resolved::Col(y)] = chosen[..] else {
            return None;
        };
        if tag(expr) != Some(pg_sys::NodeTag::T_OpExpr) {
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
        (plain && cstr(pg_sys::get_opname((*op).opno)) == "=").then(|| (x.clone(), y.clone()))
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

/// The columns `query` reads from each of its range table entries, by rtindex:
/// `Some(columns)`, or `None` when it reads the whole row. An entry it doesn't
/// read is absent. Vars of nested subqueries that point at this level count; a
/// reference through a join alias counts as one to the columns behind it.
///
/// SAFETY: `query` is a valid Query.
unsafe fn referenced_columns(query: *mut pg_sys::Query) -> HashMap<usize, Option<HashSet<i16>>> {
    struct Refs {
        depth: u32,
        vars: Vec<(usize, i16)>,
    }
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
