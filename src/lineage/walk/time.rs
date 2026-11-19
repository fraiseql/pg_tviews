//! How a level reads the current time, or its reader.

use super::{Oid, TViewError, TViewResult, Walker, list_len, pg_guard, pg_sys, tag};

impl Walker<'_> {
    /// Record the time this level reads: `CURRENT_DATE` and the other SQL
    /// value functions of the time, and the `pg_catalog` functions that return
    /// the current time. Its rows then change with no write.
    ///
    /// # Errors
    /// [`TViewError::DefinitionRefused`] when the level reads who reads it:
    /// `CURRENT_USER`, `CURRENT_ROLE`, `USER` (D8: a document whose content
    /// depends on its reader cannot be the same for every reader of its row).
    ///
    /// SAFETY: `query` is the valid Query of the innermost level.
    pub(super) unsafe fn note_time(&mut self, query: *mut pg_sys::Node) -> TViewResult<()> {
        let mut found = Vec::new();
        // SAFETY: a read-only walk of a valid Query.
        unsafe { collect_time(query, &mut found) };
        for node in found {
            let reader = match node {
                TimeNode::Value(op) => reader_value_name(op),
                TimeNode::Function(funcid, _) => match self.function(funcid).0.as_str() {
                    "pg_catalog.current_user" | "pg_catalog.\"current_user\"" => {
                        Some("current_user")
                    }
                    // walk: n/a(another function names no reader)
                    _ => None,
                },
                // walk: n/a(a cast names no reader)
                TimeNode::Cast(_) => None,
            };
            if let Some(reader) = reader {
                return Err(TViewError::DefinitionRefused {
                    reason: format!(
                        "the definition reads {reader}: a TVIEW's rows are the same for every \
                         reader, and its refresh runs as its owner. Filter what each role reads \
                         with a row security policy on the TVIEW's table instead"
                    ),
                });
            }
            let name = match node {
                TimeNode::Value(op) => time_value_name(op).map(str::to_string),
                TimeNode::Function(funcid, args) => {
                    let (name, _) = self.function(funcid);
                    match name.strip_prefix("pg_catalog.") {
                        Some(
                            f @ ("now"
                            | "clock_timestamp"
                            | "statement_timestamp"
                            | "transaction_timestamp"
                            | "timeofday"),
                        ) => Some(format!("{f}()")),
                        Some("age") if args == 1 => Some("age()".to_string()),
                        // walk: n/a(another pg_catalog function reads no time; one outside it
                        // is untracked, note_functions)
                        _ => None,
                    }
                }
                // A text read as a date or a time may be 'now', 'today'…
                TimeNode::Cast(typid) => time_type_name(typid).map(|t| format!("a cast to {t}")),
            };
            if let Some(name) = name
                && !self.graph.time_reads.contains(&name)
            {
                self.graph.time_reads.push(name);
            }
        }
        Ok(())
    }
}

/// How a SQL value function naming the reader is written, `None` for another
/// one. `SESSION_USER` names the session, not the role reading: not one.
const fn reader_value_name(op: pg_sys::SQLValueFunctionOp::Type) -> Option<&'static str> {
    use pg_sys::SQLValueFunctionOp as Op;
    match op {
        Op::SVFOP_CURRENT_USER => Some("CURRENT_USER"),
        Op::SVFOP_CURRENT_ROLE => Some("CURRENT_ROLE"),
        Op::SVFOP_USER => Some("USER"),
        // walk: n/a(another SQL value function names no reader)
        _ => None,
    }
}

/// A node of a query level that may read the current time.
#[derive(Debug, Clone, Copy)]
pub(super) enum TimeNode {
    /// A `SQLValueFunction`, by its op.
    Value(pg_sys::SQLValueFunctionOp::Type),
    /// A function call: the function and its argument count.
    Function(Oid, usize),
    /// A cast through the input function of a type (`CoerceViaIO`), by the
    /// type cast to.
    Cast(Oid),
}

/// How a date or time type is written, `None` for another type: its input
/// function reads the current time for `now`, `today`…
const fn time_type_name(typid: Oid) -> Option<&'static str> {
    match typid {
        pg_sys::DATEOID => Some("date"),
        pg_sys::TIMEOID => Some("time"),
        pg_sys::TIMETZOID => Some("timetz"),
        pg_sys::TIMESTAMPOID => Some("timestamp"),
        pg_sys::TIMESTAMPTZOID => Some("timestamptz"),
        // walk: n/a(the input of another type reads no clock)
        _ => None,
    }
}

/// How a SQL value function of the time is written, `None` for another one
/// (`CURRENT_USER`…).
pub(super) const fn time_value_name(op: pg_sys::SQLValueFunctionOp::Type) -> Option<&'static str> {
    use pg_sys::SQLValueFunctionOp as Op;
    match op {
        Op::SVFOP_CURRENT_DATE => Some("CURRENT_DATE"),
        Op::SVFOP_CURRENT_TIME | Op::SVFOP_CURRENT_TIME_N => Some("CURRENT_TIME"),
        Op::SVFOP_CURRENT_TIMESTAMP | Op::SVFOP_CURRENT_TIMESTAMP_N => Some("CURRENT_TIMESTAMP"),
        Op::SVFOP_LOCALTIME | Op::SVFOP_LOCALTIME_N => Some("LOCALTIME"),
        Op::SVFOP_LOCALTIMESTAMP | Op::SVFOP_LOCALTIMESTAMP_N => Some("LOCALTIMESTAMP"),
        // walk: n/a(another SQL value function, CURRENT_USER and the like, reads no time)
        _ => None,
    }
}

/// The SQL value functions and function calls of a query level's own
/// expressions, not those of nested subqueries.
///
/// SAFETY: `node` is a valid Query.
pub(super) unsafe fn collect_time(node: *mut pg_sys::Node, out: &mut Vec<TimeNode>) {
    #[pg_guard]
    unsafe extern "C-unwind" fn walker(
        node: *mut pg_sys::Node,
        ctx: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `ctx` is the Vec passed below; `node` is valid.
        unsafe {
            let out = &mut *ctx.cast::<Vec<TimeNode>>();
            match tag(node) {
                None => false,
                Some(pg_sys::NodeTag::T_SQLValueFunction) => {
                    out.push(TimeNode::Value(
                        (*node.cast::<pg_sys::SQLValueFunction>()).op,
                    ));
                    false
                }
                Some(pg_sys::NodeTag::T_FuncExpr) => {
                    let f = node.cast::<pg_sys::FuncExpr>();
                    out.push(TimeNode::Function((*f).funcid, list_len((*f).args)));
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                Some(pg_sys::NodeTag::T_CoerceViaIO) => {
                    out.push(TimeNode::Cast(
                        (*node.cast::<pg_sys::CoerceViaIO>()).resulttype,
                    ));
                    pg_sys::expression_tree_walker(node, Some(walker), ctx)
                }
                Some(pg_sys::NodeTag::T_SubLink | pg_sys::NodeTag::T_Query) => false,
                // walk: leaf(descend) nothing of its own: its children are walked
                Some(_) => pg_sys::expression_tree_walker(node, Some(walker), ctx),
            }
        }
    }
    // SAFETY: walk the level's expressions; subqueries record their own.
    unsafe {
        pg_sys::query_tree_walker(
            node.cast(),
            Some(walker),
            std::ptr::from_mut(out).cast(),
            (pg_sys::QTW_IGNORE_RT_SUBQUERIES | pg_sys::QTW_IGNORE_CTE_SUBQUERIES).cast_signed(),
        );
    }
}
