//! Health checks, diagnostics, and performance monitoring.

/// One row of `pg_tviews_check_health()`: status, component, message, severity.
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
/// - Catalog revision and metadata consistency
/// - Orphaned and missing triggers
/// - TVIEW count
pub(crate) fn health_check() -> Vec<(String, String, String, String)> {
    let mut results = vec![ok(
        "extension",
        format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
    )];
    let (catalog, unversioned) = catalog_check();
    results.push(catalog);
    // A 0.1.0 catalog has none of the tables the other checks read.
    if !unversioned {
        results.extend([
            metadata_check(),
            plan_check(),
            columns_check(),
            seq_scan_check(),
            reregister_check(),
            trigger_check(),
            count_check(),
        ]);
    }
    results
}

/// The catalog's counts ([`crate::catalog::row::counts`]), or the failed check.
fn registered(component: &str) -> Result<(i64, i64, i64), Check> {
    crate::catalog::row::counts()
        .map_err(|e| error(component, format!("could not be checked: {e}")))
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

/// Every column a stored plan numbers still has that number: a restore or a
/// catalog edited by hand that left a stale number makes writes read another
/// column, or fail.
fn columns_check() -> Check {
    match crate::catalog::row::stale_column_numbers() {
        Ok(None) => ok("columns", "Every column a plan numbers has its number"),
        Ok(Some(stale)) => error(
            "columns",
            format!(
                "stale column numbers, writes to these tables read other columns: {stale}; \
                 run SELECT tviews.pg_tviews_reregister(entity) for each"
            ),
        ),
        Err(e) => error("columns", format!("could not be checked: {e}")),
    }
}

/// Mapping queries that scan a large table: the advice registration printed,
/// re-evaluated with today's indexes and statistics (an index dropped later).
/// Each is planned as its TVIEW's owner, as the trigger runs it.
fn seq_scan_check() -> Check {
    let mut found = Vec::new();
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("seq_scans", format!("could not be checked: {e}")),
    };
    for meta in &metas {
        let Ok(_owner) = crate::owner::AsOwner::of_entity(&meta.entity_name) else {
            continue;
        };
        for mapping in &meta.plan.tables {
            let Some(template) = mapping
                .sql
                .as_deref()
                .filter(|_| mapping.kind == crate::lineage::MappingKind::Mapped)
            else {
                continue;
            };
            match crate::lineage::large_seq_scans(&mapping.table, template) {
                Ok(scans) => found.extend(scans.into_iter().map(|(relation, rows)| {
                    format!(
                        "tv_{} (writes to {} scan {relation}, about {rows} rows)",
                        meta.entity_name, mapping.table
                    )
                })),
                Err(e) => {
                    return error(
                        "seq_scans",
                        format!(
                            "the mapping of {} in tv_{}: {e}",
                            mapping.table, meta.entity_name
                        ),
                    );
                }
            }
        }
    }
    if found.is_empty() {
        ok("seq_scans", "No mapping query scans a large table")
    } else {
        warning(
            "seq_scans",
            format!(
                "writes map to keys with a sequential scan: {}; an index on the columns each \
                 joins on makes them cheaper (SELECT * FROM \
                 tviews.pg_tviews_ensure_propagation_indexes(tview) creates the propagation ones)",
                sample(&found)
            ),
        )
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

/// The session's refresh counters, and the keys queued in the transaction
/// (`queued`).
pub(crate) fn queue_stats() -> serde_json::Value {
    let stats = crate::metrics::metrics_api::get_queue_stats(crate::queue::get_queue_size());

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
        "plans_prepared": stats.plans_prepared,
        "per_execution_plans": stats.per_execution_plans,
        "propagation_pruned": stats.propagation_pruned,
        "queued": queued(),
    });

    json_value
}

/// The keys queued in the transaction, as `{entity, pk}`.
fn queued() -> Vec<serde_json::Value> {
    crate::queue::get_queue_contents()
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
        .collect()
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
