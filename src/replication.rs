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
use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet};

/// Catalog facts about one TVIEW, read in a single query.
pub struct TviewRelation {
    pub entity: String,
    pub schema: String,
    pub table: String,
    /// The backing view, schema-qualified and quoted.
    pub view: String,
    pub unlogged: bool,
    /// The `tv_*` table's OID, whose owner reads the backing view.
    pub table_oid: pg_sys::Oid,
}

impl TviewRelation {
    /// Every registered TVIEW (or only `entity`), ordered by entity.
    pub fn load(entity: Option<&str>) -> TViewResult<Vec<Self>> {
        let query = format!(
            "SELECT m.entity, n.nspname::text AS schema, t.relname::text AS tbl, \
                    pg_catalog.quote_ident(vn.nspname) || '.' || pg_catalog.quote_ident(v.relname) \
                        AS view, \
                    t.relpersistence = 'u' AS unlogged, \
                    m.table_oid::oid AS table_oid \
             FROM {} m \
             JOIN pg_class t ON t.oid = m.table_oid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_class v ON v.oid = m.view_oid \
             JOIN pg_namespace vn ON vn.oid = v.relnamespace \
             WHERE $1::text IS NULL OR m.entity = $1 \
             ORDER BY m.entity",
            crate::utils::meta_table()
        );
        Spi::connect(|client| {
            let args = [crate::utils::spi::text(entity)];
            let mut out = Vec::new();
            for row in client.select(&query, None, &args)? {
                out.push(Self {
                    entity: row["entity"].value()?.unwrap_or_default(),
                    schema: row["schema"].value()?.unwrap_or_default(),
                    table: row["tbl"].value()?.unwrap_or_default(),
                    view: row["view"].value()?.unwrap_or_default(),
                    unlogged: row["unlogged"].value()?.unwrap_or(false),
                    table_oid: row["table_oid"].value()?.unwrap_or(pg_sys::InvalidOid),
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
    /// view still has rows: the state after promotion or a crash restart. The
    /// view is read as the TVIEW's owner: it may call the owner's functions.
    pub fn needs_rebuild(&self) -> TViewResult<bool> {
        if !self.unlogged || !self.table_is_empty()? {
            return Ok(false);
        }
        let _owner = crate::owner::AsOwner::of_table(self.table_oid)?;
        Ok(!has_no_rows(&self.view)?)
    }
}

/// The TVIEWs reset to empty after promotion or a crash restart: UNLOGGED, empty,
/// and their backing view has rows, or reads the table of a TVIEW that needs a
/// rebuild (its view is empty until that one is filled). Found dependencies
/// first.
fn needing_rebuild(relations: &[TviewRelation]) -> TViewResult<HashSet<String>> {
    let graph = crate::queue::graph::EntityDepGraph::load()?;
    let order = dependencies_first(&graph.children);
    let mut sorted: Vec<&TviewRelation> = relations.iter().collect();
    sorted.sort_by_key(|rel| {
        order
            .iter()
            .position(|e| e == &rel.entity)
            .unwrap_or(usize::MAX)
    });
    let mut needing = HashSet::new();
    for rel in sorted {
        let reads_one = graph
            .children
            .get(&rel.entity)
            .is_some_and(|deps| deps.iter().any(|d| needing.contains(d)));
        if rel.unlogged && ((reads_one && rel.table_is_empty()?) || rel.needs_rebuild()?) {
            needing.insert(rel.entity.clone());
        }
    }
    Ok(needing)
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
fn pg_tviews_is_replica_readable(entity: &str) -> Result<Option<bool>, ErrorReport> {
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
    ErrorReport,
> {
    let recovering = in_recovery();
    let relations = TviewRelation::load(None)?;
    let needing = if recovering {
        HashSet::new()
    } else {
        needing_rebuild(&relations)?
    };
    let mut rows = Vec::new();
    for rel in relations {
        let readable = !rel.unlogged;
        let is_empty = if recovering && rel.unlogged {
            None
        } else {
            Some(rel.table_is_empty()?)
        };
        let needs_rebuild = (!recovering).then(|| needing.contains(&rel.entity));
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
) -> Result<TableIterator<'static, (name!(entity, String), name!(rows, i64))>, ErrorReport> {
    crate::revision::check();
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
    let needing = if only_empty {
        needing_rebuild(&relations)?
    } else {
        HashSet::new()
    };
    let mut targets: Vec<TviewRelation> = relations
        .into_iter()
        .filter(|rel| !only_empty || needing.contains(&rel.entity))
        .collect();
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
        if only_empty {
            // Known empty: fill without TRUNCATE, so readers are not blocked.
            crate::admin::fill_empty_tview(&rel.entity)?;
        } else {
            // Every target is rebuilt, dependencies first: no cascade needed.
            crate::admin::rebuild_one(&rel.entity)?;
        }
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
    // A TVIEW reading one just filled, but not itself empty, is refreshed here.
    crate::admin::flush_after_rebuilds()?;
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
fn pg_tviews_set_logged(entity: &str, logged: bool) -> Result<(), ErrorReport> {
    crate::revision::check();
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
    crate::utils::spi_run_ddl(&sql)
        .map_err(|error| TViewError::SpiError { query: sql, error }.into())
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
