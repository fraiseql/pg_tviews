//! Health checks, diagnostics, and performance monitoring.

use pgrx::prelude::*;

/// One row of `pg_tviews_health_check()`: status, component, message, severity.
type Check = (String, String, String, String);

fn ok(component: &str, message: impl Into<String>) -> Check {
    ("OK".into(), component.into(), message.into(), "info".into())
}

fn warning(component: &str, message: impl Into<String>) -> Check {
    (
        "WARNING".into(),
        component.into(),
        message.into(),
        "warning".into(),
    )
}

fn error(component: &str, message: impl Into<String>) -> Check {
    (
        "ERROR".into(),
        component.into(),
        message.into(),
        "error".into(),
    )
}

/// Health check function for production monitoring
///
/// Returns a comprehensive health status including:
/// - Extension version
/// - `jsonb_delta` availability
/// - Catalog revision and metadata consistency
/// - Orphaned and missing triggers
/// - TVIEW count
#[pg_extern]
fn pg_tviews_health_check() -> TableIterator<
    'static,
    (
        name!(status, String),
        name!(component, String),
        name!(message, String),
        name!(severity, String),
    ),
> {
    let mut results = vec![
        ok(
            "extension",
            format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
        ),
        jsonb_delta_check(),
    ];
    let (catalog, unversioned) = catalog_check();
    results.push(catalog);
    // A 0.1.0 catalog has none of the tables the other checks read.
    if !unversioned {
        results.extend([
            metadata_check(),
            reregister_check(),
            trigger_check(),
            count_check(),
        ]);
    }
    TableIterator::new(results)
}

fn jsonb_delta_check() -> Check {
    let installed =
        Spi::get_one::<bool>("SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_delta'")
            .unwrap_or(Some(false))
            .unwrap_or(false);
    if installed {
        ok(
            "jsonb_delta",
            "jsonb_delta extension available (optimized mode)",
        )
    } else {
        warning(
            "jsonb_delta",
            "jsonb_delta not installed (falling back to standard JSONB)",
        )
    }
}

/// Whether this library matches the installed SQL, and whether the
/// catalog is a 0.1.0 one.
fn catalog_check() -> (Check, bool) {
    match crate::revision::installed() {
        crate::revision::Installed::Matches => (
            ok(
                "catalog",
                format!(
                    "catalog revision {} matches the library",
                    crate::revision::CATALOG_REVISION
                ),
            ),
            false,
        ),
        crate::revision::Installed::Differs(revision) => (
            error(
                "catalog",
                format!(
                    "library catalog revision {} does not match the installed extension \
                     ({revision}): {}",
                    crate::revision::CATALOG_REVISION,
                    crate::revision::remedy(revision)
                ),
            ),
            false,
        ),
        crate::revision::Installed::Unversioned => (
            error(
                "catalog",
                "the installed extension is a 0.1.0 catalog: run scripts/migrate-from-0.1.0.sql",
            ),
            true,
        ),
    }
}

fn metadata_check() -> Check {
    let orphaned = Spi::get_one::<i64>(&format!(
        "SELECT COUNT(*) FROM {} m
         WHERE NOT EXISTS (SELECT 1 FROM pg_class WHERE oid = m.table_oid)",
        crate::utils::meta_table()
    ))
    .unwrap_or(Some(0))
    .unwrap_or(0);
    if orphaned > 0 {
        error(
            "metadata",
            format!("{orphaned} orphaned metadata entries found"),
        )
    } else {
        ok("metadata", "All metadata entries valid")
    }
}

/// TVIEWs registered before a release that changed what registration derives.
fn reregister_check() -> Check {
    let stale = Spi::connect(|client| {
        client
            .select(
                &format!(
                    "SELECT count(*) FROM {} WHERE needs_reregister",
                    crate::utils::meta_table()
                ),
                None,
                &[],
            )?
            .first()
            .get_one::<i64>()
    })
    .ok()
    .flatten()
    .unwrap_or(0);
    if stale == 0 {
        ok("reregister", "No TVIEW needs re-registration")
    } else {
        warning(
            "reregister",
            format!(
                "{stale} TVIEW{} registered by an older release: run \
                 SELECT * FROM tviews.pg_tviews_reregister_all()",
                if stale == 1 { "" } else { "s" }
            ),
        )
    }
}

/// `pg_tviews`' triggers against the tables the TVIEWs read.
fn trigger_check() -> Check {
    match crate::dependency::triggers::trigger_problems() {
        Ok(p) if p.orphaned.is_empty() && p.missing.is_empty() && p.untagged.is_empty() => {
            ok("triggers", "All triggers properly linked")
        }
        Ok(p) => {
            let parts: Vec<String> = [
                (&p.orphaned, "orphaned trigger", "found"),
                (
                    &p.missing,
                    "missing trigger",
                    "(run pg_tviews_reregister_all())",
                ),
                (
                    &p.untagged,
                    "trigger without an entity",
                    "(run pg_tviews_reregister_all())",
                ),
            ]
            .into_iter()
            .filter(|(list, _, _)| !list.is_empty())
            .map(|(list, what, action)| {
                format!("{} {action}: {}", count(list.len(), what), sample(list))
            })
            .collect();
            warning("triggers", parts.join("; "))
        }
        Err(e) => error("triggers", format!("could not check triggers: {e}")),
    }
}

fn count_check() -> Check {
    let tviews = Spi::get_one::<i64>(&format!(
        "SELECT COUNT(*) FROM {}",
        crate::utils::meta_table()
    ))
    .unwrap_or(Some(0))
    .unwrap_or(0);
    ok("tviews", format!("{tviews} TVIEWs registered"))
}

/// Get current queue statistics
/// Returns metrics about the current transaction's refresh operations
#[pg_extern]
fn pg_tviews_queue_stats() -> pgrx::JsonB {
    let stats = crate::metrics::metrics_api::get_queue_stats();

    let json_value = serde_json::json!({
        "queue_size": stats.queue_size,
        "flushes": stats.flushes,
        "total_refreshes": stats.total_refreshes,
        "total_iterations": stats.total_iterations,
        "max_iterations": stats.max_iterations,
        "total_timing_ms": stats.total_timing_ms(),
        "graph_cache_hit_rate": stats.graph_cache_hit_rate(),
        "table_cache_hit_rate": stats.table_cache_hit_rate(),
        "graph_cache_hits": stats.graph_cache_hits,
        "graph_cache_misses": stats.graph_cache_misses,
        "table_cache_hits": stats.table_cache_hits,
        "table_cache_misses": stats.table_cache_misses,
        "direct_patch_captured": stats.direct_patch_captured,
        "direct_patches_applied": stats.direct_patches_applied,
        "direct_patch_fallbacks": stats.direct_patch_fallbacks,
        "view_recomputes": stats.view_recomputes,
        "refresh_noop_skipped": stats.refresh_noop_skipped,
        "catalog_lookups": stats.catalog_lookups,
        "propagation_pruned": stats.propagation_pruned
    });

    pgrx::JsonB(json_value)
}

/// Debug function: View current queue contents
/// Returns the entities and PKs currently in the refresh queue
#[pg_extern]
fn pg_tviews_debug_queue() -> pgrx::JsonB {
    let contents = crate::metrics::metrics_api::get_queue_contents();

    let json_contents: Vec<serde_json::Value> = contents
        .into_iter()
        .map(|key| {
            serde_json::json!({
                "entity": key.entity,
                "pk": match key.key {
                    crate::queue::key::KeyValue::Int(v) => serde_json::Value::from(v),
                    crate::queue::key::KeyValue::Text(v) => serde_json::Value::from(v),
                }
            })
        })
        .collect();

    pgrx::JsonB(serde_json::json!(json_contents))
}

/// Size, row count and index count of each TVIEW, largest first.
///
/// The row count is a `count(*)` of each TVIEW run with the caller's privileges;
/// a TVIEW the caller cannot read gets a NULL count and a NOTICE.
#[pg_extern]
fn pg_tviews_performance_stats() -> TableIterator<
    'static,
    (
        name!(entity, String),
        name!(table_size, String),
        name!(total_size, String),
        name!(row_count, Option<i64>),
        name!(index_count, i32),
    ),
> {
    // The TVIEW table is read through its catalog OID, so any schema works.
    let query = format!(
        "SELECT m.entity, m.table_oid::pg_catalog.regclass::pg_catalog.text AS tview,
                pg_catalog.pg_size_pretty(pg_catalog.pg_relation_size(m.table_oid)) AS table_size,
                pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size(m.table_oid))
                    AS total_size,
                (SELECT pg_catalog.count(*)::int FROM pg_catalog.pg_index
                 WHERE indrelid = m.table_oid) AS index_count
         FROM {} m
         ORDER BY pg_catalog.pg_relation_size(m.table_oid) DESC",
        crate::utils::meta_table()
    );
    let tviews = Spi::connect(|client| {
        let mut out = Vec::new();
        for row in client.select(&query, None, &[])? {
            out.push((
                row["entity"].value::<String>()?.unwrap_or_default(),
                row["tview"].value::<String>()?.unwrap_or_default(),
                row["table_size"].value::<String>()?.unwrap_or_default(),
                row["total_size"].value::<String>()?.unwrap_or_default(),
                row["index_count"].value::<i32>()?.unwrap_or(0),
            ));
        }
        Ok::<_, spi::Error>(out)
    })
    .unwrap_or_else(|e| error!("pg_tviews: could not read the TVIEW list: {e}"));

    let stats: Vec<_> = tviews
        .into_iter()
        .map(|(entity, tview, table_size, total_size, index_count)| {
            let row_count = row_count(&tview);
            (entity, table_size, total_size, row_count, index_count)
        })
        .collect();
    TableIterator::new(stats)
}

/// `count(*)` of `tview` (a regclass text, already quoted), or `None` with a
/// NOTICE when the caller may not read it.
fn row_count(tview: &str) -> Option<i64> {
    let readable = Spi::get_one_with_args::<bool>(
        "SELECT pg_catalog.has_table_privilege($1::pg_catalog.regclass, 'SELECT')",
        &[crate::utils::spi::text(tview)],
    );
    if readable != Ok(Some(true)) {
        notice!("pg_tviews: no row count for {tview}: permission denied");
        return None;
    }
    Spi::get_one::<i64>(&format!("SELECT pg_catalog.count(*) FROM {tview}"))
        .unwrap_or_else(|e| error!("pg_tviews: could not count the rows of {tview}: {e}"))
}

/// `1 orphaned trigger`, `2 orphaned triggers`.
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The first ten items, and how many more there are.
fn sample(items: &[String]) -> String {
    const SHOWN: usize = 10;
    let text = items
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        format!("{text} and {} more", items.len() - SHOWN)
    } else {
        text
    }
}
