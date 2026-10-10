//! Health checks, diagnostics, and performance monitoring.

use pgrx::prelude::*;

/// Health check function for production monitoring
///
/// Returns a comprehensive health status including:
/// - Extension version
/// - `jsonb_delta` availability
/// - Metadata consistency
/// - Orphaned triggers
/// - Queue status
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
    let mut results = Vec::new();

    // Check 1: Extension loaded
    results.push((
        "OK".to_string(),
        "extension".to_string(),
        format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
        "info".to_string(),
    ));

    // Check 2: jsonb_delta availability
    let has_jsonb_delta =
        Spi::get_one::<bool>("SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_delta'")
            .unwrap_or(Some(false))
            .unwrap_or(false);

    if has_jsonb_delta {
        results.push((
            "OK".to_string(),
            "jsonb_delta".to_string(),
            "jsonb_delta extension available (optimized mode)".to_string(),
            "info".to_string(),
        ));
    } else {
        results.push((
            "WARNING".to_string(),
            "jsonb_delta".to_string(),
            "jsonb_delta not installed (falling back to standard JSONB)".to_string(),
            "warning".to_string(),
        ));
    }

    // Catalog revision (issue #137): does this library match the installed SQL?
    let installed = crate::revision::installed();
    let unversioned = matches!(installed, crate::revision::Installed::Unversioned);
    results.push(match installed {
        crate::revision::Installed::Matches => (
            "OK".to_string(),
            "catalog".to_string(),
            format!(
                "catalog revision {} matches the library",
                crate::revision::CATALOG_REVISION
            ),
            "info".to_string(),
        ),
        crate::revision::Installed::Differs(revision) => (
            "ERROR".to_string(),
            "catalog".to_string(),
            format!(
                "library catalog revision {} does not match the installed extension ({revision}): \
                 {}",
                crate::revision::CATALOG_REVISION,
                crate::revision::remedy(revision)
            ),
            "error".to_string(),
        ),
        crate::revision::Installed::Unversioned => (
            "ERROR".to_string(),
            "catalog".to_string(),
            "the installed extension is a 0.1.0 catalog: run scripts/migrate-from-0.1.0.sql"
                .to_string(),
            "error".to_string(),
        ),
    });

    // A 0.1.0 catalog has none of the tables the other checks read.
    if unversioned {
        return TableIterator::new(results);
    }

    // Check 3: Metadata consistency
    let orphaned_meta = Spi::get_one::<i64>(&format!(
        "SELECT COUNT(*) FROM {} m
         WHERE NOT EXISTS (SELECT 1 FROM pg_class WHERE oid = m.table_oid)",
        crate::utils::meta_table()
    ))
    .unwrap_or(Some(0))
    .unwrap_or(0);

    if orphaned_meta > 0 {
        results.push((
            "ERROR".to_string(),
            "metadata".to_string(),
            format!("{orphaned_meta} orphaned metadata entries found"),
            "error".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "metadata".to_string(),
            "All metadata entries valid".to_string(),
            "info".to_string(),
        ));
    }

    // TVIEWs registered before a release that changed what registration derives.
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
    results.push(if stale == 0 {
        (
            "OK".to_string(),
            "reregister".to_string(),
            "No TVIEW needs re-registration".to_string(),
            "info".to_string(),
        )
    } else {
        (
            "WARNING".to_string(),
            "reregister".to_string(),
            format!(
                "{stale} TVIEW{} registered by an older release: run \
                 SELECT * FROM tviews.pg_tviews_reregister_all()",
                if stale == 1 { "" } else { "s" }
            ),
            "warning".to_string(),
        )
    });

    // Check 4: pg_tviews' triggers against the tables the TVIEWs read (issue #139).
    match crate::dependency::triggers::trigger_problems() {
        Ok(p) if p.orphaned.is_empty() && p.missing.is_empty() && p.untagged.is_empty() => {
            results.push((
                "OK".to_string(),
                "triggers".to_string(),
                "All triggers properly linked".to_string(),
                "info".to_string(),
            ));
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
            results.push((
                "WARNING".to_string(),
                "triggers".to_string(),
                parts.join("; "),
                "warning".to_string(),
            ));
        }
        Err(e) => results.push((
            "ERROR".to_string(),
            "triggers".to_string(),
            format!("could not check triggers: {e}"),
            "error".to_string(),
        )),
    }

    // Check 5: TVIEW count
    let tview_count = Spi::get_one::<i64>(&format!(
        "SELECT COUNT(*) FROM {}",
        crate::utils::meta_table()
    ))
    .unwrap_or(Some(0))
    .unwrap_or(0);

    results.push((
        "OK".to_string(),
        "tviews".to_string(),
        format!("{tview_count} TVIEWs registered"),
        "info".to_string(),
    ));

    TableIterator::new(results)
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
    // SAFETY: the text datum borrows `tview`, which outlives the select.
    let readable = Spi::get_one_with_args::<bool>(
        "SELECT pg_catalog.has_table_privilege($1::pg_catalog.regclass, 'SELECT')",
        &[unsafe { pgrx::datum::DatumWithOid::new(tview, pgrx::PgBuiltInOids::TEXTOID.value()) }],
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
