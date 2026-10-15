//! Read a backing view's analyzed query into a [`QueryGraph`] (ADR 0157).
//!
//! The only module that walks `pg_sys` nodes. The view's query comes from the
//! relcache (`get_view_query`) and is copied before it is walked; nothing here
//! modifies a node. Views are expanded by OID, CTEs and subqueries in place, to a
//! depth of [`MAX_DEPTH`] query levels.

#![allow(clippy::cast_ptr_alignment)] // Reason: `Node *` is cast to the node type its tag names, as PostgreSQL does; palloc aligns every node for its own type.

use super::{
    Column, Conjunct, DataEmbed, DataField, DataShape, IdentityKind, Lookup, Maps, Occurrence,
    OutputColumn, Piece, QueryGraph, Root, Scope, Sql, WalkedIdentity,
};
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;

/// Query levels (views, subqueries, CTEs) followed before giving up.
pub const MAX_DEPTH: usize = 32;

/// A range-table index, or an attribute number, as a `usize`: positive in a query
/// tree; 0 (which names no entry) for anything else.
pub(super) fn index(n: impl TryInto<usize>) -> usize {
    n.try_into().unwrap_or(0)
}

/// What the walk needs to know about the registered TVIEWs.
pub struct Context<'a> {
    /// Tables of TVIEWs (`tv_*`) → their entity: not base tables, as in
    /// `pg_tview_reads`.
    pub tview_tables: &'a HashMap<Oid, String>,
    /// Backing views of other TVIEWs → their entity.
    pub tview_views: &'a HashMap<Oid, String>,
    /// The TVIEW's entity.
    pub entity: &'a str,
    /// The TVIEW's key column, `pk_<entity>`.
    pub key_column: &'a str,
}

/// Read the backing view `view_oid` into a [`QueryGraph`].
///
/// # Errors
/// Returns an error if the view cannot be opened, nests deeper than
/// [`MAX_DEPTH`] levels, or reads a view the current user may not read.
pub fn analyze(view_oid: Oid, ctx: &Context<'_>) -> TViewResult<QueryGraph> {
    let mut walker = Walker {
        ctx,
        graph: QueryGraph::default(),
        levels: Vec::new(),
        catalog: CatalogNames::default(),
        cte_parent: None,
        read_ctes: HashSet::new(),
        wanted: None,
        identity_level: false,
        nullable: HashSet::new(),
        unions: 0,
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
        // Only a view has the rewrite rule get_view_query reads.
        if (*(*rel).rd_rel).relkind != pg_sys::RELKIND_VIEW.cast_signed() {
            pg_sys::relation_close(rel, pg_sys::AccessShareLock.cast_signed());
            return Err(TViewError::CatalogError {
                operation: format!("Read the query of view {view_oid:?}"),
                pg_error: "relation is not a view".to_string(),
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
    /// The UNION leaves the level sits in, outermost first.
    unions: Scope,
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
    /// What the column stands for in each UNION branch of a subquery (a column
    /// or a computed output), and the scopes of the branches where it is opaque.
    Alt(Vec<Self>, Vec<Scope>),
    /// An output computed from columns.
    Expr(Computed),
    /// A column of a first-row level (`DISTINCT ON`, or windows all partitioned)
    /// that is not its key: which row carries it depends on the other rows
    /// of its partition, so a predicate on it maps a row of another occurrence
    /// toward the rows carrying it (a superset of the first ones, whose key then
    /// maps on), never a row of its own occurrence away from it.
    Inbound(Column),
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
            Self::Col(c) | Self::Inbound(c) => vec![c.occ],
            Self::Alt(terms, _) => terms.iter().flat_map(Self::occs).collect(),
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
    /// The table of another TVIEW: an occurrence like a base table's, and
    /// an equality on its key says where it is embedded.
    Tview {
        entity: String,
        relid: Oid,
        occ: usize,
    },
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
    graph: QueryGraph,
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
    /// UNIONs entered so far: the next one's number.
    unions: usize,
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

mod collect;
mod data;
mod deparse;
mod expr;
mod level;
mod nodes;
mod predicate;
mod time;

use crate::utils::ident::quote_if_needed;
use collect::{
    base_read, collect_functions, collect_params, collect_sublinks, collect_vars,
    column_read_counts, const_text, has_sublink, referenced_columns, relation_entry,
};
use nodes::{
    Cast, WINDOW_REASON, equality, in_clause, is_required_sublink, opaque_reason, output_position,
    quote_literal, setop_leaves, strip_casts, strip_relabel, term_sql, top_opaque_reason,
    unnest_array, windows_partitioned,
};

#[cfg(test)]
mod tests {
    use super::time::time_value_name;
    use pgrx::pg_sys::SQLValueFunctionOp as Op;

    #[test]
    fn the_time_value_functions_are_named_as_written() {
        assert_eq!(
            time_value_name(Op::SVFOP_CURRENT_DATE),
            Some("CURRENT_DATE")
        );
        assert_eq!(
            time_value_name(Op::SVFOP_CURRENT_TIMESTAMP_N),
            Some("CURRENT_TIMESTAMP")
        );
        assert_eq!(time_value_name(Op::SVFOP_LOCALTIME_N), Some("LOCALTIME"));
        assert_eq!(time_value_name(Op::SVFOP_CURRENT_USER), None);
        assert_eq!(time_value_name(Op::SVFOP_CURRENT_SCHEMA), None);
    }
}
