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
            contents_check(),
            function_paths_check(),
            owner_reads_check(),
            tv_policies_check(),
            frame_settings_check(),
            tv_writers_check(),
            tv_triggers_check(),
            change_feed_check(),
            time_handoff_check(),
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
                format!(
                    "the installed extension is a 0.1.0 catalog: {}",
                    crate::revision::RECREATE
                ),
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
/// Each is planned as its TVIEW's owner, as the trigger runs it, for the TVIEWs
/// whose owner's privileges the caller has: planning runs the owner's code.
fn seq_scan_check() -> Check {
    let mut found = Vec::new();
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("seq_scans", format!("could not be checked: {e}")),
    };
    let mut skipped = 0;
    for meta in &metas {
        match acts_for_owner(meta) {
            Ok(true) => {}
            Ok(false) => {
                skipped += 1;
                continue;
            }
            Err(e) => return error("seq_scans", format!("could not be checked: {e}")),
        }
        let Ok(owner) = crate::owner::Owner::of_tview(meta) else {
            continue;
        };
        let scanned = crate::owner::AsOwner::run(owner, |frame| {
            let mut scans = Vec::new();
            for mapping in &meta.plan.tables {
                let Some(template) = mapping
                    .sql
                    .as_deref()
                    .filter(|_| mapping.kind == crate::lineage::MappingKind::Mapped)
                else {
                    continue;
                };
                let relid = pgrx::pg_sys::Oid::from(mapping.relid);
                // Named as it is called now: the plan's name is the one it had
                // at registration.
                let table = crate::utils::qualified_relname_from_oid(relid)
                    .unwrap_or_else(|_| mapping.table.clone());
                match crate::lineage::large_seq_scans(frame, relid, &table, template) {
                    Ok(found) => scans.extend(found.into_iter().map(|(relation, rows)| {
                        format!(
                            "tv_{} (writes to {table} scan {relation}, about {rows} rows)",
                            meta.entity_name
                        )
                    })),
                    Err(e) => {
                        return Ok(Err(format!(
                            "the mapping of {table} in tv_{}: {e}",
                            meta.entity_name
                        )));
                    }
                }
            }
            Ok(Ok(scans))
        });
        match scanned {
            Ok(Ok(scans)) => found.extend(scans),
            Ok(Err(why)) => return error("seq_scans", why),
            Err(e) => return error("seq_scans", format!("could not be checked: {e}")),
        }
    }
    let unchecked = if skipped == 0 {
        String::new()
    } else {
        format!(
            " ({skipped} TVIEW{} of other owners not checked)",
            if skipped == 1 { "" } else { "s" }
        )
    };
    if found.is_empty() {
        ok(
            "seq_scans",
            format!("No mapping query scans a large table{unchecked}"),
        )
    } else {
        warning(
            "seq_scans",
            format!(
                "writes map to keys with a sequential scan: {}; an index on the columns each \
                 joins on makes them cheaper (SELECT * FROM \
                 tviews.pg_tviews_ensure_propagation_indexes(tview) creates the propagation ones)",
                sample(&found)
            ) + &unchecked,
        )
    }
}

/// TVIEWs whose table is empty while their definition yields rows (a restore
/// that left the data out, a table emptied by hand). Each is read as its owner,
/// who alone may read its backing view; a populated table is not read further.
///
/// `OK` means every registered TVIEW was checked and holds rows: a TVIEW the
/// caller may not be told about, or an UNLOGGED one whose refill is pending, is
/// listed as not checked, with the reason, under `WARNING`. A caller is told
/// about a TVIEW when it has the owner's privileges or may read the whole TVIEW
/// table ([`may_be_told`]). Nothing is read on a standby in recovery.
fn contents_check() -> Check {
    // SAFETY: reads shared memory state.
    if unsafe { pgrx::pg_sys::RecoveryInProgress() } {
        return ok("contents", "Not checked during recovery");
    }
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("contents", format!("could not be checked: {e}")),
    };
    let mut empty = Vec::new();
    let mut unchecked = Vec::new();
    for meta in &metas {
        match contents_of(meta) {
            Ok(Contents::Filled) => {}
            Ok(Contents::Empty(name)) => empty.push(name),
            Ok(Contents::Unchecked(name, why)) => unchecked.push(format!("{name} ({why})")),
            Err(e) => {
                return error(
                    "contents",
                    format!("tv_{} could not be checked: {e}", meta.entity_name),
                );
            }
        }
    }
    let mut parts = Vec::new();
    if !empty.is_empty() {
        parts.push(format!(
            "empty while their definition yields rows: {}; refill each with \
             SELECT tviews.pg_tviews_refresh(tview)",
            sample(&empty)
        ));
    }
    if !unchecked.is_empty() {
        parts.push(format!("not checked: {}", sample(&unchecked)));
    }
    if parts.is_empty() {
        ok(
            "contents",
            "Every TVIEW holds the rows its definition yields",
        )
    } else {
        warning("contents", parts.join("; "))
    }
}

/// What [`contents_check`] found of one TVIEW, by its qualified name.
enum Contents {
    Filled,
    Empty(String),
    Unchecked(String, Unchecked),
}

/// Why [`contents_check`] did not check a TVIEW.
#[derive(Clone, Copy)]
enum Unchecked {
    /// The caller has neither the owner's privileges nor a read of the whole table.
    OwnerPrivileges,
    /// An UNLOGGED TVIEW reset by PostgreSQL: its next write refills it.
    RefillPending,
}

impl std::fmt::Display for Unchecked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OwnerPrivileges => "owner's privileges required",
            Self::RefillPending => "unlogged, refill pending",
        })
    }
}

/// Whether `meta`'s TVIEW is filled, empty while its view yields rows, or not
/// checked. The view is read as the owner.
fn contents_of(meta: &crate::catalog::TviewMeta) -> crate::TViewResult<Contents> {
    let table = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    if !may_be_told(meta)? {
        return Ok(Contents::Unchecked(table, Unchecked::OwnerPrivileges));
    }
    if crate::lifecycle::validity::needs_fill(&meta.entity_name, meta.tview_oid)? {
        return Ok(Contents::Unchecked(table, Unchecked::RefillPending));
    }
    crate::owner::AsOwner::run(crate::owner::Owner::of_tview(meta)?, |frame| {
        frame.require_owner_of(meta.tview_oid)?;
        let view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
        let has_rows = |relation: &str| {
            crate::utils::spi::one_in::<bool>(
                frame,
                &format!("SELECT EXISTS (SELECT 1 FROM {relation})"),
                &[],
            )
            .map(|exists| exists.unwrap_or(false))
        };
        Ok(if !has_rows(&table)? && has_rows(&view)? {
            Contents::Empty(table.clone())
        } else {
            Contents::Filled
        })
    })
}

/// Whether the caller may learn whether `meta`'s TVIEW is empty while its view
/// yields rows: it has the owner's privileges, or may read the whole TVIEW table
/// (`SELECT`, its schema's `USAGE`, no row security applying to it), and so would
/// see the rows a correct table holds anyway.
fn may_be_told(meta: &crate::catalog::TviewMeta) -> crate::TViewResult<bool> {
    Ok(acts_for_owner(meta)?
        || (crate::visibility::may_read_relation(meta.tview_oid)
            && !crate::visibility::rls_active_for_caller(meta.tview_oid)?))
}

/// TVIEWs whose refresh runs a function whose `search_path` does not end in
/// `pg_temp` (registered before such functions were refused): the function
/// resolves unqualified names in the writer's temporary schema first. Reads the
/// catalog only.
fn function_paths_check() -> Check {
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("function_paths", format!("could not be checked: {e}")),
    };
    let mut found = Vec::new();
    for meta in &metas {
        match crate::catalog::function_paths::issues(meta.view_oid, Some(meta.tview_oid)) {
            Ok(issues) => found.extend(issues.into_iter().map(|issue| {
                format!(
                    "tv_{} runs {} (search_path = {}; set it to {})",
                    meta.entity_name, issue.function, issue.path, issue.fix
                )
            })),
            Err(e) => {
                return error(
                    "function_paths",
                    format!("tv_{} could not be checked: {e}", meta.entity_name),
                );
            }
        }
    }
    if found.is_empty() {
        ok(
            "function_paths",
            "Every function a refresh runs sets no search_path, or one ending in pg_temp",
        )
    } else {
        error(
            "function_paths",
            format!(
                "functions whose search_path does not end in pg_temp: {}; fix each with \
                 ALTER FUNCTION … SET search_path = <schemas>, pg_temp",
                sample(&found)
            ),
        )
    }
}

/// TVIEWs read for by a role that may no longer read what it reads (a privilege
/// revoked after the create): their next refresh fails, and the update refuses
/// them. Reads the catalog only.
fn owner_reads_check() -> Check {
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("owner_reads", format!("could not be checked: {e}")),
    };
    let mut found = Vec::new();
    for meta in &metas {
        match crate::catalog::owner_reads::lacking(meta) {
            Ok(lacking) if lacking.is_empty() => {}
            Ok(lacking) => found.push(format!("tv_{} ({})", meta.entity_name, lacking.join("; "))),
            Err(e) => {
                return error(
                    "owner_reads",
                    format!("tv_{} could not be checked: {e}", meta.entity_name),
                );
            }
        }
    }
    if found.is_empty() {
        ok(
            "owner_reads",
            "The roles reading for every TVIEW may read what they read",
        )
    } else {
        error(
            "owner_reads",
            format!(
                "roles that may no longer read what they read for a TVIEW: {}; grant each \
                 privilege named, or drop the TVIEW",
                sample(&found)
            ),
        )
    }
}

/// Row security on the TVIEWs' own tables (D6, D8):
/// - an ERROR for a table whose owner a policy can filter (`FORCE ROW LEVEL
///   SECURITY` without a policy letting it read and write every row): its
///   refreshes fail ([`crate::guard::filtered_tviews`]);
/// - a WARNING for a table other roles may read, without row security, while a
///   table it reads filters its readers: the TVIEW publishes what they hide;
/// - a WARNING for a table embedding rows of a table with row security whose
///   policies differ from its own: a reader may read, inside a document, rows
///   that table hides from it. Policies are compared as written (command,
///   permissive, `USING`, `WITH CHECK`), roles aside.
fn tv_policies_check() -> Check {
    let filtered = match crate::guard::filtered_tviews() {
        Ok(filtered) => filtered,
        Err(e) => return error("tv_policies", format!("could not be checked: {e}")),
    };
    if !filtered.is_empty() {
        let found: Vec<String> = filtered
            .iter()
            .map(|(table, owner, why)| format!("{table} ({owner}: {why})"))
            .collect();
        return error(
            "tv_policies",
            format!(
                "row security applies to the owner of {}: its refreshes fail; give the owner a \
                 policy USING (true) WITH CHECK (true), or NO FORCE ROW LEVEL SECURITY",
                sample(&found)
            ),
        );
    }
    let warnings = crate::catalog::queries::tv_policy_warnings();
    match warnings {
        Ok(found) if found.is_empty() => ok(
            "tv_policies",
            "No TVIEW publishes rows a policy of a table it reads hides",
        ),
        Ok(found) => warning("tv_policies", sample(&found)),
        Err(e) => error("tv_policies", format!("could not be checked: {e}")),
    }
}

/// The custom settings each TVIEW's definition reads by name, with the value its
/// refreshes read (the server's, the database's, `ALTER ROLE ALL`'s, or its
/// `settings` option's), never a session's. A WARNING when one is unset there:
/// a per-request setting, which no refresh sees.
fn frame_settings_check() -> Check {
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("frame_settings", format!("could not be checked: {e}")),
    };
    let reads: Vec<(&str, &str, Option<&str>)> = metas
        .iter()
        .flat_map(|meta| {
            meta.plan.settings_read.iter().map(|name| {
                let pinned = meta
                    .frame_settings
                    .iter()
                    .find(|(pinned, _)| pinned == name)
                    .map(|(_, value)| value.as_str());
                (meta.entity_name.as_str(), name.as_str(), pinned)
            })
        })
        .collect();
    if reads.is_empty() {
        return ok("frame_settings", "No TVIEW reads a custom setting by name");
    }
    let _frame = crate::owner::RenderPin::new();
    let mut unset = Vec::new();
    let mut read = Vec::new();
    for &(entity, name, pinned) in &reads {
        // The TVIEW's settings option pins it, else the frame reads the server's.
        let value = match pinned {
            Some(pinned) => (!pinned.is_empty()).then(|| pinned.to_string()),
            None => crate::owner::setting_value(name),
        };
        match value {
            Some(value) => read.push(format!("tv_{entity} reads {name} as '{value}'")),
            None => unset.push(format!("tv_{entity} reads {name}")),
        }
    }
    if unset.is_empty() {
        ok(
            "frame_settings",
            format!("{}; a session's value reaches no refresh", sample(&read)),
        )
    } else {
        warning(
            "frame_settings",
            format!(
                "{}, unset where its refreshes run: they read no session's value (a \
                 per-request setting); set it for the server or the database, or pin it \
                 with the settings option",
                sample(&unset)
            ),
        )
    }
}

/// Whether the caller has the privileges of the owner of `meta`'s TVIEW: only
/// then is the TVIEW's own code run as its owner on the caller's behalf.
fn acts_for_owner(meta: &crate::catalog::TviewMeta) -> crate::TViewResult<bool> {
    let member = crate::utils::spi::one::<bool>(
        "SELECT pg_catalog.pg_has_role(c.relowner, 'USAGE') FROM pg_catalog.pg_class c \
         WHERE c.oid = $1",
        &[crate::utils::spi::oid(meta.tview_oid)],
    )?;
    Ok(member == Some(true))
}

/// TVIEWs whose plan an earlier release derived (`needs_reregister`): an update
/// that could not derive them again leaves them so. Each is named, with why it
/// cannot be re-derived when a check of this release knows (row security, a
/// privilege revoked); their refreshes skip and patch nothing until then.
fn reregister_check() -> Check {
    let metas = match crate::catalog::TviewMeta::load_all() {
        Ok(metas) => metas,
        Err(e) => return error("reregister", format!("could not be checked: {e}")),
    };
    let found: Vec<String> = metas
        .iter()
        .filter(|meta| meta.plan_release == crate::catalog::PlanRelease::Earlier)
        .map(|meta| match reregister_reason(meta) {
            Some(why) => format!("tv_{}: {why}", meta.entity_name),
            None => format!("tv_{}", meta.entity_name),
        })
        .collect();
    if found.is_empty() {
        ok("reregister", "No TVIEW needs re-registration")
    } else {
        warning(
            "reregister",
            format!(
                "{} left to re-register: {}; fix each and run \
                 SELECT tviews.pg_tviews_reregister('<TVIEW>'), or \
                 SELECT * FROM tviews.pg_tviews_reregister_all()",
                count(found.len(), "TVIEW"),
                found.join("; ")
            ),
        )
    }
}

/// Why `meta`'s TVIEW cannot be re-derived, when a check of this release says:
/// row security on a table it reads, or a privilege a role reading for it lacks.
fn reregister_reason(meta: &crate::catalog::TviewMeta) -> Option<String> {
    if let Err(e) = crate::catalog::stopgap::check_row_security(meta) {
        return Some(first_line(&e.to_string()));
    }
    match crate::catalog::owner_reads::lacking(meta) {
        Ok(lacking) if !lacking.is_empty() => Some(lacking.join("; ")),
        _ => None,
    }
}

/// The first line of `message`.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or_default().to_string()
}

/// `pg_tviews`' triggers against the plans: a missing or disabled one leaves a
/// TVIEW stale, one an older release installed per TVIEW fails the writes to its
/// table (errors); an orphaned one does no harm (a warning).
fn trigger_check() -> Check {
    match crate::dependency::triggers::trigger_problems() {
        Ok(p)
            if p.orphaned.is_empty()
                && p.missing.is_empty()
                && p.per_tview.is_empty()
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
                    &p.per_tview,
                    "trigger of an older release",
                    "(run pg_tviews_reregister_all())",
                ),
            ]
            .into_iter()
            .filter(|(list, _, _)| !list.is_empty())
            .map(|(list, what, action)| {
                format!("{} {action}: {}", count(list.len(), what), sample(list))
            })
            .collect();
            if p.disabled.is_empty() && p.missing.is_empty() && p.per_tview.is_empty() {
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
/// (`queued`) of the TVIEWs the caller may read without row security applying;
/// the others are named by type in `omitted_types`. `queue_size` counts every
/// key, as `tviews.stats` does.
///
/// # Errors
/// A failed catalog read.
pub(crate) fn queue_stats() -> crate::TViewResult<serde_json::Value> {
    let (queued, omitted) = queued()?;
    let stats = crate::metrics::metrics_api::get_queue_stats(crate::queue::get_queue_size());

    let mut json_value = serde_json::json!({
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
        "queued": queued,
    });
    if !omitted.is_empty() {
        json_value["omitted_types"] = serde_json::json!(omitted);
    }
    #[cfg(feature = "test-hooks")]
    {
        json_value["cache_resets"] = crate::cache::resets().into();
        json_value["pending_work"] = crate::queue::state::pending_count().into();
        json_value["patch_stats"] = crate::stats::patch_stats()
            .into_iter()
            .map(|(code, n)| (code, serde_json::Value::from(n)))
            .collect::<serde_json::Map<_, _>>()
            .into();
        json_value["stats_slots_by_database"] = crate::stats::slots_by_database()
            .into_iter()
            .map(|(db, n)| (db.to_string(), serde_json::Value::from(n)))
            .collect::<serde_json::Map<_, _>>()
            .into();
    }

    Ok(json_value)
}

/// The keys queued in the transaction, as `{entity, pk}`, of the TVIEWs the
/// caller may read without row security applying, and the types of the others.
///
/// # Errors
/// A failed catalog read.
fn queued() -> crate::TViewResult<(Vec<serde_json::Value>, std::collections::BTreeSet<String>)> {
    let keys = crate::queue::get_queue_contents();
    let entities: std::collections::BTreeSet<String> =
        keys.iter().map(|key| key.entity.clone()).collect();
    let mut shown = std::collections::HashSet::new();
    let mut omitted = std::collections::BTreeSet::new();
    for (entity, typename, _, oid) in
        crate::catalog::queries::typenames_and_tables(entities.into_iter().collect())?
    {
        // Unknown counts as active.
        if crate::visibility::may_read_relation(oid)
            && !crate::visibility::rls_active_for_caller(oid).unwrap_or(true)
        {
            shown.insert(entity);
        } else {
            omitted.insert(typename);
        }
    }
    let listed = keys
        .into_iter()
        .filter(|key| shown.contains(&key.entity))
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
    Ok((listed, omitted))
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
/// The TVIEW tables' OIDs, for the checks reading their grants.
fn tview_tables() -> crate::TViewResult<Vec<pgrx::pg_sys::Oid>> {
    Ok(crate::catalog::TviewMeta::load_all()?
        .iter()
        .map(|meta| meta.tview_oid)
        .collect())
}

/// Roles other than a TVIEW table's owner granted INSERT, UPDATE, DELETE or
/// TRUNCATE on it (E1-15). `pg_tviews` is the table's only writer, and its guard
/// refuses every other write: such a grant serves nothing, and writes as soon as
/// the guard is disabled.
fn tv_writers_check() -> Check {
    let found = tview_tables().and_then(|tables| {
        crate::utils::spi::strings(
            "SELECT pg_catalog.format('%s: %s %s', t.oid::pg_catalog.regclass, \
                    CASE a.grantee WHEN 0 THEN 'PUBLIC' \
                         ELSE pg_catalog.pg_get_userbyid(a.grantee)::pg_catalog.text END, \
                    pg_catalog.string_agg(a.privilege_type, '/' ORDER BY a.privilege_type)) \
             FROM pg_catalog.pg_class t, pg_catalog.aclexplode(t.relacl) a \
             WHERE t.oid OPERATOR(pg_catalog.=) ANY ($1) \
               AND a.grantee OPERATOR(pg_catalog.<>) t.relowner \
               AND a.privilege_type IN ('INSERT', 'UPDATE', 'DELETE', 'TRUNCATE') \
             GROUP BY t.oid, a.grantee ORDER BY 1",
            &[crate::utils::spi::oid_array(tables)],
        )
    });
    match found {
        Ok(found) if found.is_empty() => ok(
            "tv_writers",
            "No role but its owner may write a TVIEW table",
        ),
        Ok(found) => warning(
            "tv_writers",
            format!(
                "writes granted on TVIEW tables, which only pg_tviews writes (its guard refuses \
                 the others): {}; REVOKE INSERT, UPDATE, DELETE, TRUNCATE from these roles",
                sample(&found)
            ),
        ),
        Err(e) => error("tv_writers", format!("could not be checked: {e}")),
    }
}

/// User triggers and rules on TVIEW tables: each changes what a maintenance write
/// leaves in the table, which freshness assumes nothing does. A WARNING naming
/// them; `pg_tviews`' own triggers (functions of its schema) are not reported.
fn tv_triggers_check() -> Check {
    let found = tview_tables().and_then(|tables| {
        crate::utils::spi::strings(
            "SELECT x.what FROM ( \
               SELECT pg_catalog.format('%s on %s', t.tgname, \
                          t.tgrelid::pg_catalog.oid::pg_catalog.regclass::pg_catalog.text) AS what \
               FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_proc p ON p.oid = t.tgfoid \
               WHERE NOT t.tgisinternal AND t.tgrelid OPERATOR(pg_catalog.=) ANY ($1) \
                 AND p.pronamespace NOT IN (SELECT n.oid FROM pg_catalog.pg_namespace n \
                                            WHERE n.nspname OPERATOR(pg_catalog.=) $2) \
               UNION ALL \
               SELECT pg_catalog.format('rule %s on %s', r.rulename, \
                          r.ev_class::pg_catalog.oid::pg_catalog.regclass::pg_catalog.text) \
               FROM pg_catalog.pg_rewrite r \
               WHERE r.ev_class OPERATOR(pg_catalog.=) ANY ($1) \
                 AND r.rulename OPERATOR(pg_catalog.<>) '_RETURN') x \
             ORDER BY x.what",
            &[
                crate::utils::spi::oid_array(tables),
                crate::utils::spi::text(crate::utils::ext_schema()),
            ],
        )
    });
    match found {
        Ok(found) if found.is_empty() => {
            ok("tv_triggers", "No user trigger or rule on a TVIEW table")
        }
        Ok(found) => warning(
            "tv_triggers",
            format!(
                "user triggers or rules on TVIEW tables change what pg_tviews writes there, \
                 and its rows may then differ from their definitions: {}",
                sample(&found)
            ),
        ),
        Err(e) => error("tv_triggers", format!("could not be checked: {e}")),
    }
}

/// The change feed (`pg_tviews.publish_changes = rows`) sends the keys of every
/// TVIEW's changed rows to every role that can `LISTEN`, whatever it may read
/// (E1-05): a WARNING naming the login roles that may connect but not read a TVIEW.
fn change_feed_check() -> Check {
    match crate::config::publish_changes() {
        None => return ok("change_feed", "The change feed is off"),
        Some(crate::config::Publish::Types) => {
            return ok(
                "change_feed",
                "The change feed publishes type names only: no keys of rows",
            );
        }
        Some(crate::config::Publish::Rows) => {}
    }
    let found = tview_tables().and_then(|tables| {
        crate::utils::spi::strings(
            "SELECT pg_catalog.format('%s (%s)', r.rolname, \
                    pg_catalog.string_agg(t.oid::pg_catalog.regclass::pg_catalog.text, ' ' \
                                          ORDER BY t.oid::pg_catalog.regclass::pg_catalog.text)) \
             FROM pg_catalog.pg_roles r, pg_catalog.pg_class t \
             WHERE r.rolcanlogin AND NOT r.rolsuper \
               AND pg_catalog.has_database_privilege(r.oid, \
                       pg_catalog.current_database(), 'CONNECT') \
               AND t.oid OPERATOR(pg_catalog.=) ANY ($1) \
               AND NOT pg_catalog.has_table_privilege(r.oid, t.oid, 'SELECT') \
             GROUP BY r.rolname ORDER BY r.rolname",
            &[crate::utils::spi::oid_array(tables)],
        )
    });
    match found {
        Ok(found) if found.is_empty() => ok(
            "change_feed",
            "The change feed publishes row keys; every role that may connect may read every TVIEW",
        ),
        Ok(found) => warning(
            "change_feed",
            format!(
                "the change feed publishes the keys of changed rows to any role that can LISTEN: \
                 these may connect but not read the TVIEWs named: {}; set \
                 pg_tviews.publish_changes to types or off, or REVOKE CONNECT",
                sample(&found)
            ),
        ),
        Err(e) => error("change_feed", format!("could not be checked: {e}")),
    }
}

/// Keys written across a time boundary by a transaction older than it, waiting
/// for the drain (`crate::time_drain`): a WARNING once the oldest waited a minute,
/// which the drain worker takes milliseconds to clear. Counts only, read as the
/// extension's owner: the keys are rows of TVIEWs the caller may not read.
fn time_handoff_check() -> Check {
    let found = crate::owner::as_extension_owner(|| {
        crate::utils::spi::two::<i64, f64>(
            &format!(
                "SELECT pg_catalog.count(*), \
                        COALESCE(pg_catalog.date_part('epoch', \
                            pg_catalog.clock_timestamp() OPERATOR(pg_catalog.-) \
                            pg_catalog.min(h.handed_at)), 0)::pg_catalog.float8 \
                 FROM {}.pg_tview_time_handoff h",
                crate::utils::ext_schema()
            ),
            &[],
        )
    });
    match found {
        Ok((Some(0) | None, _)) => ok(
            "time_handoff",
            "No key written across a time boundary waits to be refreshed",
        ),
        Ok((Some(pending), age)) if age.unwrap_or(0.0) < 60.0 => ok(
            "time_handoff",
            format!("keys written across a time boundary, being refreshed: {pending}"),
        ),
        Ok((Some(pending), age)) => warning(
            "time_handoff",
            format!(
                "keys written across a time boundary wait to be refreshed: {pending}, the oldest \
                 for {:.0} s: the drain worker did not run (see the server log); the next \
                 time refresh of each TVIEW refreshes them",
                age.unwrap_or(0.0)
            ),
        ),
        Err(e) => error("time_handoff", format!("could not be checked: {e}")),
    }
}

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
