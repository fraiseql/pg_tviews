//! What a definition may depend on besides the rows it reads (dialect.md, Axes C
//! and D): two evaluations of it over the same rows return the same rows. Every
//! level of the backing view's query and of every view it reads is walked, and
//! the first node that breaks the rule refuses the definition:
//!
//! - a system column that changes with no write `pg_tviews` sees (`ctid`, `xmax`,
//!   `cmin`, `cmax`; `xmin` and `tableoid` change only with a write, and are read
//!   with no column skip, `wholerow`);
//! - a volatile function, wherever it is called (the output depends on it, or on
//!   which rows a filter keeps), the operators and casts it backs included;
//! - a `pg_catalog` function that reads the session, the transaction, the server
//!   or the statistics instead of the rows ([`session_reader`]), and
//!   `SESSION_USER`;
//! - `TABLESAMPLE`.
//!
//! Functions outside `pg_catalog` that are not immutable are also governed by
//! `function_reads` (the tables they read); their bodies are not visible.

use super::{Oid, cstr, elements, index, pg_guard, pg_sys, tag, view_query};
use crate::error::{TViewError, TViewResult};

/// The walk's state: the query levels it is in, innermost last, the views to walk
/// next, and the first node found outside the dialect.
struct Walk {
    levels: Vec<*mut pg_sys::Query>,
    views: Vec<Oid>,
    refused: Option<String>,
}

/// Refuse a definition whose rows may differ between two evaluations over the
/// same rows: the backing view `view_oid` and every view it reads.
///
/// # Errors
/// [`TViewError::DefinitionRefused`] (`0A000`) naming the first construct found;
/// the error opening a view.
pub fn check(view_oid: Oid) -> TViewResult<()> {
    let mut walk = Walk {
        levels: Vec::new(),
        views: vec![view_oid],
        refused: None,
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
        if let Some(reason) = walk.refused {
            return Err(TViewError::DefinitionRefused { reason });
        }
    }
    Ok(())
}

impl Walk {
    /// The system column `var` reads, when it changes with no write.
    ///
    /// SAFETY: `var` is a valid Var of the innermost level's query tree.
    unsafe fn system_column(&self, var: *const pg_sys::Var) -> Option<String> {
        // SAFETY: a valid Var, and the range table of the level it names.
        unsafe {
            let attno = i32::from((*var).varattno);
            let name = match attno {
                pg_sys::SelfItemPointerAttributeNumber => "ctid",
                pg_sys::MinCommandIdAttributeNumber => "cmin",
                pg_sys::MaxTransactionIdAttributeNumber => "xmax",
                pg_sys::MaxCommandIdAttributeNumber => "cmax",
                // walk: n/a(a user column, the whole row, xmin or tableoid: wholerow reads them)
                _ => return None,
            };
            let up = index((*var).varlevelsup);
            let query = *self
                .levels
                .len()
                .checked_sub(up + 1)
                .and_then(|i| self.levels.get(i))?;
            let rtable = elements::<pg_sys::RangeTblEntry>((*query).rtable);
            let rte = *index((*var).varno)
                .checked_sub(1)
                .and_then(|i| rtable.get(i))?;
            let table = cstr(pg_sys::format_type_be_qualified(pg_sys::get_rel_type_id(
                (*rte).relid,
            )));
            Some(format!(
                "the definition reads the system column {name} of {table}, which changes with \
                 no write pg_tviews sees (VACUUM FULL, CLUSTER, a row lock): read the key \
                 instead, or xmin for a row version"
            ))
        }
    }
}

/// How the function `funcid` breaks the dialect, `None` when it does not.
fn function_refusal(funcid: Oid) -> Option<String> {
    // SAFETY: catalog lookups by OID.
    let (schema, name, volatile) = unsafe {
        (
            cstr(pg_sys::get_namespace_name(pg_sys::get_func_namespace(
                funcid,
            ))),
            cstr(pg_sys::get_func_name(funcid)),
            pg_sys::func_volatile(funcid) == pg_sys::PROVOLATILE_VOLATILE.cast_signed(),
        )
    };
    let builtin = schema == "pg_catalog";
    let shown = if builtin {
        format!("{name}()")
    } else {
        // Under the pinned `pg_catalog, pg_temp` path: the function qualified, its
        // built-in argument types not. SAFETY: a catalog lookup by OID; the string
        // is copied.
        cstr(unsafe { pg_sys::format_procedure(funcid) })
    };
    if volatile {
        return Some(format!(
            "the definition calls {shown}, which is volatile: two evaluations over the same \
             rows may differ, and pg_tviews stores one. Compute the value when the row is \
             written and store it in a column instead"
        ));
    }
    let reads = builtin.then(|| session_reader(&name)).flatten()?;
    Some(format!(
        "the definition calls {shown}, which reads {reads}, not the rows: its value changes \
         with no write, and no refresh follows it"
    ))
}

/// What the `pg_catalog` function `name` reads besides its arguments that changes
/// with no write and that no frame pins, `None` for the others: immutable
/// functions, and stable ones whose settings the owner frame pins
/// (`docs/reference/owner-frame.md`). Volatile functions are refused by their
/// volatility. `current_user()` is refused as a reader of its reader (D8).
pub(crate) fn session_reader(name: &str) -> Option<&'static str> {
    let class = match name {
        "system_user"
        | "getpgusername"
        | "inet_client_addr"
        | "inet_client_port"
        | "inet_server_addr"
        | "inet_server_port"
        | "pg_backend_pid"
        | "pg_client_encoding"
        | "pg_my_temp_schema"
        | "pg_is_other_temp_schema"
        | "pg_trigger_depth"
        | "pg_listening_channels"
        | "pg_cursor"
        | "pg_prepared_statement"
        | "pg_show_all_settings" => "the session",
        "txid_current"
        | "txid_current_if_assigned"
        | "txid_current_snapshot"
        | "pg_current_xact_id"
        | "pg_current_xact_id_if_assigned"
        | "pg_current_snapshot"
        | "mxid_age" => "the transaction",
        "pg_postmaster_start_time"
        | "pg_conf_load_time"
        | "pg_relation_filenode"
        | "pg_relation_filepath"
        | "pg_filenode_relation"
        | "pg_column_toast_chunk_id"
        | "pg_get_replication_slots"
        | "pg_available_extensions"
        | "pg_available_extension_versions"
        | "pg_extension_update_paths"
        | "pg_timezone_names"
        | "pg_timezone_abbrevs_abbrevs"
        | "pg_timezone_abbrevs_zone" => "the server",
        "pg_has_role" | "row_security_active" => "the privileges of the role evaluating it",
        // walk: R a privilege check: refused
        _ if name.starts_with("has_") && name.ends_with("_privilege") => {
            "the privileges of the role evaluating it"
        }
        // walk: R a statistics reader: refused
        _ if name.starts_with("pg_stat_") => "the statistics",
        // walk: R a reader of relations it does not name: refused
        _ if [
            "table_to_",
            "schema_to_",
            "database_to_",
            "query_to_",
            "cursor_to_",
        ]
        .iter()
        .any(|p| name.starts_with(p)) =>
        {
            "relations it does not name"
        }
        // walk: n/a(a built-in that reads only its arguments and pinned settings)
        _ => return None,
    };
    Some(class)
}

#[pg_guard]
unsafe extern "C-unwind" fn check_function(funcid: Oid, ctx: *mut std::ffi::c_void) -> bool {
    // SAFETY: `ctx` is the Walk passed through `walker`.
    let walk = unsafe { &mut *ctx.cast::<Walk>() };
    walk.refused = function_refusal(funcid);
    walk.refused.is_some()
}

#[pg_guard]
unsafe extern "C-unwind" fn walker(node: *mut pg_sys::Node, ctx: *mut std::ffi::c_void) -> bool {
    crate::utils::guard_depth();
    // SAFETY: `ctx` is the Walk passed by `check`; `node` is valid.
    unsafe {
        let walk = &mut *ctx.cast::<Walk>();
        if walk.refused.is_some() {
            return true;
        }
        match tag(node) {
            None => false,
            Some(pg_sys::NodeTag::T_Var) => {
                walk.refused = walk.system_column(node.cast::<pg_sys::Var>());
                walk.refused.is_some()
            }
            Some(pg_sys::NodeTag::T_SQLValueFunction) => {
                let op = (*node.cast::<pg_sys::SQLValueFunction>()).op;
                if op == pg_sys::SQLValueFunctionOp::SVFOP_SESSION_USER {
                    walk.refused = Some(
                        "the definition reads SESSION_USER, which names the session, not the \
                         rows: its value changes with no write, and no refresh follows it"
                            .to_string(),
                    );
                    return true;
                }
                // walk: n/a(the time and the reader are note_time's; CURRENT_CATALOG and
                // CURRENT_SCHEMA are pinned by the owner frame)
                false
            }
            Some(pg_sys::NodeTag::T_NextValueExpr) => {
                walk.refused = Some(
                    "the definition draws from a sequence: two evaluations differ".to_string(),
                );
                true
            }
            Some(pg_sys::NodeTag::T_Query) => {
                let query = node.cast::<pg_sys::Query>();
                for rte in elements::<pg_sys::RangeTblEntry>((*query).rtable) {
                    if (*rte).rtekind != pg_sys::RTEKind::RTE_RELATION {
                        continue;
                    }
                    if !(*rte).tablesample.is_null() {
                        walk.refused = Some(
                            "the definition reads a table with TABLESAMPLE: two evaluations \
                             sample other rows"
                                .to_string(),
                        );
                        return true;
                    }
                    if pg_sys::get_rel_relkind((*rte).relid).cast_unsigned() == pg_sys::RELKIND_VIEW
                    {
                        walk.views.push((*rte).relid);
                    }
                }
                walk.levels.push(query);
                let flags = pg_sys::QTW_IGNORE_JOINALIASES | pg_sys::QTW_IGNORE_GROUPEXPRS;
                let done = pg_sys::query_tree_walker(query, Some(walker), ctx, flags.cast_signed());
                walk.levels.pop();
                done
            }
            // walk: leaf(descend) every function the node calls is checked, then its children
            Some(_) => {
                pg_sys::check_functions_in_node(node, Some(check_function), ctx)
                    || pg_sys::expression_tree_walker(node, Some(walker), ctx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::session_reader;

    #[test]
    fn built_ins_reading_the_session_are_classified() {
        assert_eq!(session_reader("pg_backend_pid"), Some("the session"));
        assert_eq!(session_reader("txid_current"), Some("the transaction"));
        assert_eq!(
            session_reader("has_table_privilege"),
            Some("the privileges of the role evaluating it")
        );
        assert_eq!(
            session_reader("pg_stat_get_live_tuples"),
            Some("the statistics")
        );
        assert_eq!(
            session_reader("table_to_xml"),
            Some("relations it does not name")
        );
        assert_eq!(session_reader("upper"), None);
        assert_eq!(session_reader("to_char"), None);
        assert_eq!(session_reader("current_setting"), None);
        assert_eq!(session_reader("now"), None);
    }
}
