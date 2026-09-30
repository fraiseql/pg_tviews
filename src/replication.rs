//! Physical replication and UNLOGGED TVIEWs.
//!
//! An UNLOGGED `tv_*` table (the default, `pg_tviews.unlogged_by_default`) is
//! not WAL-logged: a hot standby refuses to read it, and promotion or a crash
//! restart resets it to its empty init fork. These functions let clients see
//! which TVIEWs a standby can serve, let deploy tooling rebuild the emptied
//! ones in dependency order, and switch a TVIEW between LOGGED and UNLOGGED.
//! The startup worker in [`crate::rebuild_worker`] calls
//! [`pg_tviews_rebuild_all`] once recovery has finished.

use crate::error::{TViewError, TViewResult};
use crate::utils::quote_identifier;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet};

/// Catalog facts about one TVIEW, read in a single query.
pub struct TviewRelation {
    pub entity: String,
    pub schema: String,
    pub table: String,
    pub view: String,
    pub unlogged: bool,
}

impl TviewRelation {
    /// Every registered TVIEW (or only `entity`), ordered by entity.
    pub fn load(entity: Option<&str>) -> TViewResult<Vec<Self>> {
        const QUERY: &str = "SELECT m.entity, n.nspname::text AS schema, t.relname::text AS tbl, \
                    v.relname::text AS view, t.relpersistence = 'u' AS unlogged \
             FROM pg_tview_meta m \
             JOIN pg_class t ON t.oid = m.table_oid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_class v ON v.oid = m.view_oid \
             WHERE $1::text IS NULL OR m.entity = $1 \
             ORDER BY m.entity";
        Spi::connect(|client| {
            // SAFETY: the text datum borrows `entity`, which outlives the select.
            let args = [unsafe {
                DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            }];
            let mut out = Vec::new();
            for row in client.select(QUERY, None, &args)? {
                out.push(Self {
                    entity: row["entity"].value()?.unwrap_or_default(),
                    schema: row["schema"].value()?.unwrap_or_default(),
                    table: row["tbl"].value()?.unwrap_or_default(),
                    view: row["view"].value()?.unwrap_or_default(),
                    unlogged: row["unlogged"].value()?.unwrap_or(false),
                });
            }
            Ok::<_, spi::Error>(out)
        })
        .map_err(|e| TViewError::CatalogError {
            operation: "Load TVIEW relations".to_string(),
            pg_error: e.to_string(),
        })
    }

    fn qualified(&self, name: &str) -> String {
        format!(
            "{}.{}",
            quote_identifier(&self.schema),
            quote_identifier(name)
        )
    }

    /// Whether the `tv_*` table has no rows.
    pub fn table_is_empty(&self) -> TViewResult<bool> {
        has_no_rows(&self.qualified(&self.table))
    }

    /// Whether the table is an UNLOGGED TVIEW reset to empty while its backing
    /// view still has rows: the state after promotion or a crash restart.
    pub fn needs_rebuild(&self) -> TViewResult<bool> {
        Ok(self.unlogged && self.table_is_empty()? && !has_no_rows(&self.qualified(&self.view))?)
    }
}

/// Read-only on purpose (`select`, not `Spi::get_one`, which assigns a
/// transaction id): it must run on a hot standby.
fn has_no_rows(qualified: &str) -> TViewResult<bool> {
    let sql = format!("SELECT NOT EXISTS (SELECT 1 FROM {qualified}) AS empty");
    Spi::connect(|client| {
        let rows = client.select(&sql, Some(1), &[])?;
        rows.first().get_one::<bool>()
    })
    .map(|v| v.unwrap_or(true))
    .map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })
}

fn in_recovery() -> bool {
    // SAFETY: RecoveryInProgress only reads shared memory state.
    unsafe { pg_sys::RecoveryInProgress() }
}

/// Whether a hot standby can read `tv_<entity>`: true for a LOGGED table, false
/// for an UNLOGGED one, NULL for an unknown entity.
///
/// # Errors
/// Returns an error if the catalog query fails.
#[pg_extern]
fn pg_tviews_is_replica_readable(entity: &str) -> Result<Option<bool>, TViewError> {
    Ok(TviewRelation::load(Some(entity))?
        .first()
        .map(|r| !r.unlogged))
}

/// Replication state of every TVIEW, safe to call on a standby.
///
/// `is_empty` and `needs_rebuild` are NULL for an UNLOGGED TVIEW during
/// recovery, where its table cannot be read; `needs_rebuild` is NULL for every
/// TVIEW during recovery.
///
/// # Errors
/// Returns an error if a catalog query or an emptiness probe fails.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_replication_status() -> Result<
    TableIterator<
        'static,
        (
            name!(entity, String),
            name!(persistence, String),
            name!(replica_readable, bool),
            name!(is_empty, Option<bool>),
            name!(needs_rebuild, Option<bool>),
        ),
    >,
    TViewError,
> {
    let recovering = in_recovery();
    let mut rows = Vec::new();
    for rel in TviewRelation::load(None)? {
        let readable = !rel.unlogged;
        let is_empty = if recovering && rel.unlogged {
            None
        } else {
            Some(rel.table_is_empty()?)
        };
        let needs_rebuild = if recovering {
            None
        } else {
            Some(rel.needs_rebuild()?)
        };
        let persistence = if rel.unlogged { "unlogged" } else { "logged" };
        rows.push((
            rel.entity,
            persistence.to_string(),
            readable,
            is_empty,
            needs_rebuild,
        ));
    }
    Ok(TableIterator::new(rows))
}

/// Rebuild TVIEWs from their backing views, dependencies first, and return
/// each rebuilt entity with its row count, in rebuild order.
///
/// With `only_empty` (the default) only UNLOGGED TVIEWs that are empty while
/// their backing view is not are rebuilt: run it after a promotion, a crash
/// restart or a restore. With `only_empty => false` every TVIEW is rebuilt.
///
/// # Errors
/// Returns an error during recovery, or if a catalog query or a refresh fails.
#[pg_extern]
#[allow(clippy::type_complexity)] // Reason: pgrx TableIterator row type spells out the columns
fn pg_tviews_rebuild_all(
    only_empty: default!(bool, true),
) -> Result<TableIterator<'static, (name!(entity, String), name!(rows, i64))>, TViewError> {
    Ok(TableIterator::new(rebuild_all(only_empty)?))
}

/// See [`pg_tviews_rebuild_all`].
///
/// # Errors
/// Returns an error during recovery, or if a catalog query or a refresh fails.
pub fn rebuild_all(only_empty: bool) -> TViewResult<Vec<(String, i64)>> {
    if in_recovery() {
        return Err(TViewError::InvalidInput {
            parameter: "pg_tviews_rebuild_all".to_string(),
            reason: "cannot rebuild TVIEWs during recovery; run it on the primary \
                     (or after promotion)"
                .to_string(),
        });
    }

    let relations = TviewRelation::load(None)?;
    let mut targets = Vec::new();
    for rel in relations {
        if !only_empty || rel.needs_rebuild()? {
            targets.push(rel);
        }
    }
    if targets.is_empty() {
        return Ok(Vec::new());
    }

    // A TVIEW whose backing view reads another TVIEW is rebuilt after it.
    let graph = crate::queue::graph::EntityDepGraph::load()?;
    let order = dependencies_first(&graph.children);
    targets.sort_by_key(|rel| {
        order
            .iter()
            .position(|e| e == &rel.entity)
            .unwrap_or(usize::MAX)
    });

    let mut rebuilt = Vec::with_capacity(targets.len());
    for rel in targets {
        const REFRESH: &str = "SELECT pg_tviews_refresh($1)";
        // SAFETY: the text datum borrows `rel.entity`, which outlives the call.
        let args = [unsafe {
            DatumWithOid::new(
                rel.entity.as_str(),
                PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value(),
            )
        }];
        Spi::run_with_args(REFRESH, &args).map_err(|e| TViewError::SpiError {
            query: REFRESH.to_string(),
            error: e.to_string(),
        })?;
        crate::queue::mark_crash_recovery_checked(&rel.entity);
        let count_sql = format!("SELECT count(*) FROM {}", rel.qualified(&rel.table));
        let rows = Spi::get_one::<i64>(&count_sql)
            .map_err(|e| TViewError::SpiError {
                query: count_sql,
                error: e.to_string(),
            })?
            .unwrap_or(0);
        rebuilt.push((rel.entity, rows));
    }
    Ok(rebuilt)
}

/// Entities ordered so that each comes after everything it depends on
/// (`depends_on[x]` lists the entities `x` reads), ties broken by name.
fn dependencies_first(depends_on: &HashMap<String, Vec<String>>) -> Vec<String> {
    fn visit(
        entity: &str,
        depends_on: &HashMap<String, Vec<String>>,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if !seen.insert(entity.to_string()) {
            return;
        }
        let mut deps = depends_on.get(entity).cloned().unwrap_or_default();
        deps.sort();
        for dep in &deps {
            visit(dep, depends_on, seen, out);
        }
        out.push(entity.to_string());
    }

    let mut roots: Vec<&String> = depends_on.keys().collect();
    roots.sort();
    let (mut seen, mut out) = (HashSet::new(), Vec::new());
    for entity in roots {
        visit(entity, depends_on, &mut seen, &mut out);
    }
    out
}

/// Switch `tv_<entity>` to LOGGED (`logged => true`, readable on standbys) or
/// back to UNLOGGED. `ALTER TABLE … SET [UN]LOGGED` rewrites the whole table
/// under an ACCESS EXCLUSIVE lock.
///
/// # Errors
/// Returns an error if the entity is unknown or the `ALTER TABLE` fails.
#[pg_extern]
fn pg_tviews_set_logged(entity: &str, logged: bool) -> Result<(), TViewError> {
    let rel = TviewRelation::load(Some(entity))?
        .into_iter()
        .next()
        .ok_or_else(|| TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;
    let persistence = if logged { "LOGGED" } else { "UNLOGGED" };
    let sql = format!(
        "ALTER TABLE {} SET {persistence}",
        rel.qualified(&rel.table)
    );
    crate::utils::spi_run_ddl(&sql).map_err(|error| TViewError::SpiError { query: sql, error })
}

#[cfg(test)]
mod tests {
    use super::dependencies_first;
    use std::collections::HashMap;

    #[test]
    fn dependencies_come_first() {
        let depends_on = HashMap::from([
            (
                "comment".to_string(),
                vec!["post".to_string(), "user".to_string()],
            ),
            ("post".to_string(), vec!["user".to_string()]),
        ]);
        assert_eq!(dependencies_first(&depends_on), ["user", "post", "comment"]);
    }
}
