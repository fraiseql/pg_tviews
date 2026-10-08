//! Aggregate TVIEWs (issue #58): an entity with no `tb_<entity>` whose rows are the
//! `GROUP BY` groups of its source tables.
//!
//! The caller names, for each source table, the column whose value **is** the group
//! key (`group_keys`). Each becomes a local path from that table straight to the
//! entity, so the ordinary row trigger enqueues the affected groups (both the old and
//! the new group when a row moves) and the ordinary pk refresh recomputes them from
//! the backing view: a new group is inserted, an emptied one deleted.

use crate::catalog::plan::LocalPath;
use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::collections::BTreeMap;

/// Source table name → the column whose value is the group key.
pub type GroupKeys = BTreeMap<String, String>;

/// One local path per group key: a change to a row of `table` refreshes the
/// group named by that row's `column`.
///
/// # Errors
/// Returns an error if a named table is not a source of the view or lacks the column.
pub fn local_paths(
    entity: &str,
    group_keys: &GroupKeys,
    base_tables: &[Oid],
    view_oid: Oid,
) -> TViewResult<Vec<LocalPath>> {
    let mut paths = Vec::with_capacity(group_keys.len());
    for (table, column) in group_keys {
        let oid =
            source_table_oid(table, base_tables)?.ok_or_else(|| TViewError::InvalidInput {
                parameter: "group_keys".to_string(),
                reason: format!("'{table}' is not a table the definition reads"),
            })?;
        if !column_exists(oid, column)? {
            return Err(TViewError::InvalidInput {
                parameter: "group_keys".to_string(),
                reason: format!("table '{table}' has no column '{column}'"),
            });
        }
        paths.push(LocalPath {
            source_oid: oid,
            source_table: table.clone(),
            entity_name: entity.to_string(),
            initial_col: column.clone(),
            source_columns: crate::ddl::create::view_source_columns(view_oid, oid),
            root: false,
            initial_attnum: None,
        });
    }
    Ok(paths)
}

fn source_table_oid(table: &str, base_tables: &[Oid]) -> TViewResult<Option<Oid>> {
    let args = [
        crate::utils::spi::text(table),
        crate::utils::spi::oid_array(base_tables.to_vec()),
    ];
    Spi::connect(|client| {
        let mut rows = client.select(
            "SELECT oid FROM pg_class WHERE relname = $1 AND oid = ANY($2)",
            Some(1),
            &args,
        )?;
        match rows.next() {
            Some(row) => row.get::<Oid>(1),
            None => Ok(None),
        }
    })
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Resolve group key table '{table}'"),
        pg_error: e.to_string(),
    })
}

fn column_exists(table: Oid, column: &str) -> TViewResult<bool> {
    let args = [
        crate::utils::spi::oid(table),
        crate::utils::spi::text(column),
    ];
    Spi::connect(|client| {
        client
            .select(
                "SELECT EXISTS (SELECT 1 FROM pg_attribute \
                 WHERE attrelid = $1 AND attname = $2 AND attnum > 0 AND NOT attisdropped)",
                Some(1),
                &args,
            )?
            .first()
            .get_one::<bool>()
    })
    .map(|b| b.unwrap_or(false))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check group key column '{column}'"),
        pg_error: e.to_string(),
    })
}
