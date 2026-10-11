//! Physical replication and UNLOGGED TVIEWs.
//!
//! An UNLOGGED `tv_*` table (option `logged: false`) is
//! not WAL-logged: a hot standby refuses to read it, and promotion or a crash
//! restart resets it to its empty init fork. These functions let clients see
//! which TVIEWs a standby can serve, let deploy tooling rebuild the emptied
//! ones in dependency order, and switch a TVIEW between LOGGED and UNLOGGED.
//! The startup worker in [`crate::rebuild_worker`] calls
//! [`pg_tviews_rebuild_all`] once recovery has finished.

use crate::error::{TViewError, TViewResult};
use crate::utils::ident;
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet};

/// Catalog facts about one TVIEW, read in a single query.
pub struct TviewRelation {
    pub entity: String,
    pub schema: String,
    pub table: String,
    pub unlogged: bool,
    /// The `tv_*` table's OID, whose owner reads the backing view.
    pub table_oid: pg_sys::Oid,
}

impl TviewRelation {
    /// Every registered TVIEW (or only `entity`), ordered by entity.
    pub fn load(entity: Option<&str>) -> TViewResult<Vec<Self>> {
        let query = format!(
            "SELECT m.entity, n.nspname::text AS schema, t.relname::text AS tbl, \
                    t.relpersistence = 'u' AS unlogged, \
                    m.table_oid::oid AS table_oid \
             FROM {} m \
             JOIN pg_class t ON t.oid = m.table_oid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
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
        format!("{}.{}", ident::quoted(&self.schema), ident::quoted(name))
    }

    /// Whether the `tv_*` table has no rows.
    pub fn table_is_empty(&self) -> TViewResult<bool> {
        has_no_rows(&self.qualified(&self.table))
    }

    /// Whether the table is an UNLOGGED TVIEW that PostgreSQL reset (after
    /// promotion or a crash restart) and nothing filled since.
    pub fn needs_rebuild(&self) -> TViewResult<bool> {
        crate::lifecycle::validity::needs_fill(self.table_oid)
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

/// Replication state of every TVIEW, safe to call on a standby.
///
/// `is_empty` and `needs_rebuild` are NULL for an UNLOGGED TVIEW during
/// recovery, where its table cannot be read; `needs_rebuild` is NULL for every
/// TVIEW during recovery.
///
/// # Errors
/// Returns an error if a catalog query or an emptiness probe fails.
#[allow(clippy::type_complexity)] // Reason: one row of pg_tviews_replication_status()
pub(crate) fn replication_status()
-> TViewResult<Vec<(String, String, bool, Option<bool>, Option<bool>)>> {
    let recovering = in_recovery();
    let relations = TviewRelation::load(None)?;
    let mut rows = Vec::new();
    for rel in relations {
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
    Ok(rows)
}

/// Rebuild TVIEWs from their backing views, dependencies first, and return each
/// rebuilt entity with its row count, in rebuild order: only the UNLOGGED TVIEWs
/// PostgreSQL reset with `only_empty`, every TVIEW otherwise.
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

    // A TVIEW whose backing view reads another TVIEW is rebuilt after it.
    let mut targets = TviewRelation::load(None)?;
    let graph = crate::catalog::EntityDepGraph::load()?;
    let order = dependencies_first(&graph.children);
    targets.sort_by_key(|rel| {
        order
            .iter()
            .position(|e| e == &rel.entity)
            .unwrap_or(usize::MAX)
    });

    let mut rebuilt = Vec::new();
    for rel in targets {
        if only_empty {
            // Filled without TRUNCATE, so readers are not blocked; one another
            // transaction claimed is left to it.
            if !crate::lifecycle::validity::fill_if_reset(&rel.entity)? {
                continue;
            }
        } else {
            // Every target is rebuilt, dependencies first: no cascade needed.
            crate::refresh::full::rebuild_one(&rel.entity)?;
        }
        // Counted as its owner, as it was rebuilt: the caller may not read it.
        let _owner = crate::owner::AsOwner::of_entity(&rel.entity)?;
        let count_sql = format!("SELECT count(*) FROM {}", rel.qualified(&rel.table));
        let rows = Spi::get_one::<i64>(&count_sql)
            .map_err(|e| TViewError::SpiError {
                query: count_sql,
                error: e.to_string(),
            })?
            .unwrap_or(0);
        rebuilt.push((rel.entity, rows));
    }
    if rebuilt.is_empty() {
        return Ok(rebuilt);
    }
    // A TVIEW reading one just filled, but not itself reset, is refreshed here.
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
