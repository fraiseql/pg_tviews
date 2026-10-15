//! Helpers over query nodes: opaque levels, casts, set operations, quoting.

use super::{Column, Oid, Resolved, Sql, cstr, elements, list_len, pg_sys, tag};

/// Why a level's occurrences sit under a window function.
pub(super) const WINDOW_REASON: &str = "read under a window function";

/// Why none of a level's columns can be seen through from the level above: a
/// window function, LIMIT/OFFSET or GROUPING SETS decide which rows exist, or what
/// they hold, from rows other than their own. Window functions all partitioned
/// (`partitioned`, see [`windows_partitioned`]) don't count: the level above sees
/// their partition columns.
///
/// A set-returning function in the select list only multiplies rows: the other
/// output columns keep the values of the row they come from, so only the outputs
/// that return a set are opaque (see `Walker::output`).
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn opaque_reason(query: *mut pg_sys::Query, partitioned: bool) -> Option<String> {
    // SAFETY: fields of a valid Query.
    unsafe {
        if (*query).hasWindowFuncs && !partitioned {
            Some(WINDOW_REASON.to_string())
        } else if !(*query).limitCount.is_null() || !(*query).limitOffset.is_null() {
            Some("read under LIMIT/OFFSET".to_string())
        } else if !(*query).groupingSets.is_null() {
            Some("read under GROUPING SETS".to_string())
        } else {
            None
        }
    }
}

/// Whether a level has window functions and every one of its windows has a
/// PARTITION BY: a row's window values then come from the rows of its partition
/// only.
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn windows_partitioned(query: *mut pg_sys::Query) -> bool {
    // SAFETY: fields of a valid Query and of its window clauses.
    unsafe {
        let windows = elements::<pg_sys::WindowClause>((*query).windowClause);
        (*query).hasWindowFuncs
            && !windows.is_empty()
            && windows.iter().all(|w| list_len((**w).partitionClause) > 0)
    }
}

/// [`opaque_reason`] for a level whose output is the TVIEW itself, where a
/// set-returning function is one too: the rows it makes share one key.
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn top_opaque_reason(query: *mut pg_sys::Query) -> Option<String> {
    // SAFETY: fields of a valid Query.
    unsafe {
        opaque_reason(query, false)
            .or_else(|| {
                (*query)
                    .hasTargetSRFs
                    .then(|| "read under a set-returning function".to_string())
            })
            .map(|why| format!("{why} in the top-level SELECT"))
    }
}

/// The position of the output column named `name`.
///
/// SAFETY: `query` is a valid Query.
pub(super) unsafe fn output_position(query: *mut pg_sys::Query, name: &str) -> Option<usize> {
    // SAFETY: the target list of a valid Query.
    unsafe {
        elements::<pg_sys::TargetEntry>((*query).targetList)
            .iter()
            .position(|tle| !(**tle).resjunk && cstr((**tle).resname) == name)
    }
}

/// Whether sort/group reference `sortref` appears in a GROUP BY / DISTINCT clause.
///
/// SAFETY: `clause` is null or a valid list of `SortGroupClause`.
pub(super) unsafe fn in_clause(sortref: pg_sys::Index, clause: *mut pg_sys::List) -> bool {
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
pub(super) unsafe fn setop_leaves(node: *mut pg_sys::Node, leaves: &mut Vec<usize>) {
    // SAFETY: checked by tag before each cast.
    unsafe {
        match tag(node) {
            Some(pg_sys::NodeTag::T_RangeTblRef) => {
                leaves.push(super::index((*node.cast::<pg_sys::RangeTblRef>()).rtindex));
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
pub(super) unsafe fn strip_relabel(mut node: *mut pg_sys::Node) -> *mut pg_sys::Node {
    // SAFETY: checked by tag before each cast.
    unsafe {
        while tag(node) == Some(pg_sys::NodeTag::T_RelabelType) {
            node = (*node.cast::<pg_sys::RelabelType>()).arg.cast();
        }
    }
    node
}

/// A cast of one value: the type it gives, and whether NULL stays NULL.
#[derive(Debug, Clone, Copy)]
pub(super) struct Cast {
    pub(super) type_oid: Oid,
    pub(super) strict: bool,
}

/// `node` seen through the casts around it (binary, I/O, or a one-argument cast
/// function), and those casts, innermost first.
///
/// SAFETY: `node` is null or a valid expression.
pub(super) unsafe fn strip_casts(mut node: *mut pg_sys::Node) -> (*mut pg_sys::Node, Vec<Cast>) {
    let mut casts = Vec::new();
    // SAFETY: checked by tag before each cast.
    unsafe {
        loop {
            let (arg, cast) = match tag(node) {
                Some(pg_sys::NodeTag::T_RelabelType) => {
                    let r = node.cast::<pg_sys::RelabelType>();
                    (
                        (*r).arg.cast(),
                        Cast {
                            type_oid: (*r).resulttype,
                            strict: true,
                        },
                    )
                }
                Some(pg_sys::NodeTag::T_CoerceViaIO) => {
                    let r = node.cast::<pg_sys::CoerceViaIO>();
                    (
                        (*r).arg.cast(),
                        Cast {
                            type_oid: (*r).resulttype,
                            strict: true,
                        },
                    )
                }
                Some(pg_sys::NodeTag::T_FuncExpr) => {
                    let f = node.cast::<pg_sys::FuncExpr>();
                    let is_cast = matches!(
                        (*f).funcformat,
                        pg_sys::CoercionForm::COERCE_EXPLICIT_CAST
                            | pg_sys::CoercionForm::COERCE_IMPLICIT_CAST
                    );
                    let [arg] = elements::<pg_sys::Node>((*f).args)[..] else {
                        break;
                    };
                    if !is_cast {
                        break;
                    }
                    let strict = pg_sys::func_strict((*f).funcid);
                    (
                        arg,
                        Cast {
                            type_oid: (*f).funcresulttype,
                            strict,
                        },
                    )
                }
                _ => break,
            };
            casts.push(cast);
            node = arg;
        }
    }
    casts.reverse();
    (node, casts)
}

/// A positive `EXISTS (…)` or `x IN (…)`.
///
/// SAFETY: `node` is a valid expression.
pub(super) unsafe fn is_required_sublink(node: *mut pg_sys::Node) -> bool {
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
pub(super) fn term_sql(term: &Resolved) -> Option<Sql> {
    match term {
        Resolved::Col(c) | Resolved::Inbound(c) => Some(c.sql()),
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
pub(super) unsafe fn unnest_array(node: *mut pg_sys::Node) -> Option<*mut pg_sys::Node> {
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
pub(super) unsafe fn equality(
    expr: *mut pg_sys::Node,
    chosen: &[&Resolved],
) -> Option<(Column, Column)> {
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

/// SQL string literal of `text`, quoted by PostgreSQL (`quote_literal()`).
pub(super) fn quote_literal(text: &str) -> Option<String> {
    let c = std::ffi::CString::new(text).ok()?;
    // SAFETY: a NUL-terminated string; the palloc'd result is copied.
    Some(cstr(unsafe { pg_sys::quote_literal_cstr(c.as_ptr()) }))
}
