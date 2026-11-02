//! Aggregate TVIEWs: an entity with no `tb_<entity>` whose rows are the
//! `GROUP BY` groups of its source tables.
//!
//! The caller names, for each source table, the column whose value **is** the group
//! key (`group_keys`). The analysis finds them from the query tree (each table
//! whose column equals `pk_<entity>`), and every declared key must be among them;
//! it may find more (a table read in a subquery correlated on the key). Each is a
//! local path
//! from that table straight to the entity, so the ordinary row trigger enqueues the
//! affected groups (both the old and the new group when a row moves) and the
//! ordinary pk refresh recomputes them from the backing view: a new group is
//! inserted, an emptied one deleted.

use crate::error::{TViewError, TViewResult};
use crate::lineage::plan::LocalPath;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::collections::BTreeMap;

/// Source table name → the column whose value is the group key.
pub type GroupKeys = BTreeMap<String, String>;

/// Check the declared group keys against the paths the analysis found (each
/// table whose column equals the group key, from the query tree): every declared
/// key must be one of them. The analysis may find more (a table read in a
/// subquery correlated on the key): they are kept, each refreshing its groups.
///
/// # Errors
/// [`TViewError::InvalidInput`] naming a declared key the definition does not
/// group by.
pub fn check_group_keys(
    entity: &str,
    group_keys: &GroupKeys,
    paths: &[LocalPath],
) -> TViewResult<()> {
    let found: Vec<(&str, &str)> = paths
        .iter()
        .map(|p| (p.source_table.as_str(), p.initial_col.as_str()))
        .collect();
    match group_keys
        .iter()
        .find(|(table, column)| !found.contains(&(table.as_str(), column.as_str())))
    {
        Some((table, column)) => Err(TViewError::InvalidInput {
            parameter: "group_keys".to_string(),
            reason: if let Some(path) = paths.iter().find(|p| &p.source_table == table)
                && !column_exists(path.source_oid, column)?
            {
                format!("table '{table}' has no column '{column}'")
            } else if found.iter().any(|(t, _)| t == table) {
                format!(
                    "tv_{entity} does not group by {table}.{column}: the definition's group \
                     key comes from {}",
                    list(&found)
                )
            } else {
                format!(
                    "'{table}' is not a table the definition reads its group key from: the \
                     definition's group key comes from {}",
                    list(&found)
                )
            },
        }),
        None => Ok(()),
    }
}

fn list(found: &[(&str, &str)]) -> String {
    found
        .iter()
        .map(|(table, column)| format!("{table}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn column_exists(table: Oid, column: &str) -> TViewResult<bool> {
    Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_attribute \
         WHERE attrelid = $1 AND attname = $2 AND attnum > 0 AND NOT attisdropped)",
        &[
            crate::utils::spi::oid(table),
            crate::utils::spi::text(column),
        ],
    )
    .map(|b| b.unwrap_or(false))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check group key column '{column}'"),
        pg_error: e.to_string(),
    })
}
