//! REPEATABLE READ cross-checks (ADR 0207). A transaction snapshot can't see what
//! committed after it, however long the transaction waits, so a refresh may
//! compute a row from data a concurrent transaction already changed, and a
//! write may miss a row a concurrent transaction already added. As PostgreSQL's
//! foreign-key checks do, both are compared with the latest snapshot; a
//! difference fails the transaction with 40001. Nothing runs under another
//! isolation level.

use super::Policy;
use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Whether this transaction cross-checks (REPEATABLE READ).
#[must_use]
pub fn enabled() -> bool {
    Policy::current() == Policy::FailFast
}

/// Refresh side: the rows of `meta` named by `keys`, as this transaction wrote
/// them, are what its backing view computes under the latest snapshot.
///
/// # Errors
/// Returns an error if a query fails; a difference raises 40001.
pub fn refreshed_rows(meta: &TviewMeta, keys: &[KeyValue]) -> TViewResult<()> {
    if keys.is_empty() || !enabled() {
        return Ok(());
    }
    let columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    let stored: std::collections::HashMap<String, String> =
        crate::utils::column_types(meta.tview_oid)?
            .into_iter()
            .collect();
    let key_type = meta.key_type()?;
    let key = quote_identifier(&meta.identity.column);
    let any = crate::refresh::key_cast(&key_type, "$1", true);
    // A stored column can have another type than the view's: compare the
    // view's value as the table stores it.
    let row = |alias: &str, cast: bool| {
        columns
            .iter()
            .map(|c| {
                let column = format!("{alias}.{}", quote_identifier(c));
                match stored.get(c) {
                    Some(ty) if cast => format!("{column}::{ty}"),
                    _ => column,
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let select = |relation: &str, alias: &str, cast: bool| {
        format!(
            "SELECT {alias}.{key}::pg_catalog.text, ROW({})::pg_catalog.text FROM {relation} {alias} \
             WHERE {alias}.{key} OPERATOR(pg_catalog.=) ANY ({any})",
            row(alias, cast)
        )
    };
    let tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let args = [crate::refresh::key_array(&key_type, keys)?];
    let written = sorted(crate::utils::spi::kept_rows(
        &select(&tv, "t", false),
        &args,
    )?);
    let latest = sorted(Spi::connect(|_| {
        crate::utils::spi::latest_rows_connected(&select(&view, "v", true), &args, true)
    })?);
    if written != latest {
        super::serialization_failure(
            "a TVIEW row this transaction refreshed changed in a concurrent transaction",
        );
    }
    Ok(())
}

pub use crate::utils::spi::TextRow;

/// Writer side: `found`, the rows a discovery query returned under the
/// transaction's snapshot, hold every row `latest` returned under the latest
/// snapshot.
pub fn discovered(found: &[TextRow], latest: &[TextRow]) {
    if missed(found, latest) {
        super::serialization_failure("a concurrent transaction added rows this write must refresh");
    }
}

/// Writer side, for a query run on its own: `sql` returns, under the latest
/// snapshot, no row missing from `found`.
///
/// # Errors
/// Returns an error if the query fails; a missed row raises 40001.
pub fn discovered_by(sql: &str, args: &[DatumWithOid<'_>], found: &[TextRow]) -> TViewResult<()> {
    if !enabled() {
        return Ok(());
    }
    let latest = Spi::connect(|_| crate::utils::spi::latest_rows_connected(sql, args, true))?;
    discovered(found, &latest);
    Ok(())
}

/// Whether `latest` has a row `found` lacks. A row whose first column is NULL
/// names nothing.
fn missed(found: &[TextRow], latest: &[TextRow]) -> bool {
    latest
        .iter()
        .filter(|row| row.first().is_some_and(Option::is_some))
        .any(|row| !found.contains(row))
}

fn sorted(mut rows: Vec<Vec<Option<String>>>) -> Vec<Vec<Option<String>>> {
    rows.sort();
    rows
}

#[cfg(test)]
mod tests {
    use super::missed;

    fn row(cells: &[&str]) -> Vec<Option<String>> {
        cells.iter().map(|c| Some((*c).to_string())).collect()
    }

    #[test]
    fn a_row_only_the_latest_snapshot_sees_is_missed() {
        let found = vec![row(&["1", "10"])];
        assert!(!missed(&found, &[row(&["1", "10"])]));
        assert!(missed(&found, &[row(&["1", "10"]), row(&["1", "11"])]));
        // Gone in the latest snapshot: deleted since, nothing to refresh.
        assert!(!missed(&found, &[]));
    }

    #[test]
    fn a_null_key_names_no_row() {
        assert!(!missed(&[], &[vec![None, Some("x".into())]]));
    }
}
