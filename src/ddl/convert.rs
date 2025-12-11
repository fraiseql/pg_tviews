//! Convert existing tables to TVIEWs
//!
//! This module handles converting a table that was created by standard
//! PostgreSQL DDL into a proper TVIEW structure.

use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};
use crate::schema::TViewSchema;

/// Convert an existing table to a TVIEW
///
/// # Strategy
///
/// PostgreSQL has already created tv_entity as a regular table.
/// We need to:
/// 1. Validate it has TVIEW structure (pk_*, id, data columns)
/// 2. Extract the data
/// 3. Create backing view v_entity (reconstructed SELECT)
/// 4. Recreate tv_entity as a view that reads from v_entity
/// 5. Install triggers on base tables
/// 6. Populate metadata
///
/// # Challenges
///
/// - We don't have the original SELECT statement
/// - Must infer base tables from data
/// - Must handle edge cases (empty tables, complex JOINs)
pub fn convert_existing_table_to_tview(
    table_name: &str,
) -> TViewResult<()> {
    let entity_name = extract_entity_name(table_name)?;

    info!("Converting existing table '{}' to TVIEW", table_name);

    // Step 1: Validate structure
    validate_tview_structure(table_name, entity_name)?;

    // Step 2: Infer schema from table
    let schema = infer_schema_from_table(table_name)?;

    // Step 3: Extract existing data (will be restored later)
    let data_backup = backup_table_data(table_name, &schema)?;

    // Step 4: Get base tables (infer from data or require user hint)
    // For Phase 2, we'll require the base table to be specified
    // In Phase 3, we can add smarter inference
    let base_tables = infer_base_tables(table_name)?;

    // Step 5: Drop the existing table
    Spi::run(&format!("DROP TABLE {} CASCADE", table_name))?;
    info!("Dropped existing table '{}'", table_name);

    // Step 6: Reconstruct as proper TVIEW
    reconstruct_as_tview(
        table_name,
        entity_name,
        &schema,
        &base_tables,
        &data_backup,
    )?;

    info!("Successfully converted '{}' to TVIEW", table_name);
    Ok(())
}

/// Validate that table has required TVIEW structure
fn validate_tview_structure(table_name: &str, entity_name: &str) -> TViewResult<()> {
    let pk_col = format!("pk_{}", entity_name);

    // Check required columns exist
    let columns = get_table_columns(table_name)?;

    let has_pk = columns.iter().any(|c| c.name == pk_col);
    let has_id = columns.iter().any(|c| c.name == "id");
    let has_data = columns.iter().any(|c| c.name == "data");

    if !has_pk || !has_id || !has_data {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!(
                "Table must have TVIEW structure: {}, id, data. Found: {}",
                pk_col,
                columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>().join(", ")
            ),
        });
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
            table_name.replace("'", "''")
        );

        let results = client.select(&query, None, None)?;

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
    // For Phase 2, we'll implement a simple backup
    // In practice, we'd need to handle the actual data structure
    let mut backup = Vec::new();

    Spi::connect(|client| {
        let query = format!("SELECT * FROM {}", table_name);
        let results = client.select(&query, None, None)?;

        for row in results {
            // For now, just store a placeholder
            // In practice, we'd extract the actual row data
            backup.push(BackupRow {
                id: row["id"].value()?,
                data: row["data"].value()?,
            });
        }

        Ok::<_, spi::Error>(())
    })?;

    Ok(backup)
}

fn infer_base_tables(_table_name: &str) -> TViewResult<Vec<String>> {
    // For Phase 2, return empty (no triggers installed)
    // In Phase 3, we can add logic to infer base tables from data
    Ok(Vec::new())
}

fn reconstruct_as_tview(
    table_name: &str,
    entity_name: &str,
    schema: &TViewSchema,
    _base_tables: &[String],
    data_backup: &[BackupRow],
) -> TViewResult<()> {
    // Step 1: Create the backing view
    let view_name = format!("v_{}", entity_name);

    // For Phase 2, create a simple view that unions the backed up data
    // In practice, this would reconstruct the original SELECT
    let mut values = Vec::new();
    for row in data_backup {
        if let (Some(id), Some(data)) = (&row.id, &row.data) {
            values.push(format!("('{}'::uuid, '{}')", id, data));
        }
    }

    let values_clause = if values.is_empty() {
        "SELECT NULL::uuid as id, NULL::jsonb as data WHERE false".to_string()
    } else {
        format!("VALUES {}", values.join(", "))
    };

    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM ({}) AS t(id, data)",
        view_name, values_clause
    ))?;
    info!("Created view {}", view_name);

    // Step 2: Create the TVIEW wrapper
    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM {}",
        table_name, view_name
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
    schema: &TViewSchema,
) -> TViewResult<()> {
    // Get OIDs
    let view_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}'",
        view_name
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for view {}", view_name),
        pg_error: "View not found".to_string(),
    })?;

    let table_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}'",
        tview_name
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for table {}", tview_name),
        pg_error: "Table not found".to_string(),
    })?;

    // Insert metadata
    Spi::run(&format!(
        "INSERT INTO pg_tview_meta (entity, view_oid, table_oid, definition, fk_columns, uuid_fk_columns)
         VALUES ('{}', {}, {}, '{}', '{}', '{}')
         ON CONFLICT (entity) DO UPDATE SET
            view_oid = EXCLUDED.view_oid,
            table_oid = EXCLUDED.table_oid,
            definition = EXCLUDED.definition,
            fk_columns = EXCLUDED.fk_columns,
            uuid_fk_columns = EXCLUDED.uuid_fk_columns",
        entity_name.replace("'", "''"),
        view_oid.as_u32(),
        table_oid.as_u32(),
        format!("SELECT * FROM {}", view_name), // Placeholder definition
        "{}", // Empty fk_columns for now
        "{}", // Empty uuid_fk_columns for now
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