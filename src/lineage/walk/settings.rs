//! The custom settings a definition reads with `current_setting()` (dialect.md,
//! Axis D). Every refresh reads them in the owner frame, where a setting has the
//! value of the server, the database, `ALTER ROLE ALL … SET` or the TVIEW's
//! `settings` option, never a session's (`owner::custom`). A setting read for a
//! value (a label's locale) is reported at registration with that value. A
//! setting that decides which rows the TVIEW holds (in a `WHERE`, `ON`, `HAVING`,
//! `LIMIT` or `OFFSET`, or a set-returning function's arguments in `FROM`, of the
//! definition or of a view or subquery it reads in `FROM`) is refused: a
//! per-request setting such as a tenant would keep the rows of the server's value
//! only.
//!
//! Function bodies are not visible: a setting a called function reads is read in
//! the frame too, unreported.

use super::{Oid, cstr, elements, pg_guard, pg_sys, tag, view_query};
use crate::error::{TViewError, TViewResult};

/// One `current_setting()` call of a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingRead {
    /// The setting's name, `None` when an expression computes it.
    pub name: Option<String>,
    /// It decides which rows the TVIEW holds, not only a value of one.
    pub decides_rows: bool,
}

/// Where the expressions being walked sit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// They compute a value of a row.
    Value,
    /// They decide which rows the TVIEW holds.
    Rows,
}

struct Walk {
    mode: Mode,
    reads: Vec<SettingRead>,
}

/// The `current_setting()` calls of the backing view `view_oid` and of every view
/// it reads, in the order found, duplicates kept.
///
/// # Errors
/// The error opening a view.
pub fn reads(view_oid: Oid) -> TViewResult<Vec<SettingRead>> {
    let mut walk = Walk {
        mode: Mode::Value,
        reads: Vec::new(),
    };
    // SAFETY: `view_query` returns a copy in the current memory context; the
    // walk only reads it.
    unsafe {
        let query = view_query(view_oid)?;
        rows_of(query, &mut walk)?;
    }
    Ok(walk.reads)
}

/// Refuse a definition whose rows a setting decides; the names it reads for a
/// value, each once, in the order found.
///
/// # Errors
/// [`TViewError::DefinitionRefused`] (`0A000`) naming the first such setting;
/// the error opening a view.
pub fn check(view_oid: Oid) -> TViewResult<Vec<String>> {
    let reads = reads(view_oid)?;
    if let Some(read) = reads.iter().find(|r| r.decides_rows) {
        let named = read.name.as_deref().map_or_else(
            || "a setting it names by an expression".to_string(),
            |n| format!("the setting {n}"),
        );
        return Err(TViewError::DefinitionRefused {
            reason: format!(
                "the definition decides which rows it holds on {named} (current_setting() \
                 in a WHERE, ON, HAVING, LIMIT or a FROM function): every refresh runs as the \
                 TVIEW's owner, where a setting has the server's value, not a session's, so the \
                 TVIEW would hold the rows of that value only. Keep every row and filter where \
                 tv_* is read (a WHERE, or a row security policy on tv_*)"
            ),
        });
    }
    let mut names: Vec<String> = Vec::new();
    for name in reads.into_iter().filter_map(|r| r.name) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

/// Walk `query`, whose rows are (a source of) the TVIEW's rows: its filters
/// decide them, its output only computes values; the views, subqueries and
/// common table expressions it reads in `FROM` are walked the same way.
///
/// SAFETY: `query` is a valid query tree.
unsafe fn rows_of(query: *mut pg_sys::Query, walk: &mut Walk) -> TViewResult<()> {
    crate::utils::guard_depth();
    // SAFETY: a valid query tree, read only.
    unsafe {
        let deciding: [*mut pg_sys::Node; 4] = [
            (*query).jointree.cast(),
            (*query).havingQual,
            (*query).limitOffset,
            (*query).limitCount,
        ];
        for node in deciding {
            expressions(node, Mode::Rows, walk)?;
        }
        expressions((*query).targetList.cast(), Mode::Value, walk)?;
        expressions((*query).windowClause.cast(), Mode::Value, walk)?;
        for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
            match (*rte).rtekind {
                pg_sys::RTEKind::RTE_SUBQUERY => rows_of((*rte).subquery, walk)?,
                pg_sys::RTEKind::RTE_RELATION
                    if pg_sys::get_rel_relkind((*rte).relid).cast_unsigned()
                        == pg_sys::RELKIND_VIEW =>
                {
                    rows_of(view_query((*rte).relid)?, walk)?;
                }
                // A set-returning function's arguments decide how many rows it
                // returns.
                pg_sys::RTEKind::RTE_FUNCTION => {
                    expressions((*rte).functions.cast(), Mode::Rows, walk)?;
                }
                pg_sys::RTEKind::RTE_TABLEFUNC => {
                    expressions((*rte).tablefunc.cast(), Mode::Rows, walk)?;
                }
                pg_sys::RTEKind::RTE_VALUES => {
                    expressions((*rte).values_lists.cast(), Mode::Value, walk)?;
                }
                // walk: n/a(a join's or a CTE reference's expressions are walked
                // where they are defined; the others hold none)
                _ => {}
            }
        }
        for cte in elements::<pg_sys::CommonTableExpr>((*query).cteList) {
            rows_of((*cte).ctequery.cast(), walk)?;
        }
    }
    Ok(())
}

/// Walk the expressions `node` holds in `mode`: a subquery inside them, and the
/// views it reads, wholly so.
///
/// SAFETY: `node` is null or a valid node.
unsafe fn expressions(node: *mut pg_sys::Node, mode: Mode, walk: &mut Walk) -> TViewResult<()> {
    if node.is_null() {
        return Ok(());
    }
    let mut inner = Inner {
        walk: Walk {
            mode,
            reads: Vec::new(),
        },
        error: None,
    };
    // SAFETY: `inner` outlives the walk, which only reads `node`.
    unsafe { walker(node, std::ptr::from_mut(&mut inner).cast()) };
    walk.reads.append(&mut inner.walk.reads);
    inner.error.map_or(Ok(()), Err)
}

struct Inner {
    walk: Walk,
    error: Option<TViewError>,
}

/// The setting named by the first argument of a `current_setting()` call, `None`
/// when it is not a constant.
///
/// SAFETY: `args` is the call's argument list.
unsafe fn constant_name(args: *mut pg_sys::List) -> Option<String> {
    // SAFETY: a valid list; a Const of type text holds a text datum.
    unsafe {
        let first = *elements::<pg_sys::Node>(args).first()?;
        if tag(first) != Some(pg_sys::NodeTag::T_Const) {
            return None;
        }
        let value = first.cast::<pg_sys::Const>();
        if (*value).constisnull || (*value).consttype != pg_sys::TEXTOID {
            return None;
        }
        Some(cstr(pg_sys::text_to_cstring(
            (*value).constvalue.cast_mut_ptr::<pg_sys::text>(),
        )))
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn walker(node: *mut pg_sys::Node, ctx: *mut std::ffi::c_void) -> bool {
    crate::utils::guard_depth();
    // SAFETY: `ctx` is the Inner passed by `expressions`; `node` is valid.
    unsafe {
        let inner = &mut *ctx.cast::<Inner>();
        if inner.error.is_some() {
            return true;
        }
        match tag(node) {
            None => false,
            Some(pg_sys::NodeTag::T_Query) => {
                let query = node.cast::<pg_sys::Query>();
                for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                    if (*rte).rtekind == pg_sys::RTEKind::RTE_RELATION
                        && pg_sys::get_rel_relkind((*rte).relid).cast_unsigned()
                            == pg_sys::RELKIND_VIEW
                    {
                        match view_query((*rte).relid) {
                            Ok(view) => {
                                if walker(view.cast(), ctx) {
                                    return true;
                                }
                            }
                            Err(error) => {
                                inner.error = Some(error);
                                return true;
                            }
                        }
                    }
                }
                pg_sys::query_tree_walker(query, Some(walker), ctx, 0)
            }
            Some(pg_sys::NodeTag::T_FuncExpr) => {
                let call = node.cast::<pg_sys::FuncExpr>();
                let funcid = (*call).funcid.to_u32();
                if funcid == pg_sys::F_CURRENT_SETTING_TEXT
                    || funcid == pg_sys::F_CURRENT_SETTING_TEXT_BOOL
                {
                    inner.walk.reads.push(SettingRead {
                        name: constant_name((*call).args),
                        decides_rows: inner.walk.mode == Mode::Rows,
                    });
                }
                pg_sys::expression_tree_walker(node, Some(walker), ctx)
            }
            // walk: leaf(descend) the calls are found in the children
            Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
        }
    }
}
