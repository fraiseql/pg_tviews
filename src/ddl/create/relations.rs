//! The relations a TVIEW is made of: its backing view and its table, created,
//! keyed and populated; OIDs and names resolved in the catalog.

use super::Storage;
use super::ViewColumns;
use super::index_name;
use super::indexes::{create_tview_indexes, storage_clause};
use crate::cascade_path;
use crate::error::TViewError;
use crate::error::TViewResult;
use crate::utils::log_debug;
use crate::utils::quote_identifier;
use pgrx::pg_sys;
use pgrx::pg_sys::Oid;
use pgrx::prelude::Spi;
use pgrx::prelude::notice;
use pgrx::spi;

/// Make a TVIEW's table keyed on its identity at re-registration (ADR 0169): drop
/// the unique index on `pk_<entity>` a DISTINCT ON TVIEW of beta.22 had (#164), and
/// add the primary key a table created without one lacks (a DISTINCT ON key named
/// `identifier`, `fk_*` or `*_id`). A primary key on another column is refused.
pub(crate) fn key_table_on_identity(
    schema_name: &str,
    tview_name: &str,
    identity: &str,
) -> TViewResult<()> {
    let table = relation_oid(schema_name, tview_name)?;
    let qualified = crate::utils::qualified_relname_from_oid(table)?;
    let catalog = |e: pgrx::spi::Error| TViewError::CatalogError {
        operation: format!("Read the keys of {qualified}"),
        pg_error: e.to_string(),
    };
    let pk_unique = index_name(tview_name, "pk_unique");
    let leftover = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_index i \
                        JOIN pg_catalog.pg_class c ON c.oid = i.indexrelid \
                        WHERE i.indrelid = $1 AND c.relname = $2)",
        &[
            crate::utils::spi::oid(table),
            crate::utils::spi::text(pk_unique.as_str()),
        ],
    )
    .map_err(catalog)?;
    if leftover == Some(true) {
        let sql = format!(
            "DROP INDEX {}.{}",
            quote_identifier(schema_name),
            quote_identifier(&pk_unique)
        );
        crate::utils::spi_run_ddl(&sql).map_err(|e| TViewError::SpiError {
            query: sql,
            error: e,
        })?;
    }
    let key = Spi::get_one_with_args::<Vec<String>>(
        "SELECT pg_catalog.array_agg(a.attname::text ORDER BY a.attnum) \
         FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary",
        &[crate::utils::spi::oid(table)],
    )
    .map_err(catalog)?
    .unwrap_or_default();
    match key.as_slice() {
        [column] if column == identity => Ok(()),
        [] => {
            let sql = format!(
                "ALTER TABLE {qualified} ADD PRIMARY KEY ({})",
                quote_identifier(identity)
            );
            crate::utils::spi_run_ddl(&sql).map_err(|e| TViewError::SpiError {
                query: sql,
                error: e,
            })
        }
        _ => Err(TViewError::DefinitionRefused {
            reason: format!(
                "{qualified} is keyed on ({}), but its rows are named by {identity}: \
                 pg_tviews_create_or_replace() with the same query rebuilds it",
                key.join(", ")
            ),
        }),
    }
}

/// The OID of relation `schema.name`.
pub(crate) fn relation_oid(schema: &str, name: &str) -> TViewResult<pg_sys::Oid> {
    find_relation(schema, name)?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Look up {schema}.{name}"),
        pg_error: "relation not found".to_string(),
    })
}

/// Whether relation `schema.name` exists.
pub(crate) fn relation_exists(schema: &str, name: &str) -> TViewResult<bool> {
    Ok(find_relation(schema, name)?.is_some())
}

pub(crate) fn find_relation(schema: &str, name: &str) -> TViewResult<Option<pg_sys::Oid>> {
    Spi::get_one_with_args::<pg_sys::Oid>(
        "SELECT (SELECT c.oid FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relname = $2)",
        &[
            crate::utils::spi::text(schema),
            crate::utils::spi::text(name),
        ],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Look up {schema}.{name}"),
        pg_error: e.to_string(),
    })
}

/// Re-resolve the relation OIDs stored inside serialized cascade paths against
/// the current catalog, using the same relname → OID map that creation built
/// from the backing view's base tables.
///
/// Cascade paths carry raw OIDs inside JSON text, which `pg_dump` copies
/// verbatim. After a restore those OIDs name nothing (or an unrelated
/// relation), so `pg_tview_meta`'s insert trigger calls this to rebind them.
/// For a freshly created TVIEW the result is identical to the input. A path
/// whose table can no longer be found is marked `unresolvable` (full-refresh
/// fallback) rather than left pointing at a stale OID.
pub fn rebind_cascade_paths(view_oid: Oid, cascade_paths: &[String]) -> TViewResult<Vec<String>> {
    if cascade_paths.is_empty() {
        return Ok(Vec::new());
    }

    let args = [crate::utils::spi::oid(view_oid)];
    let (view_name, schema_name) = Spi::get_two_with_args::<String, String>(
        "SELECT c.relname::text, n.nspname::text FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid WHERE c.oid = $1",
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Resolve backing view {view_oid:?}"),
        pg_error: e.to_string(),
    })?;
    let (Some(view_name), Some(schema_name)) = (view_name, schema_name) else {
        return Err(TViewError::CatalogError {
            operation: format!("Resolve backing view {view_oid:?}"),
            pg_error: "view not found".to_string(),
        });
    };

    let base_table_oids = crate::dependency::find_base_tables(&view_name, Some(&schema_name))?;
    let oid_map = build_oid_name_map(&base_table_oids)?;

    cascade_paths
        .iter()
        .map(|json| {
            let mut path: cascade_path::CascadePath =
                serde_json::from_str(json).map_err(|e| TViewError::CatalogError {
                    operation: "Parse cascade path".to_string(),
                    pg_error: e.to_string(),
                })?;
            match oid_map.get(&path.source_table) {
                Some(oid) => path.source_oid = *oid,
                None => path.unresolvable = true,
            }
            for hop in &mut path.hops {
                match oid_map.get(&hop.table_name) {
                    Some(oid) => hop.table_oid = *oid,
                    None => path.unresolvable = true,
                }
            }
            serde_json::to_string(&path).map_err(|e| TViewError::CatalogError {
                operation: "Serialize cascade path".to_string(),
                pg_error: e.to_string(),
            })
        })
        .collect()
}

/// Build a map from table name → OID for a set of base table OIDs.
pub(crate) fn build_oid_name_map(
    oids: &[pg_sys::Oid],
) -> TViewResult<std::collections::HashMap<String, pg_sys::Oid>> {
    use std::collections::HashMap;

    if oids.is_empty() {
        return Ok(HashMap::new());
    }

    let oid_list = oids
        .iter()
        .map(|o| o.to_u32().to_string())
        .collect::<Vec<_>>()
        .join(",");

    let query = format!("SELECT oid, relname::text FROM pg_class WHERE oid IN ({oid_list})");

    let mut map = HashMap::new();
    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        for row in rows {
            let oid: pg_sys::Oid = row["oid"].value()?.unwrap_or(pg_sys::Oid::INVALID);
            let name: String = row["relname"].value()?.unwrap_or_default();
            map.insert(name, oid);
        }
        Ok::<_, spi::Error>(())
    })?;

    Ok(map)
}

/// Columns of `source_table` that the backing view `view_oid` reads, directly or
/// through views, from `PostgreSQL`'s own column-level `pg_depend` records: the
/// exact set of source columns whose change can alter a target TVIEW row.
///
/// Empty when they cannot be read; the caller treats an empty result as "unknown
/// ⇒ always refresh", so a miss is never unsafe.
pub(crate) fn view_source_columns(view_oid: Oid, source_oid: Oid) -> Vec<String> {
    match crate::catalog::reads::view_columns_read(view_oid, source_oid) {
        Ok(columns) => columns.into_iter().map(|(name, _)| name).collect(),
        Err(e) => {
            notice!(
                "view_source_columns({view_oid:?}, {source_oid:?}): {e} — cascade will always refresh"
            );
            Vec::new()
        }
    }
}

/// Check if a TVIEW already exists
pub(crate) fn tview_exists(tview_name: &str) -> TViewResult<bool> {
    let entity_name = tview_name.trim_start_matches("tv_");
    let args = vec![crate::utils::spi::text(entity_name)];

    Spi::get_one_with_args::<bool>(
        &format!(
            "SELECT COUNT(*) > 0 FROM {} WHERE entity = $1",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW exists: {tview_name}"),
        pg_error: e.to_string(),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Create the backing view that contains the user's SELECT definition
pub(crate) fn create_backing_view(
    view_name: &str,
    select_sql: &str,
    schema_name: &str,
) -> TViewResult<()> {
    let qi_schema = quote_identifier(schema_name);
    let qi_view = quote_identifier(view_name);
    let create_view_sql = format!("CREATE VIEW {qi_schema}.{qi_view} AS {select_sql}");

    log_debug!(
        "create_backing_view START - schema='{}', view='{}', sql_len={}",
        schema_name,
        view_name,
        create_view_sql.len()
    );

    match crate::utils::spi_run_ddl(&create_view_sql) {
        Ok(()) => {
            // Log successful spi_run_ddl
            log_debug!("spi_run_ddl SUCCEEDED for {}.{}", schema_name, view_name);
        }
        Err(e) => {
            // Log spi_run_ddl failure
            log_debug!(
                "spi_run_ddl FAILED - {}.{} - error: {}",
                schema_name,
                view_name,
                e
            );
            return Err(TViewError::SpiError {
                query: create_view_sql.clone(),
                error: e,
            });
        }
    }

    // Log before verification check
    log_debug!(
        "checking if view exists - schema='{}', view='{}' in pg_class",
        schema_name,
        view_name
    );

    // Verify the view was created (schema-qualified to avoid false positives across schemas)
    let check_args = vec![
        crate::utils::spi::text(view_name),
        crate::utils::spi::text(schema_name),
    ];
    let exists = match Spi::get_one_with_args::<i32>(
        "SELECT 1 FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.relname = $1 AND n.nspname = $2 AND c.relkind = 'v'",
        &check_args,
    ) {
        Ok(result) => {
            if result.is_some() {
                // Log successful verification
                log_debug!(
                    "VERIFIED - backing view {}.{} exists in pg_class",
                    schema_name,
                    view_name
                );
                true
            } else {
                // Log verification failure
                log_debug!(
                    "VERIFICATION FAILED - backing view {}.{} not found in pg_class after spi_run_ddl",
                    schema_name,
                    view_name
                );
                false
            }
        }
        Err(e) => {
            // Log verification query failure
            log_debug!(
                "verification query FAILED - could not check pg_class: {}",
                e
            );
            return Err(TViewError::SpiError {
                query: format!("Check view {schema_name}.{view_name} exists"),
                error: e.to_string(),
            });
        }
    };

    if !exists {
        return Err(TViewError::CatalogError {
            operation: format!("Create view {schema_name}.{view_name}"),
            pg_error: "View was not created (CREATE VIEW succeeded but view missing from pg_class)"
                .to_string(),
        });
    }

    Ok(())
}

/// Create the materialized table with proper schema inferred from the backing view,
/// with its primary key on the TVIEW's identity column (ADR 0169).
pub(crate) fn create_materialized_table(
    tview_name: &str,
    schema: &ViewColumns,
    schema_name: &str,
    identity: &str,
    storage: Storage,
    view_oid: pg_sys::Oid,
) -> TViewResult<()> {
    let qi_schema = quote_identifier(schema_name);
    let qi_tview = quote_identifier(tview_name);
    // `<name> <type>`, with PRIMARY KEY on the identity column.
    let key = |name: &str| if name == identity { " PRIMARY KEY" } else { "" };

    // Build column definitions based on inferred schema
    let mut columns = Vec::new();

    // pk_<entity>: the row identity, or a plain column (DISTINCT ON another key)
    if let Some(pk) = &schema.pk {
        columns.push(format!("{} BIGINT{}", quote_identifier(pk), key(pk)));
    }

    // ID column (Trinity identifier)
    if let Some(id) = &schema.id {
        let not_null = if id == identity { key(id) } else { " NOT NULL" };
        columns.push(format!("{} UUID{not_null}", quote_identifier(id)));
    }

    // Every other column takes the backing view's type, typmod included and
    // schema-qualified (an enum, a domain, a composite, `numeric(6,2)`, `bit(4)`).
    // The convention columns above keep their fixed types: the key is BIGINT,
    // `id` UUID, `data` JSONB and `fk_*` BIGINT whatever the view computes them as.
    let view_types: std::collections::HashMap<String, String> =
        crate::utils::column_types(view_oid)?.into_iter().collect();
    let view_type = |col: &str, fallback: &str| {
        view_types
            .get(col)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    };

    // Identifier column (optional Trinity identifier)
    if let Some(identifier) = &schema.identifier {
        columns.push(format!(
            "{} {}{}",
            quote_identifier(identifier),
            view_type(identifier, "TEXT"),
            key(identifier)
        ));
    }

    // Data column (JSONB read model)
    if let Some(data) = &schema.data {
        columns.push(format!("{} JSONB", quote_identifier(data)));
    }

    // Foreign key columns (for lineage tracking)
    for fk in &schema.fk {
        columns.push(format!("{} BIGINT{}", quote_identifier(fk), key(fk)));
    }

    // UUID foreign key columns (for filtering): a column named `*_id` may be TEXT.
    for uuid_fk in &schema.uuid_fk {
        columns.push(format!(
            "{} {}{}",
            quote_identifier(uuid_fk),
            view_type(uuid_fk, "UUID"),
            key(uuid_fk)
        ));
    }

    for (col_name, col_type) in &schema.additional {
        columns.push(format!(
            "{} {}{}",
            quote_identifier(col_name),
            view_type(col_name, col_type),
            key(col_name)
        ));
    }
    if !columns.iter().any(|c| c.ends_with(" PRIMARY KEY")) {
        return Err(TViewError::DefinitionRefused {
            reason: format!("{tview_name} has no column {identity} to key its rows on"),
        });
    }

    // Add timestamps for tracking
    columns.push("created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());
    columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());

    let columns_sql = columns.join(",\n    ");

    let unlogged_keyword = if storage.logged { "" } else { "UNLOGGED " };
    let with = storage_clause(storage.fillfactor);
    let create_table_sql = format!(
        "CREATE {unlogged_keyword}TABLE {qi_schema}.{qi_tview} (\n    {columns_sql}\n){with}"
    );

    crate::utils::spi_run_ddl(&create_table_sql).map_err(|e| TViewError::SpiError {
        query: create_table_sql,
        error: e,
    })?;

    // Create indexes for performance
    create_tview_indexes(tview_name, schema, schema_name, storage.data_gin_index)?;

    Ok(())
}

/// Populate the materialized table with initial data from the backing view, and
/// return the number of rows.
pub(crate) fn populate_initial_data(
    tview_name: &str,
    schema_name: &str,
    view_oid: pg_sys::Oid,
) -> TViewResult<u64> {
    // Get actual column names from the backing view (like pg_tviews_refresh does)
    // This ensures consistency and handles any discrepancies between inferred schema and actual view
    let view = crate::utils::qualified_relname_from_oid(view_oid)?;
    let view_columns = crate::utils::get_view_columns_by_oid(view_oid)?;

    if view_columns.is_empty() {
        return Err(TViewError::CatalogError {
            operation: format!("Get columns for view {view}"),
            pg_error: "View has no selectable columns".to_string(),
        });
    }

    // Use the actual view columns for both insert and select
    let insert_columns = view_columns;

    let qi_schema = quote_identifier(schema_name);
    let qi_tview = quote_identifier(tview_name);
    let col_list = insert_columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let insert_sql = format!(
        "INSERT INTO {qi_schema}.{qi_tview} ({col_list}) \
         SELECT {col_list} FROM {view}"
    );
    // Rendered as every refresh renders (#200); the definition itself was parsed
    // under the caller's settings, as CREATE VIEW parses it.
    let _pin = crate::owner::RenderPin::new();

    let rows = Spi::connect_mut(|client| client.update(&insert_sql, None, &[]).map(|t| t.len()))
        .map_err(|e| TViewError::SpiError {
            query: insert_sql,
            error: e.to_string(),
        })?;

    Ok(rows as u64)
}
