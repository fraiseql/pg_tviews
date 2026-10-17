//! The indexes `pg_tviews` created on a TVIEW's table and still owns, recorded by
//! name in `pg_tview_meta.managed_index_names`.
//!
//! Only what `pg_tviews` created is recorded: a replace leaves out, and the
//! `data_gin_index` option drops, only these. Everything else on the table is the
//! user's. The record is written as the extension's owner; callers check the
//! caller's right to change the TVIEW.

use crate::error::TViewResult;
use pgrx::pg_sys;

/// Change the record of the TVIEW whose table is `table` with `sql`, which reads
/// the table as `$1` and the names as `$2`.
fn update(table: pg_sys::Oid, sql: &str, names: &[String]) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    crate::utils::spi::run(
        &format!(
            "UPDATE {} SET managed_index_names = {sql} \
             WHERE table_oid = $1::pg_catalog.oid::pg_catalog.regclass",
            crate::catalog::meta_table()
        ),
        &[
            crate::utils::spi::oid(table),
            crate::utils::spi::text_array_of(names),
        ],
    )
}

/// Add `names` to the record, which stays sorted and without duplicates (and is
/// written, if it never was, even with no names).
pub(crate) fn record(table: pg_sys::Oid, names: &[String]) -> TViewResult<()> {
    update(
        table,
        "ARRAY(SELECT DISTINCT n FROM pg_catalog.unnest(\
             pg_catalog.array_cat(COALESCE(managed_index_names, '{}'), $2)) n ORDER BY n)",
        names,
    )
}

/// Remove `names` from the record.
pub(crate) fn forget(table: pg_sys::Oid, names: &[String]) -> TViewResult<()> {
    if names.is_empty() {
        return Ok(());
    }
    update(
        table,
        "ARRAY(SELECT n FROM pg_catalog.unnest(managed_index_names) n \
               WHERE n <> ALL ($2) ORDER BY n)",
        names,
    )
}

/// Record that the managed index `old` is now named `new`.
pub(crate) fn rename(table: pg_sys::Oid, old: &str, new: &str) -> TViewResult<()> {
    if recorded(table)?.iter().any(|n| n == old) {
        forget(table, &[old.to_string()])?;
        record(table, &[new.to_string()])?;
    }
    Ok(())
}

/// The recorded names, sorted; empty for a table that is no TVIEW's.
pub(crate) fn recorded(table: pg_sys::Oid) -> TViewResult<Vec<String>> {
    crate::utils::spi::strings(
        &format!(
            "SELECT n FROM {} m, pg_catalog.unnest(m.managed_index_names) n \
             WHERE m.table_oid = $1::pg_catalog.oid::pg_catalog.regclass ORDER BY n",
            crate::catalog::meta_table()
        ),
        &[crate::utils::spi::oid(table)],
    )
}

/// Whether the TVIEW's record was never written: a TVIEW registered by a release
/// that did not record its indexes, being re-registered by the upgrade.
pub(crate) fn unrecorded(table: pg_sys::Oid) -> TViewResult<bool> {
    Ok(crate::utils::spi::one::<bool>(
        &format!(
            "SELECT managed_index_names IS NULL FROM {} \
             WHERE table_oid = $1::pg_catalog.oid::pg_catalog.regclass",
            crate::catalog::meta_table()
        ),
        &[crate::utils::spi::oid(table)],
    )?
    .unwrap_or(false))
}

/// The `indexes` option `entity` declared, as stored (`[]` when none).
///
/// # Errors
/// A failed catalog read.
pub(crate) fn declared(entity: &str) -> TViewResult<serde_json::Value> {
    let stored = crate::utils::spi::one::<pgrx::JsonB>(
        &format!(
            "SELECT declared_indexes FROM {} WHERE entity = $1",
            crate::catalog::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )?;
    Ok(stored.map_or_else(|| serde_json::Value::Array(Vec::new()), |s| s.0))
}

/// Store the `indexes` option `entity` declares.
///
/// # Errors
/// A failed catalog write.
pub(crate) fn declare(entity: &str, indexes: serde_json::Value) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    crate::utils::spi::run(
        &format!(
            "UPDATE {} SET declared_indexes = $2 WHERE entity = $1",
            crate::catalog::meta_table()
        ),
        &[
            crate::utils::spi::text(entity),
            crate::utils::spi::jsonb(pgrx::JsonB(indexes)),
        ],
    )
}

/// `WHERE` conditions over `pg_index i` / `pg_class ic` that select the indexes a
/// user added to the TVIEW's table `$1`: every index that backs no constraint and
/// that `pg_tviews` did not create.
pub(crate) fn user_index_filter() -> String {
    format!(
        "i.indrelid = $1 \
         AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
                         WHERE k.conindid = i.indexrelid) \
         AND ic.relname <> ALL (SELECT pg_catalog.unnest(m.managed_index_names) \
                                FROM {} m WHERE m.table_oid = $1::pg_catalog.oid::pg_catalog.regclass)",
        crate::catalog::meta_table()
    )
}
