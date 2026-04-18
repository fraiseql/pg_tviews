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

/// Force a full refresh of all rows in a TVIEW from its backing view.
///
/// Rebuilds the materialized table by truncating and re-inserting from the
/// backing view using an explicit column list. The explicit list is derived
/// from the view's own columns via `pg_attribute`, which excludes the
/// table-only `created_at`/`updated_at` columns (they carry `DEFAULT NOW()`
/// and must not appear in the `SELECT *` projection of the view).
///
/// This avoids the column-count mismatch that a naive
/// `INSERT INTO tv_entity SELECT * FROM v_entity` would produce when the
/// materialized table has extra timestamp columns the view does not.
///
/// # Errors
/// Returns error if the entity is not registered, the view/table OIDs cannot
/// be resolved, or the truncate/insert operations fail.
#[pg_extern]
fn pg_tviews_refresh(entity: &str) -> TViewResult<()> {
    use crate::catalog::TviewMeta;

    let meta = TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::MetadataNotFound {
        entity: entity.to_string(),
    })?;

    let tv_name = crate::utils::relname_from_oid(meta.tview_oid)?;
    let view_name = crate::utils::lookup_view_for_source(meta.view_oid)?;

    let view_columns = crate::utils::get_view_columns_by_oid(meta.view_oid)?;

    if view_columns.is_empty() {
        return Err(TViewError::CatalogError {
            operation: format!("Get columns for view {view_name}"),
            pg_error: "View has no selectable columns".to_string(),
        });
    }

    let col_list = view_columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");
    let qi_tv = quote_identifier(&tv_name);
    let qi_view = quote_identifier(&view_name);

    Spi::run(&format!("TRUNCATE {qi_tv}"))?;
    Spi::run(&format!(
        "INSERT INTO {qi_tv} ({col_list}) SELECT {col_list} FROM {qi_view}"
    ))?;

    Ok(())
}

/// Refresh all TVIEWs in the database.
/// This is a convenience function for bulk operations like schema migrations
/// or data seeding workflows.
///
/// # Errors
/// Returns error if any TVIEW cannot be refreshed
#[pg_extern]
fn pg_tviews_refresh_all() -> TViewResult<()> {
    use crate::catalog::TviewMeta;

    // Get all TVIEW metadata
    let all_tviews = TviewMeta::load_all()?;

    if all_tviews.is_empty() {
        info!("No TVIEWs found to refresh");
        return Ok(());
    }

    // Refresh all TVIEWs (simplified - no complex dependency ordering needed for bulk refresh)
    for meta in &all_tviews {
        info!("Refreshing TVIEW for entity: {}", meta.entity_name);
        pg_tviews_refresh(&meta.entity_name)?;
    }

    info!("Successfully refreshed {} TVIEWs", all_tviews.len());
    Ok(())
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
    let results = Spi::connect(|client| {
        let args = vec![unsafe {
            DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
        }];
        match client.select(
            "WITH RECURSIVE dep_tree AS (
                SELECT
                    pg_tview_meta.entity,
                    0 as depth,
                    ARRAY[pg_tview_meta.entity] as path,
                    pg_tview_meta.entity as depends_on
                FROM pg_tview_meta
                WHERE pg_tview_meta.entity = $1

                UNION ALL

                SELECT
                    m.entity,
                    dt.depth + 1,
                    dt.path || m.entity,
                    dt.entity as depends_on
                FROM dep_tree dt
                JOIN pg_tview_meta m ON ('fk_' || dt.entity) = ANY(m.fk_columns)
                WHERE NOT (m.entity = ANY(dt.path))
                  AND dt.depth < 10
            )
            SELECT depth, entity AS entity_name, depends_on
            FROM dep_tree
            ORDER BY depth, entity_name",
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
