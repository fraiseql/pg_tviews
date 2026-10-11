//! Administrative SQL functions: refresh, migration, cascade path.

use crate::{TViewError, TViewResult, utils::ident};
use pgrx::prelude::*;

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
            crate::utils::meta_table()
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
    let order = crate::flush::EntityDepGraph::load()?.topo_order;
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
    let graph = crate::flush::EntityDepGraph::load()?;
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
        rebuild_one(entity)?;
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

/// Rebuild one TVIEW from its backing view (`TRUNCATE` + `INSERT … SELECT`), as
/// its owner, and nothing that reads it.
///
/// The backing view runs the functions it calls as the querying role: rebuilding
/// as the caller would run the owner's view code with the caller's privileges
/// (a superuser's, after a migration).
///
/// # Errors
/// Returns error if the entity is not registered or the truncate/insert fails.
pub fn rebuild_one(entity: &str) -> TViewResult<()> {
    if let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? {
        // Every row changes: refreshes of any of them wait, and are waited for,
        // and so are writers of anything they read.
        crate::concurrency::lock_relation(
            meta.tview_oid.to_u32(),
            crate::concurrency::Side::Writer,
        );
        crate::concurrency::reads::lock_whole_read_set(&meta)?;
    }
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (qi_tv, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&format!("TRUNCATE {qi_tv}"))?;
    Spi::run(&insert)?;
    drop(owner);
    // Rebuilt from its view: its rows can be trusted again.
    let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
        TViewError::TviewNotFound {
            name: entity.to_string(),
        }
    })?;
    crate::lifecycle::validity::mark(meta.tview_oid)?;
    Ok(())
}

/// Replace every row of `tv_<entity>` with its backing view's, without `TRUNCATE`
/// (readers are never blocked), as its owner: the fill of a TVIEW whose rows
/// can't be trusted ([`crate::lifecycle::validity`]). Rows a concurrent writer
/// committed meanwhile are kept.
///
/// # Errors
/// Returns error if the entity is not registered or the delete/insert fails.
pub fn refill(entity: &str) -> TViewResult<()> {
    if let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? {
        crate::concurrency::reads::lock_whole_read_set(&meta)?;
    }
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (qi_tv, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&format!("DELETE FROM {qi_tv}"))?;
    Spi::run(&format!("{insert} ON CONFLICT DO NOTHING"))?;
    Ok(())
}

/// Populate an **empty** `tv_<entity>` from its backing view without `TRUNCATE`,
/// as its owner.
///
/// Used to fill a TVIEW created empty (a rebuild by another role). Unlike
/// [`rebuild_one`] it takes only a ROW EXCLUSIVE lock, so readers are never
/// blocked, even when the transaction stays prepared (2PC) for a while.
///
/// # Errors
/// Returns error if the entity is not registered or the insert fails.
pub fn fill_empty_tview(entity: &str) -> TViewResult<()> {
    let owner = crate::owner::AsOwner::of_entity(entity)?;
    let (_, insert) = rebuild_statements(&owner, entity)?;
    Spi::run(&insert)?;
    Ok(())
}

/// The schema-qualified TVIEW table and the `INSERT … SELECT` that fills it from
/// its backing view, once the view is known to return one row per key. The
/// explicit column list comes from the view's own columns, which excludes the
/// table-only `created_at`/`updated_at` columns.
/// Taking the owner's guard makes running the statements as anyone else
/// unrepresentable.
fn rebuild_statements(
    _owner: &crate::owner::AsOwner,
    entity: &str,
) -> TViewResult<(String, String)> {
    use crate::catalog::TviewMeta;

    let meta = TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::TviewNotFound {
        name: entity.to_string(),
    })?;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let qi_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let view_columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    if view_columns.is_empty() {
        return Err(TViewError::CatalogError {
            operation: format!("Get columns for view {qi_view}"),
            pg_error: "View has no selectable columns".to_string(),
        });
    }
    let col_list = view_columns
        .iter()
        .map(|c| ident::quoted(c))
        .collect::<Vec<_>>()
        .join(", ");
    // A key names one row: a UNION view returning several for one is refused
    // before the fill, instead of failing on the table's primary key.
    crate::refresh::refuse_duplicate_keys(&meta, "true", &[])?;
    let insert = format!("INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}");
    Ok((qi_tv, insert))
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
    let graph = crate::flush::EntityDepGraph::load()?;
    for entity in &graph.topo_order {
        rebuild_one(entity)?;
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
    let results = Spi::connect(|client| {
        let args = vec![crate::utils::spi::text(entity)];
        let rows = client.select(
            &format!(
                "WITH RECURSIVE dep_tree AS (
                SELECT
                    pg_tview_meta.entity,
                    0 as depth,
                    ARRAY[pg_tview_meta.entity] as path,
                    pg_tview_meta.entity as depends_on
                FROM {meta} pg_tview_meta
                WHERE pg_tview_meta.entity = $1

                UNION ALL

                SELECT
                    m.entity,
                    dt.depth + 1,
                    dt.path || m.entity,
                    dt.entity as depends_on
                FROM dep_tree dt
                JOIN {meta} m ON m.plan->'embeds'
                    @> pg_catalog.jsonb_build_array(pg_catalog.jsonb_build_object('entity', dt.entity))
                WHERE NOT (m.entity = ANY(dt.path))
                  AND dt.depth < 10
            )
            SELECT depth, entity, depends_on
            FROM dep_tree
            ORDER BY depth, entity",
                meta = crate::utils::meta_table()
            ),
            None,
            &args,
        )?;
        let mut paths = Vec::new();
        for row in rows {
            let depth = row["depth"].value::<i32>()?.unwrap_or(0);
            let embedding = row["entity"].value::<String>()?.unwrap_or_default();
            let depends_on = row["depends_on"].value::<String>()?.unwrap_or_default();
            paths.push((depth, embedding, depends_on));
        }
        Ok::<_, spi::Error>(paths)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Read the cascade path", &e))?;
    Ok(results)
}
