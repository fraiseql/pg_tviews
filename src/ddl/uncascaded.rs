//! Base tables a TVIEW reads whose writes no cascade maps to its keys (issues
//! #157, #158): the tables its lineage classifies `all_keys` (ADR 0157). They are
//! reported when the TVIEW is registered, and its `uncascaded_policy` decides
//! what a write to one of them does: refused at create by default.

use crate::config::UncascadedPolicy;
use crate::error::TViewResult;
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

/// What to write to declare the policy of `tview`: the option of
/// `pg_tviews_create_or_replace()`, or the setting `CREATE TABLE … AS` and
/// `pg_tviews_create()` read.
fn how_to_declare(tview: &str, policy: &str) -> String {
    format!(
        "pg_tviews_create_or_replace('{tview}', <definition>, options => \
         '{{\"uncascaded_policy\": \"{policy}\"}}'); before CREATE TABLE … AS or \
         pg_tviews_create(): SET pg_tviews.uncascaded_policy = '{policy}'"
    )
}

/// Report the uncascaded tables of `tview` (schema-qualified) under `policy`:
/// a WARNING, a NOTICE, or an ERROR that aborts the create and says what to
/// declare instead.
///
/// # Errors
/// Never returns one: under the `error` policy the ERROR is raised here.
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
            .set_hint(format!(
                "To refresh it in full on such writes instead: {}.",
                how_to_declare(tview, "full_refresh")
            ))
            .report(PgLogLevel::WARNING);
            Ok(())
        }
        UncascadedPolicy::FullRefresh => {
            notice!("{}", describe(tview, tables, "will refresh all rows of"));
            Ok(())
        }
        UncascadedPolicy::Error => {
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                format!(
                    "{}: declare what such a write does with the TVIEW's uncascaded_policy",
                    describe(tview, tables, "would not refresh")
                ),
                function_name!(),
            )
            .set_hint(format!(
                "To refresh {tview} in full on such writes: {}. \"warn\" accepts stale \
                 rows instead. Or join the tables on a column pg_tviews can trace.",
                how_to_declare(tview, "full_refresh")
            ))
            .report(PgLogLevel::ERROR);
            Ok(())
        }
    }
}

/// After `REFRESH MATERIALIZED VIEW matview`: refresh in full every TVIEW that
/// reads it under the `full_refresh` policy, then flush the queue, as the flush
/// trigger does after a write (#189). Under `warn` the TVIEW was created knowing
/// it would go stale; under `error` it was never created.
///
/// # Errors
/// Returns an error if the catalog cannot be read or a refresh fails.
pub(crate) fn refresh_readers_of(matview: Oid) -> TViewResult<()> {
    // SAFETY: a plain OID datum.
    let args = [unsafe {
        pgrx::datum::DatumWithOid::new(matview, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value())
    }];
    let entities: Vec<String> = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT entity::text FROM {} \
                     WHERE uncascaded_policy = 'full_refresh' \
                       AND $1::pg_catalog.regclass = ANY (uncascaded_oids) \
                     ORDER BY entity",
                    crate::utils::meta_table()
                ),
                None,
                &args,
            )?
            .map(|row| row.get::<String>(1))
            .collect::<Result<Vec<_>, _>>()
            .map(|entities| entities.into_iter().flatten().collect())
    })
    .map_err(|e| crate::TViewError::CatalogError {
        operation: "Find the TVIEWs reading a refreshed materialized view".to_string(),
        pg_error: e.to_string(),
    })?;
    if entities.is_empty() {
        return Ok(());
    }
    for entity in &entities {
        if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
            crate::suspend::record_change(entity);
        } else {
            crate::queue::enqueue_refresh_all(entity);
        }
    }
    crate::queue::flush_refresh_queue()
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
