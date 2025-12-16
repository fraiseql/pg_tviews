//! Convert existing tables to TVIEWs
//!
//! This module handles converting a table that was created by standard
//! `PostgreSQL` DDL into a proper TVIEW structure.

use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};
use crate::schema::TViewSchema;

/// Convert an existing table to a TVIEW
///
/// # Strategy
///
/// `PostgreSQL` has already created `tv_entity` as a regular table.
/// We need to:
/// 1. Validate it has TVIEW structure (pk_*, id, data columns)
/// 2. Extract the data
/// 3. Create backing view `v_entity` (reconstructed SELECT)
/// 4. Recreate `tv_entity` as a view that reads from `v_entity`
/// 5. Install triggers on base tables
/// 6. Populate metadata
///
/// # Challenges
///
/// - We don't have the original SELECT statement
/// - Must infer base tables from data
/// - Must handle edge cases (empty tables, complex JOINs)
///
/// # Errors
/// Returns error if table doesn't exist, conversion fails, or rollback is needed
pub fn convert_existing_table_to_tview(
    table_name: &str,
) -> TViewResult<()> {
    let entity_name = extract_entity_name(table_name)?;

    info!("Converting existing table '{}' to TVIEW", table_name);

    // Create savepoint in case conversion fails
    Spi::run("SAVEPOINT tview_conversion")?;
    info!("Created savepoint for TVIEW conversion");

    match do_conversion(table_name, entity_name) {
        Ok(()) => {
            Spi::run("RELEASE SAVEPOINT tview_conversion")?;
            info!("Successfully converted '{}' to TVIEW", table_name);
            Ok(())
        }
        Err(e) => {
            Spi::run("ROLLBACK TO SAVEPOINT tview_conversion")?;
            Err(e)
        }
    }
}

fn do_conversion(table_name: &str, entity_name: &str) -> TViewResult<()> {
    // Step 1: Validate structure
    validate_tview_structure(table_name, entity_name)?;

    // Step 2: Infer schema from table
    let schema = infer_schema_from_table(table_name)?;

    // Step 3: Extract existing data (will be restored later)
    let data_backup = backup_table_data(table_name, &schema)?;

    // Step 4: Get base tables (infer from data or require user hint)
    let base_tables = infer_base_tables(table_name)?;

    // Step 5: Drop the existing table
    Spi::run(&format!("DROP TABLE {table_name} CASCADE"))?;
    info!("Dropped existing table '{}'", table_name);

    // Step 6: Reconstruct as proper TVIEW
    reconstruct_as_tview(
        table_name,
        entity_name,
        &schema,
        &base_tables,
        &data_backup,
    )?;

    Ok(())
}

/// Validate that table has required TVIEW structure
fn validate_tview_structure(table_name: &str, entity_name: &str) -> TViewResult<()> {
    let pk_col = format!("pk_{entity_name}");

    // Check required columns exist
    let columns = get_table_columns(table_name)?;

    let has_id = columns.iter().any(|c| c.name == "id");
    let has_data = columns.iter().any(|c| c.name == "data");

    // Only require id + data columns
    // Optional optimization columns: pk_<entity>, fk_<entity>, path (LTREE), <entity>_id (UUID FKs)
    if !has_id || !has_data {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!(
                "Table must have TVIEW structure: id (UUID), data (JSONB). Found: {}",
                columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", ")
            ),
        });
    }

    // Log presence of optional optimization columns
    let has_pk = columns.iter().any(|c| c.name == pk_col);
    if has_pk {
        info!("TVIEW '{}' has pk_{} column for optimized queries", table_name, entity_name);
    }

    // Validate types
    let id_col = columns.iter().find(|c| c.name == "id").unwrap();
    if id_col.data_type != "uuid" {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!("Column 'id' must be UUID, found {}", id_col.data_type),
        });
    }

    let data_col = columns.iter().find(|c| c.name == "data").unwrap();
    if data_col.data_type != "jsonb" {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!("Column 'data' must be JSONB, found {}", data_col.data_type),
        });
    }

    Ok(())
}

#[derive(Debug)]
struct ColumnInfo {
    name: String,
    data_type: String,
    #[allow(dead_code)]
    is_nullable: bool,
}

fn get_table_columns(table_name: &str) -> TViewResult<Vec<ColumnInfo>> {
    let mut columns = Vec::new();

    Spi::connect(|client| {
        let query = format!(
            "SELECT column_name, data_type, is_nullable
             FROM information_schema.columns
             WHERE table_name = '{}'
             ORDER BY ordinal_position",
            table_name.replace('\'', "''")
        );

        let results = client.select(&query, None, &[])?;

        for row in results {
            columns.push(ColumnInfo {
                name: row["column_name"].value()?.unwrap_or_default(),
                data_type: row["data_type"].value()?.unwrap_or_default(),
                is_nullable: row["is_nullable"].value::<String>()?.unwrap_or_default() == "YES",
            });
        }

        Ok::<_, spi::Error>(())
    })?;

    Ok(columns)
}

fn infer_schema_from_table(table_name: &str) -> TViewResult<TViewSchema> {
    // For Phase 2, create a basic schema
    // In Phase 3, we can make this more sophisticated
    let columns = get_table_columns(table_name)?;

    // Find the pk_* column
    let pk_col = columns.iter().find(|c| c.name.starts_with("pk_")).unwrap();
    let entity_name = pk_col.name.strip_prefix("pk_").unwrap();

    // Create a simple schema - in practice this would be more complex
    // For now, we'll assume a simple case
    Ok(TViewSchema {
        entity_name: Some(entity_name.to_string()),
        pk_column: Some(pk_col.name.clone()),
        id_column: Some("id".to_string()),
        data_column: Some("data".to_string()),
        identifier_column: Some(pk_col.name.clone()),
        fk_columns: vec![],
        uuid_fk_columns: vec![],
        additional_columns: vec![],
        additional_columns_with_types: vec![],
    })
}

fn backup_table_data(table_name: &str, _schema: &TViewSchema) -> TViewResult<Vec<BackupRow>> {
    let backup = Spi::connect(|client| {
        let query = format!("SELECT * FROM {table_name}");
        let results = client.select(&query, None, &[])?;

        let mut backup = Vec::new();

        // Handle empty tables gracefully
        if results.is_empty() {
            info!("Table '{}' is empty, no data to backup", table_name);
            return Ok::<_, spi::Error>(backup);
        }

        for row in results {
            // Extract actual row data - handle potential NULL values
            let id = row["id"].value()?; // UUID can be NULL in some cases
            let data = row["data"].value()?; // JSONB should not be NULL but handle gracefully

            backup.push(BackupRow {
                id,
                data,
            });
        }

        Ok::<_, spi::Error>(backup)
    })?;

    info!("Backed up {} rows from table '{}'", backup.len(), table_name);
    Ok(backup)
}

fn infer_base_tables(table_name: &str) -> TViewResult<Vec<String>> {
    // First, check for user-provided hints in table comment
    if let Some(hinted_tables) = get_base_table_hints(table_name)? {
        info!("Using user-provided base table hints for '{}': {:?}", table_name, hinted_tables);
        return Ok(hinted_tables);
    }

    // Try to infer base tables from data patterns
    // This is a basic implementation for Phase 4
    let inferred = infer_base_tables_from_data(table_name)?;
    if !inferred.is_empty() {
        info!("Inferred base tables for '{}': {:?}", table_name, inferred);
        return Ok(inferred);
    }

    // No hints or inference possible, skip trigger installation
    info!("No base table hints or inference possible for '{}', skipping trigger installation", table_name);
    Ok(Vec::new())
}

/// Try to infer base tables from the data in the TVIEW
/// This is a heuristic approach for simple cases
fn infer_base_tables_from_data(table_name: &str) -> TViewResult<Vec<String>> {
    let mut base_tables = Vec::new();

    Spi::connect(|client| {
        // Sample a few rows to analyze data patterns
        let query = format!("SELECT data FROM {table_name} LIMIT 5");
        let results = client.select(&query, None, &[])?;

        for row in results {
            if let Some(data) = row["data"].value::<String>()? {
                // Try to extract table references from JSONB data
                // Look for patterns like "fk_table": value or "table_id": value
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&data) {
                    extract_table_references(&json_value, &mut base_tables);
                }
            }
        }

        Ok::<_, spi::Error>(())
    })?;

    // Remove duplicates and filter
    base_tables.sort();
    base_tables.dedup();

    // Only return tables that actually exist in the database
    let existing_tables: Vec<String> = base_tables
        .into_iter()
        .filter(|table| table_exists(table))
        .collect();

    Ok(existing_tables)
}

/// Extract potential table references from JSONB data
fn extract_table_references(json: &serde_json::Value, tables: &mut Vec<String>) {
    match json {
        serde_json::Value::Object(obj) => {
            for (key, value) in obj {
                // Look for FK patterns: fk_<table>, <table>_id
                if key.starts_with("fk_") && key.len() > 3 {
                    let table_name = format!("tb_{}", &key[3..]);
                    tables.push(table_name);
                } else if key.ends_with("_id") && key.len() > 3 {
                    let table_name = format!("tb_{}", &key[..key.len() - 3]);
                    tables.push(table_name);
                }

                // Recursively check nested objects
                extract_table_references(value, tables);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                extract_table_references(item, tables);
            }
        }
        _ => {}
    }
}

/// Check if a table exists in the current database
fn table_exists(table_name: &str) -> bool {
    Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = '{}')",
        table_name.replace('\'', "''")
    )).unwrap_or(Some(false)).unwrap_or(false)
}

/// Check for user-provided base table hints in table comment
/// Format: COMMENT ON TABLE `tv_entity` IS '`TVIEW_BASES`: `tb_table1`, `tb_table2`';
fn get_base_table_hints(table_name: &str) -> TViewResult<Option<Vec<String>>> {
    let query = format!(
        "SELECT obj_description(oid, 'pg_class') as comment
         FROM pg_class
         WHERE relname = '{}'",
        table_name.replace('\'', "''")
    );

    let comment: Option<String> = Spi::get_one(&query)?;

    if let Some(comment) = comment {
        // Look for TVIEW_BASES: pattern
        if let Some(bases_part) = comment
            .split("TVIEW_BASES:")
            .nth(1)
            .map(|s| s.trim())
        {
            // Parse comma-separated list
            let tables: Vec<String> = bases_part
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if !tables.is_empty() {
                return Ok(Some(tables));
            }
        }
    }

    Ok(None)
}

fn reconstruct_as_tview(
    table_name: &str,
    entity_name: &str,
    schema: &TViewSchema,
    _base_tables: &[String],
    data_backup: &[BackupRow],
) -> TViewResult<()> {
    // Step 1: Create the backing view
    let view_name = format!("v_{entity_name}");

    // Create view that preserves the backed up data
    if data_backup.is_empty() {
        // Empty table: create view with proper structure but no rows
        Spi::run(&format!(
            "CREATE VIEW {view_name} AS SELECT
                NULL::uuid as id,
                NULL::jsonb as data
             WHERE false"
        ))?;
        info!("Created empty view {} for empty table", view_name);
    } else {
        // Non-empty table: reconstruct with actual data
        let mut values = Vec::new();
        for row in data_backup {
            if let (Some(id), Some(data)) = (&row.id, &row.data) {
                values.push(format!("('{id}'::uuid, '{data}')"));
            }
        }

        Spi::run(&format!(
            "CREATE VIEW {} AS SELECT * FROM (VALUES {}) AS t(id, data)",
            view_name, values.join(", ")
        ))?;
        info!("Created view {} with {} rows", view_name, data_backup.len());
    }

    // Step 2: Create the TVIEW wrapper
    Spi::run(&format!(
        "CREATE VIEW {table_name} AS SELECT * FROM {view_name}"
    ))?;
    info!("Created wrapper view {}", table_name);

    // Step 3: Register metadata
    register_tview_metadata(entity_name, &view_name, table_name, schema)?;

    Ok(())
}

fn register_tview_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    _schema: &TViewSchema,
) -> TViewResult<()> {
    // Get OIDs
    let view_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{view_name}'"
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for view {view_name}"),
        pg_error: "View not found".to_string(),
    })?;

    let table_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{tview_name}'"
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for table {tview_name}"),
        pg_error: "Table not found".to_string(),
    })?;

    // Insert metadata
    let definition = format!("SELECT * FROM {view_name}");
    Spi::run(&format!(
        "INSERT INTO pg_tview_meta (entity, view_oid, table_oid, definition, fk_columns, uuid_fk_columns)
         VALUES ('{}', {}, {}, '{}', '{{}}', '{{}}')
         ON CONFLICT (entity) DO UPDATE SET
            view_oid = EXCLUDED.view_oid,
            table_oid = EXCLUDED.table_oid,
            definition = EXCLUDED.definition,
            fk_columns = EXCLUDED.fk_columns,
            uuid_fk_columns = EXCLUDED.uuid_fk_columns",
        entity_name.replace('\'', "''"),
        view_oid.to_u32(),
        table_oid.to_u32(),
        definition.replace('\'', "''")
    ))?;

    Ok(())
}

#[derive(Debug)]
struct BackupRow {
    id: Option<String>,
    data: Option<String>,
}

fn extract_entity_name(table_name: &str) -> TViewResult<&str> {
    table_name.strip_prefix("tv_")
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: "Table name must start with tv_".to_string(),
        })
}