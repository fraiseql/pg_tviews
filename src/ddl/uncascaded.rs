//! Base tables a TVIEW reads whose writes no cascade maps to its keys (issues
//! #157, #158): the tables its lineage classifies `all_keys` (ADR 0157). They are
//! reported when the TVIEW is registered, and `pg_tviews.uncascaded_policy`
//! decides what a write to one of them does.

use crate::config::UncascadedPolicy;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// A base table no cascade reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UncascadedTable {
    pub oid: Oid,
    /// Schema-qualified, quoted name.
    pub name: String,
    /// How the view reads it, when known.
    pub reason: String,
}

/// How the trigger and the flush learn what a TVIEW reached: the tables and the
/// stored policy.
#[derive(Debug, Clone)]
pub(crate) struct Uncascaded {
    pub tables: Vec<UncascadedTable>,
    pub policy: UncascadedPolicy,
}

impl Uncascaded {
    pub(crate) fn oids(&self) -> Vec<Oid> {
        self.tables.iter().map(|t| t.oid).collect()
    }
}

/// `writes to a, b will not refresh tv (a: reason; b: reason)`.
fn describe(tview: &str, tables: &[UncascadedTable], verb: &str) -> String {
    let names = tables
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let reasons = match tables {
        [one] => one.reason.clone(),
        _ => tables
            .iter()
            .map(|t| format!("{}: {}", t.name, t.reason))
            .collect::<Vec<_>>()
            .join("; "),
    };
    format!("writes to {names} {verb} {tview} ({reasons})")
}

/// Report the uncascaded tables of `tview` (schema-qualified) under `policy`:
/// a WARNING, a NOTICE, or an ERROR that aborts the create.
///
/// # Errors
/// Returns an error under the `error` policy when `tables` is not empty.
pub(crate) fn report(
    tview: &str,
    tables: &[UncascadedTable],
    policy: UncascadedPolicy,
) -> TViewResult<()> {
    if tables.is_empty() {
        return Ok(());
    }
    match policy {
        UncascadedPolicy::Warn => {
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_WARNING,
                describe(tview, tables, "will not refresh"),
                function_name!(),
            )
            .set_hint(
                "Set pg_tviews.uncascaded_policy to 'full_refresh' before creating the TVIEW \
                 to refresh it in full on such writes, or to 'error' to refuse it.",
            )
            .report(PgLogLevel::WARNING);
            Ok(())
        }
        UncascadedPolicy::FullRefresh => {
            notice!("{}", describe(tview, tables, "will refresh all rows of"));
            Ok(())
        }
        UncascadedPolicy::Error => Err(TViewError::InvalidInput {
            parameter: "tview definition".to_string(),
            reason: format!(
                "{}. Set pg_tviews.uncascaded_policy to 'warn' or 'full_refresh' to create it anyway",
                describe(tview, tables, "would not refresh")
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_lists_tables_and_reasons() {
        let t = |n: &str, r: &str| UncascadedTable {
            oid: Oid::INVALID,
            name: n.to_string(),
            reason: r.to_string(),
        };
        assert_eq!(
            describe("public.tv_o", &[t("public.a", "x")], "will not refresh"),
            "writes to public.a will not refresh public.tv_o (x)"
        );
        assert_eq!(
            describe(
                "public.tv_o",
                &[t("public.a", "x"), t("public.b", "y")],
                "will not refresh"
            ),
            "writes to public.a, public.b will not refresh public.tv_o (public.a: x; public.b: y)"
        );
    }
}
