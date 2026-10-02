//! Administrative SQL functions: refresh, migration, schema analysis, cascade path.

use crate::{TViewError, TViewResult, utils::quote_identifier};
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Analyze a SELECT statement and return inferred TVIEW schema as JSONB
///
/// Returns a JSON object with schema details on success, or `{"error": "..."}` on
/// failure. Never raises a `PostgreSQL` error so callers can use the result in
/// expressions (e.g., `IS NOT NULL`, `->>'error'`).
#[pg_extern]
fn pg_tviews_analyze_select(sql: &str) -> JsonB {
    match crate::schema::inference::infer_schema(sql) {
        Ok(schema) => match schema.to_jsonb() {
            Ok(jsonb) => jsonb,
            Err(e) => {
                JsonB(serde_json::json!({"error": format!("Failed to serialize schema: {e}")}))
            }
        },
        Err(e) => JsonB(serde_json::json!({"error": e.to_string()})),
    }
}

/// Infer column types from `PostgreSQL` catalog
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires Vec by value
fn pg_tviews_infer_types(table_name: &str, columns: Vec<String>) -> JsonB {
    match crate::schema::types::infer_column_types(table_name, &columns) {
        Ok(types) => match serde_json::to_value(&types) {
            Ok(json_value) => JsonB(json_value),
            Err(e) => {
                error!("Failed to serialize types to JSONB: {}", e);
            }
        },
        Err(e) => {
            error!("Type inference failed: {}", e);
        }
    }
}

/// Rebuild a TVIEW from its backing view, then every TVIEW whose view reads it,
/// directly or through others, in dependency order: a manual repair leaves
/// nothing stale.
///
/// Each rebuild is a `TRUNCATE` and an `INSERT … SELECT` with an explicit column
/// list (the view's own columns, so not the table-only `created_at`/`updated_at`),
/// and holds an ACCESS EXCLUSIVE lock on that TVIEW until the transaction ends.
/// `entity` is rebuilt with the caller's privileges; the TVIEWs that read it are
/// rebuilt as their owners, as a write's cascade is (issue #136).
///
/// # Errors
/// Returns error if the entity is not registered, the dependency graph cannot be
/// loaded, or a rebuild fails.
#[pg_extern]
fn pg_tviews_refresh(entity: &str) -> TViewResult<()> {
    crate::revision::check();
    rebuild_with_dependents(&[entity.to_string()], true)?;
    Ok(())
}

/// Rebuild `entities` and every TVIEW whose view reads one of them, transitively
/// (the readers of a TVIEW come from the complete dependency relation, not the
/// pruned one flush-time propagation follows), dependencies first. Readers are
/// rebuilt as their owners; `entities` too unless `requested_as_caller`. Returns
/// the rebuilt entities in order.
///
/// # Errors
/// Returns an error if the dependency graph cannot be loaded or a rebuild fails.
pub fn rebuild_with_dependents(
    entities: &[String],
    requested_as_caller: bool,
) -> TViewResult<Vec<String>> {
    let graph = crate::queue::graph::EntityDepGraph::load()?;
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
        if requested_as_caller && entities.contains(entity) {
            rebuild_one(entity)?;
        } else {
            let _owner = crate::owner::AsOwner::of_entity(entity)?;
            rebuild_one(entity)?;
        }
    }
    Ok(order)
}

/// Rebuild one TVIEW from its backing view (`TRUNCATE` + `INSERT … SELECT`), with
/// the current role's privileges, and nothing that reads it.
///
/// # Errors
/// Returns error if the entity is not registered or the truncate/insert fails.
pub fn rebuild_one(entity: &str) -> TViewResult<()> {
    let (qi_tv, insert) = rebuild_statements(entity)?;
    Spi::run(&format!("TRUNCATE {qi_tv}"))?;
    Spi::run(&insert)?;
    Ok(())
}

/// Populate an **empty** `tv_<entity>` from its backing view without `TRUNCATE`.
///
/// Used when a TVIEW is found empty while its view is not (an UNLOGGED table reset
/// by a crash restart or promotion, or a TVIEW created empty). Unlike
/// [`rebuild_one`] it takes only a ROW EXCLUSIVE lock, so readers are never
/// blocked, even when the transaction stays prepared (2PC) for a while.
///
/// # Errors
/// Returns error if the entity is not registered or the insert fails.
pub fn fill_empty_tview(entity: &str) -> TViewResult<()> {
    let (_, insert) = rebuild_statements(entity)?;
    Spi::run(&insert)?;
    Ok(())
}

/// The schema-qualified TVIEW table and the `INSERT … SELECT` that fills it from
/// its backing view. The explicit column list comes from the view's own
/// columns, which excludes the table-only `created_at`/`updated_at` columns.
fn rebuild_statements(entity: &str) -> TViewResult<(String, String)> {
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

/// Create the propagation indexes `(fk_<x>, pk_<entity>)` that TVIEWs created
/// before they became part of TVIEW creation are missing.
///
/// Cascade propagation looks parent rows up by their integer `fk_*` columns;
/// without an index that lookup scans the whole TVIEW. An `fk_*` column counts
/// as covered when **any** index on the TVIEW leads with it, so user-created
/// indexes are respected.
///
/// Returns the DDL for each missing index: executed, or only reported when
/// `dry_run` is true. Idempotent: a second call returns no rows. On large
/// TVIEWs run the reported statements by hand with `CREATE INDEX CONCURRENTLY`
/// (which cannot run inside a function).
///
/// Usage:
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes();`         -- all TVIEWs
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes('post');`   -- one entity
///   `SELECT * FROM pg_tviews_ensure_propagation_indexes(NULL, true);` -- dry run
///
/// # Errors
/// Returns error if the catalog query or an index creation fails.
#[pg_extern]
fn pg_tviews_ensure_propagation_indexes(
    entity: default!(Option<&str>, "NULL"),
    dry_run: default!(bool, false),
) -> Result<SetOfIterator<'static, String>, TViewError> {
    crate::revision::check();
    let missing = Spi::connect(|client| {
        let args = vec![unsafe {
            DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
        }];
        let rows = client.select(
            &format!(
                "SELECT n.nspname::text, c.relname::text, a.attname::text, 'pk_' || m.entity \
             FROM {meta} m \
             JOIN pg_class c ON c.oid = m.table_oid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
             WHERE ($1::text IS NULL OR m.entity = $1) \
               AND a.attname LIKE 'fk\\_%' \
               AND a.atttypid IN ('int2'::regtype, 'int4'::regtype, 'int8'::regtype) \
               AND a.attname <> 'pk_' || m.entity \
               AND EXISTS (SELECT 1 FROM pg_attribute p \
                           WHERE p.attrelid = c.oid AND p.attname = 'pk_' || m.entity \
                             AND NOT p.attisdropped) \
               AND NOT EXISTS (SELECT 1 FROM pg_index i \
                               WHERE i.indrelid = c.oid AND i.indkey[0] = a.attnum) \
             ORDER BY 1, 2, 3",
                meta = crate::utils::meta_table()
            ),
            None,
            &args,
        )?;
        let mut ddl = Vec::new();
        for row in rows {
            if let (Some(schema), Some(table), Some(fk), Some(pk)) = (
                row[1].value::<String>()?,
                row[2].value::<String>()?,
                row[3].value::<String>()?,
                row[4].value::<String>()?,
            ) {
                ddl.push(crate::ddl::create::propagation_index_ddl(
                    &schema, &table, &fk, &pk,
                ));
            }
        }
        Ok::<_, spi::Error>(ddl)
    })?;

    if !dry_run {
        for ddl in &missing {
            Spi::run(ddl)?;
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
fn pg_tviews_refresh_all_entities() -> TViewResult<()> {
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
    let graph = crate::queue::graph::EntityDepGraph::load()?;
    for entity in &graph.topo_order {
        rebuild_one(entity)?;
    }
    Ok(graph.topo_order)
}

/// Migrate all existing TVIEW triggers from the old PL/pgSQL handler to the
/// Rust `pg_tview_trigger_handler()`.
///
/// Call this once after upgrading `pg_tviews` to convert triggers installed by
/// prior versions. The operation is idempotent and safe to re-run.
///
/// Raises a `PostgreSQL` ERROR if any trigger cannot be migrated.
#[pg_extern]
fn pg_tviews_migrate_triggers() {
    crate::revision::check();
    if let Err(e) = crate::dependency::triggers::migrate_all_triggers_to_rust_handler() {
        error!("Failed to migrate triggers: {:?}", e);
    }
}

/// Show cascade dependency path for a given entity
///
/// Returns the dependency chain showing which TVIEWs depend on this entity
#[pg_extern]
fn pg_tviews_show_cascade_path(
    entity: &str,
) -> TableIterator<
    'static,
    (
        name!(depth, i32),
        name!(entity_name, String),
        name!(depends_on, String),
    ),
> {
    crate::revision::check();
    let results = Spi::connect(|client| {
        let args = vec![unsafe {
            DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
        }];
        match client.select(
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
                JOIN {meta} m ON ('fk_' || dt.entity) = ANY(m.fk_columns)
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
        ) {
            Ok(rows) => {
                let mut paths = Vec::new();
                for row in rows {
                    let depth = row["depth"].value::<i32>()?.unwrap_or(0);
                    let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
                    let depends_on = row["depends_on"].value::<String>()?.unwrap_or_default();
                    paths.push((depth, entity_name, depends_on));
                }
                Ok::<_, spi::Error>(paths)
            }
            Err(e) => {
                warning!("Failed to query cascade path: {}", e);
                Ok(Vec::new())
            }
        }
    })
    .unwrap_or_default();

    TableIterator::new(results)
}
