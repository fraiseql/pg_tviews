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
use crate::utils::ident;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Whether this transaction cross-checks (REPEATABLE READ).
#[must_use]
pub fn enabled() -> bool {
    Policy::current() == Policy::FailFast
}

/// Refresh side: the rows of `meta` named by `keys` are the same under the latest
/// snapshot as under the transaction's, which the refresh computed them from.
/// Both are read from the backing view with one kept plan, so only what the
/// snapshots see can differ, never how the rows are computed (duplicate UNION
/// keys, row order inside an aggregate, a direct patch). A TVIEW that reads the
/// current time is left alone: its rows differ between any two computations.
///
/// # Errors
/// Returns an error if a query fails; a difference raises 40001.
pub fn refreshed_rows(meta: &TviewMeta, keys: &[KeyValue]) -> TViewResult<()> {
    if keys.is_empty() || !enabled() || meta.time_dependent {
        return Ok(());
    }
    let columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?
        .iter()
        .map(|c| format!("v.{}", ident::quoted(c)))
        .collect::<Vec<_>>()
        .join(", ");
    let key_type = meta.key_type()?;
    let key = ident::quoted(&meta.identity.column);
    let view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let sql = format!(
        "SELECT v.{key}::pg_catalog.text, ROW({columns})::pg_catalog.text FROM {view} v \
         WHERE v.{key} OPERATOR(pg_catalog.=) ANY ({})",
        crate::refresh::key_cast(&key_type, "$1", true)
    );
    let args = [crate::refresh::key_array(&key_type, keys)?];
    let seen = sorted(crate::utils::spi::kept_rows(&sql, &args)?);
    let latest = sorted(Spi::connect(|_| {
        crate::utils::spi::latest_rows_connected(&sql, &args, true)
    })?);
    if seen != latest {
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
