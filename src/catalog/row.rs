//! Single values of one TVIEW's catalog row, for the callers that need one of
//! them rather than the decoded [`super::TviewMeta`].

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::collections::HashSet;

fn read_error(what: &str, entity: &str, e: &impl std::fmt::Display) -> TViewError {
    TViewError::CatalogError {
        operation: format!("Read the {what} of TVIEW {entity}"),
        pg_error: e.to_string(),
    }
}

/// Whether a TVIEW has the entity `entity`.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn exists(entity: &str) -> TViewResult<bool> {
    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE entity = $1)",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map(|e| e == Some(true))
    .map_err(|e| read_error("registration", entity, &e))
}

/// The stored (normalized) definition of `entity`'s TVIEW.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn definition(entity: &str) -> TViewResult<Option<String>> {
    Spi::get_one_with_args::<String>(
        &format!(
            "SELECT definition FROM {} WHERE entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| read_error("definition", entity, &e))
}

/// The stored `group_keys` of `entity`'s TVIEW, as stored (`None` for a plain
/// TVIEW).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn group_keys(entity: &str) -> TViewResult<Option<serde_json::Value>> {
    Spi::get_one_with_args::<pgrx::JsonB>(
        &format!(
            "SELECT group_keys FROM {} WHERE entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map(|j| j.map(|j| j.0))
    .map_err(|e| read_error("group keys", entity, &e))
}

/// The GraphQL type name declared for `entity`'s TVIEW; `None` for
/// `PascalCase(entity)`.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn typename(entity: &str) -> TViewResult<Option<String>> {
    Spi::get_one_with_args::<String>(
        &format!(
            "SELECT graphql_typename FROM {} WHERE entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| read_error("GraphQL type name", entity, &e))
}

/// Store the GraphQL type name of `entity`'s TVIEW (`None`: `PascalCase(entity)`).
///
/// # Errors
/// Returns an error if the catalog cannot be written.
pub fn set_typename(entity: &str, typename: Option<&str>) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET graphql_typename = $2 WHERE entity = $1",
            super::meta_table()
        ),
        &[
            crate::utils::spi::text(entity),
            crate::utils::spi::text(typename),
        ],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Store the GraphQL type name", &e))
}

/// Whether `entity`'s definition reads the current time.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn time_dependent(entity: &str) -> TViewResult<bool> {
    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT time_dependent FROM {} WHERE entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map(|t| t == Some(true))
    .map_err(|e| read_error("time dependence", entity, &e))
}

/// Clear `needs_reregister` of `entity`'s TVIEW, re-registered by this release.
///
/// # Errors
/// Returns an error if the catalog cannot be written.
pub fn clear_needs_reregister(entity: &str) -> TViewResult<()> {
    let _owner = crate::owner::AsOwner::of_extension()?;
    Spi::run_with_args(
        &format!(
            "UPDATE {} SET needs_reregister = false WHERE entity = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::text(entity)],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Clear needs_reregister of TVIEW {entity}"),
        pg_error: e.to_string(),
    })
}

/// The tables of every TVIEW.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn table_oids() -> TViewResult<HashSet<Oid>> {
    Ok(crate::utils::spi::oids(
        &format!(
            "SELECT table_oid::pg_catalog.oid FROM {}",
            super::meta_table()
        ),
        &[],
    )?
    .into_iter()
    .collect())
}

/// The backing view of the TVIEW whose table is `table`, if it is one.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn view_of_table(table: Oid) -> TViewResult<Option<Oid>> {
    crate::utils::spi::one::<Oid>(
        &format!(
            "SELECT m.view_oid::pg_catalog.oid FROM {} m \
             WHERE m.table_oid::pg_catalog.oid = $1",
            super::meta_table()
        ),
        &[crate::utils::spi::oid(table)],
    )
}

/// How many TVIEWs are registered, how many need re-registering, and how many
/// lost their table.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn counts() -> TViewResult<(i64, i64, i64)> {
    let (total, stale, orphaned) = Spi::get_three::<i64, i64, i64>(&format!(
        "SELECT pg_catalog.count(*), \
                pg_catalog.count(*) FILTER (WHERE m.needs_reregister), \
                pg_catalog.count(*) FILTER (WHERE NOT EXISTS \
                    (SELECT 1 FROM pg_catalog.pg_class c WHERE c.oid = m.table_oid)) \
         FROM {} m",
        super::meta_table()
    ))
    .map_err(|e| crate::utils::spi::catalog_error("Count the TVIEWs", &e))?;
    Ok((
        total.unwrap_or(0),
        stale.unwrap_or(0),
        orphaned.unwrap_or(0),
    ))
}

/// A TVIEW as [`relations`] lists it.
pub struct Listed {
    pub entity: String,
    pub table: Oid,
    /// The table's schema and name; `None` once the table is gone.
    pub schema: Option<String>,
    pub name: Option<String>,
}

/// Every TVIEW of the database.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn relations() -> TViewResult<Vec<Listed>> {
    Spi::connect(|client| {
        let mut out = Vec::new();
        for row in client.select(
            &format!(
                "SELECT m.entity::pg_catalog.text, m.table_oid::pg_catalog.oid, \
                        n.nspname::pg_catalog.text, c.relname::pg_catalog.text \
                 FROM {} m \
                 LEFT JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
                 LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 ORDER BY m.entity",
                super::meta_table()
            ),
            None,
            &[],
        )? {
            out.push(Listed {
                entity: row.get::<String>(1)?.unwrap_or_default(),
                table: row.get::<Oid>(2)?.unwrap_or(Oid::INVALID),
                schema: row.get::<String>(3)?,
                name: row.get::<String>(4)?,
            });
        }
        Ok::<_, pgrx::spi::Error>(out)
    })
    .map_err(|e| crate::utils::spi::catalog_error("List the TVIEWs", &e))
}
