//! Maintenance services behind the API: rebuilds with their dependents, time
//! refresh, propagation indexes, cascade paths.

use crate::TViewResult;

/// The TVIEWs whose definitions read the current time that the caller owns (or
/// may change as a member of its owner's role).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub(crate) fn owned_time_dependent() -> TViewResult<Vec<crate::catalog::TviewMeta>> {
    let entities = crate::utils::spi::strings(
        &format!(
            "SELECT m.entity::text FROM {} m JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
             WHERE m.time_dependent AND pg_catalog.pg_has_role(c.relowner, 'USAGE') \
             ORDER BY m.entity",
            crate::catalog::meta_table()
        ),
        &[],
    )?;
    let mut metas = Vec::new();
    for entity in entities {
        if let Some(meta) = crate::catalog::TviewMeta::load_by_entity(&entity)? {
            metas.push(meta);
        }
    }
    Ok(metas)
}

/// Refresh `chosen` TVIEWs, which read the current time, in full, as a write to a
/// `full_refresh` table would refresh them, then the TVIEWs reading them, through
/// the flush. Returns the TVIEWs refreshed, dependencies first.
///
/// # Errors
/// Returns an error if the dependency graph cannot be loaded or a refresh fails.
pub(crate) fn refresh_time_dependent(
    chosen: &[crate::catalog::TviewMeta],
) -> TViewResult<Vec<String>> {
    let order = crate::cache::graph()?.topo_order;
    let mut chosen: Vec<&crate::catalog::TviewMeta> = chosen.iter().collect();
    chosen.sort_by_key(|meta| {
        order
            .iter()
            .position(|e| *e == meta.entity_name)
            .unwrap_or(usize::MAX)
    });
    let mut refreshed = Vec::new();
    for meta in chosen {
        if crate::suspend::is_suspended() {
            crate::suspend::record_change(&meta.entity_name);
        } else {
            crate::queue::enqueue_refresh_all(&meta.entity_name);
        }
        refreshed.push(crate::utils::qualified_relname_from_oid(meta.tview_oid)?);
    }
    crate::flush::flush_refresh_queue()?;
    Ok(refreshed)
}

/// Rebuild `entities` and every TVIEW whose view reads one of them, transitively
/// (the readers of a TVIEW come from the complete dependency relation, not the
/// pruned one flush-time propagation follows), dependencies first, each as its
/// owner. Returns the rebuilt entities in order.
///
/// # Errors
/// Returns an error if the dependency graph cannot be loaded or a rebuild fails.
pub fn rebuild_with_dependents(entities: &[String]) -> TViewResult<Vec<String>> {
    let graph = crate::cache::graph()?;
    let mut readers: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for (reader, read) in &graph.children {
        for entity in read {
            readers.entry(entity).or_default().push(reader);
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut pending: std::collections::VecDeque<&str> =
        entities.iter().map(String::as_str).collect();
    while let Some(entity) = pending.pop_front() {
        if order.iter().any(|e| e == entity) {
            continue;
        }
        pending.extend(readers.get(entity).into_iter().flatten());
        order.push(entity.to_string());
    }
    order.sort_by_key(|e| {
        graph
            .topo_order
            .iter()
            .position(|t| t == e)
            .unwrap_or(usize::MAX)
    });
    for entity in &order {
        crate::refresh::full::rebuild_one(entity)?;
    }
    flush_after_rebuilds()?;
    Ok(order)
}

/// Refresh what rebuilds queued before returning: rewriting a `tv_*` table
/// that another TVIEW reads queues that reader's rows. No statement-level
/// flush trigger follows a `SELECT` of a refresh function, so the work would
/// otherwise reach `COMMIT` still queued.
///
/// # Errors
/// Returns an error if the refresh fails.
pub fn flush_after_rebuilds() -> TViewResult<()> {
    crate::flush::flush_refresh_queue()
}

/// Create the propagation indexes `(<lookup>, <identity>)` that `metas` are
/// missing, and return the DDL of each; with `dry_run`, only return it.
///
/// A TVIEW's rows are looked up by the columns holding an embedded TVIEW's key
/// and those a fan-out patch writes through (the plan's lookup columns); without
/// an index each lookup scans the whole TVIEW. A lookup column counts as covered
/// when **any** index on the TVIEW leads with it, so user-created indexes are
/// respected. Idempotent. On large TVIEWs run the returned statements by hand with
/// `CREATE INDEX CONCURRENTLY`. The indexes it creates, and those its statements
/// created when run by hand, are `pg_tviews`' (`tviews.registry.managed_indexes`).
///
/// # Errors
/// Returns error if the catalog query or an index creation fails.
pub(crate) fn ensure_propagation_indexes(
    metas: &[crate::catalog::TviewMeta],
    dry_run: bool,
) -> TViewResult<Vec<String>> {
    let mut missing = Vec::new();
    for meta in metas {
        let key = &meta.identity.column;
        let mut lookups = meta.plan.lookup_columns();
        lookups.remove(key.as_str());
        if lookups.is_empty() {
            continue;
        }
        let (schema, table) = crate::ddl::relation_name(meta.tview_oid)?;
        let mut recorded = Vec::new();
        for column in lookups {
            let index = crate::ddl::create::ManagedIndex::propagation(&table, column, key);
            let indexed = crate::utils::spi::one::<bool>(
                "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                 JOIN pg_catalog.pg_attribute a \
                   ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] \
                 WHERE i.indrelid = $1 AND a.attname = $2)",
                &[
                    crate::utils::spi::oid(meta.tview_oid),
                    crate::utils::spi::text(column),
                ],
            )?;
            if indexed != Some(true) {
                missing.push(index.ddl(&schema, &table));
                if !dry_run {
                    recorded.extend(index.create(&schema, &table)?);
                }
            } else if !dry_run && index.exists_on(meta.tview_oid)? {
                // Its reported statement, run by hand: pg_tviews' own.
                recorded.push(index.name);
            }
        }
        if !recorded.is_empty() {
            crate::catalog::indexes::record(meta.tview_oid, &recorded)?;
        }
    }
    Ok(missing)
}

/// Rebuild every TVIEW from its backing view in dependency order: a TVIEW whose
/// view reads another `tv_*` table is rebuilt after it. Returns the entities in
/// the order they were rebuilt.
///
/// # Errors
/// Returns error if the dependency graph cannot be loaded or a rebuild fails.
pub fn refresh_all_in_dependency_order() -> TViewResult<Vec<String>> {
    let graph = crate::cache::graph()?;
    for entity in &graph.topo_order {
        crate::refresh::full::rebuild_one(entity)?;
    }
    flush_after_rebuilds()?;
    Ok(graph.topo_order)
}

/// The TVIEWs embedding `entity`'s, transitively: each with its depth and the
/// TVIEW it embeds.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub(crate) fn cascade_path(entity: &str) -> TViewResult<Vec<(i32, String, String)>> {
    const MAX_DEPTH: i32 = 10;
    let graph = crate::cache::graph()?;
    let mut rows = vec![(0, entity.to_string(), entity.to_string())];
    // Every path up from `entity` that visits a TVIEW once.
    let mut frontier = vec![vec![entity.to_string()]];
    for depth in 1..=MAX_DEPTH {
        let mut next = Vec::new();
        for path in frontier {
            let Some(node) = path.last() else { continue };
            let mut embedding: Vec<&String> =
                graph.parents.get(node).into_iter().flatten().collect();
            embedding.sort();
            embedding.dedup();
            for parent in embedding.into_iter().filter(|p| !path.contains(p)) {
                rows.push((depth, parent.clone(), node.clone()));
                let mut longer = path.clone();
                longer.push(parent.clone());
                next.push(longer);
            }
        }
        frontier = next;
    }
    rows.sort();
    Ok(rows)
}
