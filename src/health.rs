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

    // Check 3: Metadata consistency
    let orphaned_meta = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_tview_meta m
         WHERE NOT EXISTS (
           SELECT 1 FROM pg_class WHERE relname::text = 'tv_' || m.entity
         )",
    )
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

    // Check 4: Orphaned triggers
    let orphaned_triggers = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_trigger
         WHERE tgname LIKE 'tview_%'
           AND tgrelid NOT IN (
             SELECT ('tb_' || entity)::regclass::oid
             FROM pg_tview_meta
           )",
    )
    .unwrap_or(Some(0))
    .unwrap_or(0);

    if orphaned_triggers > 0 {
        results.push((
            "WARNING".to_string(),
            "triggers".to_string(),
            format!("{orphaned_triggers} orphaned triggers found"),
            "warning".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "triggers".to_string(),
            "All triggers properly linked".to_string(),
            "info".to_string(),
        ));
    }

    // Check 5: TVIEW count
    let tview_count = Spi::get_one::<i64>("SELECT COUNT(*) FROM pg_tview_meta")
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
        "view_recomputes": stats.view_recomputes
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
                "pk": key.pk
            })
        })
        .collect();

    pgrx::JsonB(serde_json::json!(json_contents))
}

/// Get performance statistics for all TVIEWs
///
/// Returns size, row count, and index information for each TVIEW
#[pg_extern]
fn pg_tviews_performance_stats() -> TableIterator<
    'static,
    (
        name!(entity, String),
        name!(table_size, String),
        name!(total_size, String),
        name!(row_count, i64),
        name!(index_count, i32),
    ),
> {
    let query = "
        SELECT
            pg_tview_meta.entity,
            pg_size_pretty(pg_relation_size('tv_' || pg_tview_meta.entity)) as table_size,
            pg_size_pretty(pg_total_relation_size('tv_' || pg_tview_meta.entity)) as total_size,
            (SELECT COUNT(*) FROM ('tv_' || pg_tview_meta.entity)::regclass) as row_count,
            (SELECT COUNT(*)::int FROM pg_indexes WHERE tablename = 'tv_' || pg_tview_meta.entity) as index_count
        FROM pg_tview_meta
        ORDER BY pg_relation_size('tv_' || pg_tview_meta.entity) DESC
    ";

    let results = Spi::connect(|client| match client.select(query, None, &[]) {
        Ok(rows) => {
            let mut stats = Vec::new();
            for row in rows {
                let entity = row["entity"].value::<String>()?.unwrap_or_default();
                let table_size = row["table_size"].value::<String>()?.unwrap_or_default();
                let total_size = row["total_size"].value::<String>()?.unwrap_or_default();
                let row_count = row["row_count"].value::<i64>()?.unwrap_or(0);
                let index_count = row["index_count"].value::<i32>()?.unwrap_or(0);
                stats.push((entity, table_size, total_size, row_count, index_count));
            }
            Ok::<_, spi::Error>(stats)
        }
        Err(e) => {
            warning!("Failed to query performance stats: {}", e);
            Ok(Vec::new())
        }
    })
    .unwrap_or_default();

    TableIterator::new(results)
}
