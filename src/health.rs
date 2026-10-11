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
pub(crate) fn health_check() -> Vec<(String, String, String, String)> {
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
            plan_check(),
            reregister_check(),
            trigger_check(),
            count_check(),
        ]);
    }
    results
}

/// The count `sql` returns, or the error row of `component` saying it could
/// not be read.
fn count_of(component: &str, sql: &str) -> Result<i64, Check> {
    Spi::get_one::<i64>(sql)
        .map(Option::unwrap_or_default)
        .map_err(|e| error(component, format!("could not be checked: {e}")))
}

/// The catalog's counts ([`crate::catalog::row::counts`]), or the failed check.
fn registered(component: &str) -> Result<(i64, i64, i64), Check> {
    crate::catalog::row::counts()
        .map_err(|e| error(component, format!("could not be checked: {e}")))
}

fn jsonb_delta_check() -> Check {
    let installed = match count_of(
        "jsonb_delta",
        "SELECT COUNT(*) FROM pg_extension WHERE extname = 'jsonb_delta'",
    ) {
        Ok(n) => n > 0,
        Err(check) => return check,
    };
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
        crate::revision::Installed::Unreadable(why) => (
            error(
                "catalog",
                format!("the catalog revision could not be read: {why}"),
            ),
            false,
        ),
    }
}

fn metadata_check() -> Check {
    let orphaned = match registered("metadata") {
        Ok((_, _, orphaned)) => orphaned,
        Err(check) => return check,
    };
    if orphaned > 0 {
        error(
            "metadata",
            format!("{orphaned} orphaned metadata entries found"),
        )
    } else {
        ok("metadata", "All metadata entries valid")
    }
}

/// TVIEWs whose stored plan or identity does not decode: every write to the
/// tables they read fails until they are re-registered.
fn plan_check() -> Check {
    match crate::catalog::TviewMeta::unreadable() {
        Ok(unreadable) if unreadable.is_empty() => ok("plans", "Every propagation plan reads"),
        Ok(unreadable) => error(
            "plans",
            format!(
                "{} unreadable; until re-registered, writes to the base tables of every \
                 TVIEW fail: {}; run SELECT tviews.pg_tviews_reregister(entity) for each",
                count(unreadable.len(), "TVIEW"),
                unreadable
                    .iter()
                    .map(|(entity, why)| format!("tv_{entity} ({why})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Err(e) => error("plans", format!("could not be checked: {e}")),
    }
}

/// TVIEWs registered before a release that changed what registration derives.
fn reregister_check() -> Check {
    let stale = match registered("reregister") {
        Ok((_, stale, _)) => stale,
        Err(check) => return check,
    };
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
/// `pg_tviews`' triggers against the plans: a missing or disabled one leaves a
/// TVIEW stale (an error); an orphaned or untagged one does no harm (a warning).
fn trigger_check() -> Check {
    match crate::dependency::triggers::trigger_problems() {
        Ok(p)
            if p.orphaned.is_empty()
                && p.missing.is_empty()
                && p.untagged.is_empty()
                && p.disabled.is_empty() =>
        {
            ok("triggers", "All triggers properly linked")
        }
        Ok(p) => {
            let parts: Vec<String> = [
                (
                    &p.disabled,
                    "disabled trigger",
                    "(ALTER TABLE … ENABLE TRIGGER)",
                ),
                (
                    &p.missing,
                    "missing trigger",
                    "(run pg_tviews_reregister_all())",
                ),
                (&p.orphaned, "orphaned trigger", "found"),
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
            if p.disabled.is_empty() && p.missing.is_empty() {
                warning("triggers", parts.join("; "))
            } else {
                error("triggers", parts.join("; "))
            }
        }
        Err(e) => error("triggers", format!("could not check triggers: {e}")),
    }
}

fn count_check() -> Check {
    match registered("tviews") {
        Ok((tviews, _, _)) => ok("tviews", format!("{tviews} TVIEWs registered")),
        Err(check) => check,
    }
}

/// Get current queue statistics
/// Returns metrics about the current transaction's refresh operations
pub(crate) fn queue_stats() -> serde_json::Value {
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
        "value_locks": stats.value_locks,
        "value_lock_escalations": stats.value_lock_escalations,
        "value_lock_waits": stats.value_lock_waits,
        "value_lock_wait_ms": stats.value_lock_wait_ms(),
        "direct_patch_captured": stats.direct_patch_captured,
        "direct_patches_applied": stats.direct_patches_applied,
        "direct_patch_fallbacks": stats.direct_patch_fallbacks,
        "view_recomputes": stats.view_recomputes,
        "refresh_noop_skipped": stats.refresh_noop_skipped,
        "catalog_lookups": stats.catalog_lookups,
        "propagation_pruned": stats.propagation_pruned
    });

    json_value
}

/// Debug function: View current queue contents
/// Returns the entities and PKs currently in the refresh queue
pub(crate) fn debug_queue() -> serde_json::Value {
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

    serde_json::json!(json_contents)
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
