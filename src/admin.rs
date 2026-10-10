//! Administrative SQL functions: refresh, migration, cascade path.

use crate::{TViewError, TViewResult, utils::quote_identifier};
use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Rebuild a TVIEW from its backing view, then every TVIEW whose view reads it,
/// directly or through others, in dependency order: a manual repair leaves
/// nothing stale.
///
/// Each rebuild is a `TRUNCATE` and an `INSERT … SELECT` with an explicit column
/// list (the view's own columns, so not the table-only `created_at`/`updated_at`),
/// and holds an ACCESS EXCLUSIVE lock on that TVIEW until the transaction ends.
/// Like `REFRESH MATERIALIZED VIEW`, it requires owning the TVIEW (or the
/// extension), and every TVIEW is rebuilt as its owner.
///
/// # Errors
/// Returns error if the caller does not own the TVIEW, the entity is not
/// registered, the dependency graph cannot be loaded, or a rebuild fails.
#[pg_extern]
fn pg_tviews_refresh(entity: &str) -> Result<(), ErrorReport> {
    crate::revision::check();
    let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
        TViewError::MetadataNotFound {
            entity: entity.to_string(),
        }
    })?;
    crate::owner::require_owner(meta.tview_oid, &format!("tv_{entity}"))?;
    rebuild_with_dependents(&[entity.to_string()])?;
    Ok(())
}

/// Bring the TVIEWs whose definitions read the current time up to date:
/// `tview`, or every such TVIEW the caller owns (or may change as a member of its
/// owner's role). Each is refreshed in full as a write to a `full_refresh` table
/// would refresh it, then the TVIEWs reading it are, through the flush. For
/// `pg_cron` or the application to call at the boundary its rows depend on (the
/// day, for `CURRENT_DATE`). Returns the TVIEWs refreshed, dependencies first.
///
/// # Errors
/// Returns an error if `tview` is not a TVIEW, reads no time or is not the
/// caller's, or a refresh fails.
#[pg_extern]
fn pg_tviews_refresh_time_dependent(
    tview: default!(Option<&str>, "NULL"),
) -> Result<SetOfIterator<'static, String>, ErrorReport> {
    crate::revision::check();
    let rows: Vec<(String, pgrx::pg_sys::Oid, bool, bool)> = Spi::connect(|client| {
        let mut rows = Vec::new();
        for row in client.select(
            &format!(
                "SELECT m.entity::text, m.table_oid::oid, m.time_dependent, \
                        pg_catalog.pg_has_role(c.relowner, 'USAGE') \
                 FROM {} m JOIN pg_catalog.pg_class c ON c.oid = m.table_oid \
                 WHERE $1::text IS NULL \
                    OR m.table_oid::oid = $1::text::pg_catalog.regclass::pg_catalog.oid",
                crate::utils::meta_table()
            ),
            None,
            &[crate::utils::spi::text(tview)],
        )? {
            if let (Some(entity), Some(table)) =
                (row.get::<String>(1)?, row.get::<pgrx::pg_sys::Oid>(2)?)
            {
                rows.push((
                    entity,
                    table,
                    row.get::<bool>(3)?.unwrap_or(false),
                    row.get::<bool>(4)?.unwrap_or(false),
                ));
            }
        }
        Ok::<_, pgrx::spi::Error>(rows)
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Find the time-dependent TVIEWs".to_string(),
        pg_error: e.to_string(),
    })?;
    let chosen: Vec<(String, pgrx::pg_sys::Oid)> = match tview {
        Some(name) => {
            let Some((entity, table, dependent, _)) = rows.into_iter().next() else {
                return Err(TViewError::InvalidInput {
                    parameter: "tview".to_string(),
                    reason: format!("{name} is not a TVIEW"),
                }
                .into());
            };
            if !dependent {
                return Err(TViewError::InvalidInput {
                    parameter: "tview".to_string(),
                    reason: format!("{name} does not read the time: nothing to refresh"),
                }
                .into());
            }
            crate::owner::require_owner(table, name)?;
            vec![(entity, table)]
        }
        None => rows
            .into_iter()
            .filter(|(_, _, dependent, owned)| *dependent && *owned)
            .map(|(entity, table, _, _)| (entity, table))
            .collect(),
    };
    let order = crate::flush::EntityDepGraph::load()?.topo_order;
    let mut chosen = chosen;
    chosen.sort_by_key(|(entity, _)| order.iter().position(|e| e == entity).unwrap_or(usize::MAX));
    let mut refreshed = Vec::new();
    for (entity, table) in &chosen {
        if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
            crate::suspend::record_change(entity);
        } else {
            crate::queue::enqueue_refresh_all(entity);
        }
        refreshed.push(crate::utils::qualified_relname_from_oid(*table)?);
    }
    crate::flush::flush_refresh_queue()?;
    Ok(SetOfIterator::new(refreshed))
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
        TViewError::MetadataNotFound {
            entity: entity.to_string(),
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
/// its backing view. The explicit column list comes from the view's own
/// columns, which excludes the table-only `created_at`/`updated_at` columns.
/// Taking the owner's guard makes running the statements as anyone else
/// unrepresentable.
fn rebuild_statements(
    _owner: &crate::owner::AsOwner,
    entity: &str,
) -> TViewResult<(String, String)> {
    use crate::catalog::TviewMeta;

    let meta = TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::MetadataNotFound {
        entity: entity.to_string(),
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
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!("INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}");
    Ok((qi_tv, insert))
}

/// Create the propagation indexes `(<lookup>, <identity>)` that TVIEWs created
/// before they became part of TVIEW creation are missing.
///
/// A TVIEW's rows are looked up by the columns holding an embedded TVIEW's key
/// and those a fan-out patch writes through (the plan's lookup columns); without
/// an index each lookup scans the whole TVIEW. A lookup column counts as covered when **any** index on the TVIEW
/// leads with it, so user-created indexes are respected.
///
/// Returns the DDL for each missing index: executed, or only reported when
/// `dry_run` is true. Idempotent: a second call returns no rows. On large
/// TVIEWs run the reported statements by hand with `CREATE INDEX CONCURRENTLY`
/// (which cannot run inside a function). The indexes it creates, and those its
/// statements created when run by hand, are `pg_tviews`' (listed in
/// `tviews.registry.managed_indexes`). Requires owning each TVIEW (or the
/// extension).
///
/// Usage:
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes();`         -- all TVIEWs
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes('post');`   -- one entity
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes(NULL, true);` -- dry run
///
/// # Errors
/// Returns error if the catalog query or an index creation fails, or the caller
/// does not own a TVIEW.
#[pg_extern]
fn pg_tviews_ensure_propagation_indexes(
    entity: default!(Option<&str>, "NULL"),
    dry_run: default!(bool, false),
) -> Result<SetOfIterator<'static, String>, ErrorReport> {
    crate::revision::check();
    let metas = match entity {
        Some(entity) => vec![
            crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
                TViewError::MetadataNotFound {
                    entity: entity.to_string(),
                }
            })?,
        ],
        None => crate::catalog::TviewMeta::load_all()?,
    };
    let mut missing = Vec::new();
    for meta in &metas {
        let tview = format!("tv_{}", meta.entity_name);
        crate::owner::require_owner(meta.tview_oid, &tview)?;
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

    Ok(SetOfIterator::new(missing))
}

/// Refresh all TVIEWs in the database, dependencies first.
/// This is a convenience function for bulk operations like schema migrations
/// or data seeding workflows.
///
/// # Errors
/// Returns error if any TVIEW cannot be refreshed
#[pg_extern]
fn pg_tviews_refresh_all_entities() -> Result<(), ErrorReport> {
    crate::revision::check();
    let order = refresh_all_in_dependency_order()?;
    if order.is_empty() {
        info!("No TVIEWs found to refresh");
    } else {
        info!("Successfully refreshed {} TVIEWs", order.len());
    }
    Ok(())
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

/// Show cascade dependency path for a given entity
///
/// Returns the dependency chain showing which TVIEWs depend on this entity
#[pg_extern]
fn pg_tviews_show_cascade_path(
    entity: &str,
) -> Result<
    TableIterator<
        'static,
        (
            name!(depth, i32),
            name!(entity_name, String),
            name!(depends_on, String),
        ),
    >,
    ErrorReport,
> {
    crate::revision::check();
    if crate::catalog::TviewMeta::load_by_entity(entity)?.is_none() {
        return Err(TViewError::MetadataNotFound {
            entity: entity.to_string(),
        }
        .into());
    }
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
            SELECT depth, entity AS entity_name, depends_on
            FROM dep_tree
            ORDER BY depth, entity_name",
                meta = crate::utils::meta_table()
            ),
            None,
            &args,
        )?;
        let mut paths = Vec::new();
        for row in rows {
            let depth = row["depth"].value::<i32>()?.unwrap_or(0);
            let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
            let depends_on = row["depends_on"].value::<String>()?.unwrap_or_default();
            paths.push((depth, entity_name, depends_on));
        }
        Ok::<_, spi::Error>(paths)
    })
    .map_err(|e| crate::utils::spi::catalog_error("Read the cascade path", &e))?;

    Ok(TableIterator::new(results))
}
