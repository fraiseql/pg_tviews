//! Event Trigger handler for DDL interception
//!
//! This module handles PostgreSQL Event Triggers that fire AFTER DDL commands
//! complete. This provides a safe context for SPI operations, unlike ProcessUtility
//! hooks which cannot safely use SPI.

use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

/// Event trigger function called after DDL command completes
///
/// This is registered as an event trigger in SQL:
/// ```sql
/// CREATE EVENT TRIGGER pg_tviews_ddl_end
/// ON ddl_command_end
/// WHEN TAG IN ('CREATE TABLE', 'SELECT INTO')
/// EXECUTE FUNCTION pg_tviews_handle_ddl_event();
/// ```
///
/// # Safety Context
/// Event triggers fire AFTER the DDL completes, providing a safe context
/// for SPI operations. The table already exists at this point.
#[pg_extern]
fn pg_tviews_handle_ddl_event() -> Result<(), Box<dyn std::error::Error>> {
    info!("pg_tviews: Event trigger fired");

    // Get information about the DDL command that just executed
    let commands = get_ddl_commands()?;

    for cmd in commands {
        // Only process CREATE TABLE and SELECT INTO
        if !matches!(cmd.command_tag.as_str(), "CREATE TABLE" | "SELECT INTO") {
            continue;
        }

        let table_name = cmd.object_identity;
        info!("pg_tviews: Checking table '{}'", table_name);

        // Check if this is a tv_* table
        if !table_name.starts_with("tv_") {
            info!("pg_tviews: Not a TVIEW table, ignoring");
            continue;
        }

        info!("pg_tviews: Converting '{}' to TVIEW", table_name);

        // Convert the newly-created table to a TVIEW
        match convert_table_to_tview(&table_name) {
            Ok(()) => {
                info!("pg_tviews: Successfully converted '{}' to TVIEW", table_name);
            }
            Err(e) => {
                // Log error but don't fail the transaction
                // The table was already created by PostgreSQL
                error!("pg_tviews: Failed to convert '{}' to TVIEW: {}", table_name, e);
                error!("pg_tviews: Table exists as regular table, not a TVIEW");
            }
        }
    }

    Ok(())
}

/// Get DDL commands from pg_event_trigger_ddl_commands()
fn get_ddl_commands() -> spi::Result<Vec<DdlCommand>> {
    Spi::connect(|client| {
        let query = "SELECT command_tag, object_type, object_identity
                     FROM pg_event_trigger_ddl_commands()";

        let results = client.select(query, None, None)?;
        let mut commands = Vec::new();

        for row in results {
            let command_tag: String = row["command_tag"].value()?.unwrap_or_default();
            let object_type: String = row["object_type"].value()?.unwrap_or_default();
            let object_identity: String = row["object_identity"].value()?.unwrap_or_default();

            commands.push(DdlCommand {
                command_tag,
                object_type,
                object_identity,
            });
        }

        Ok::<_, spi::Error>(commands)
    })
}

struct DdlCommand {
    command_tag: String,
    object_type: String,
    object_identity: String,
}

/// Convert an existing table to a TVIEW
///
/// This function is called AFTER PostgreSQL has created the table.
/// Strategy:
/// 1. Validate table structure (must have pk_*, id, data columns)
/// 2. Rename tv_entity → tv_entity_materialized (the backing table)
/// 3. Create view v_entity (user's original SELECT)
/// 4. Create tv_entity as a wrapper view
/// 5. Install triggers on base tables
/// 6. Register metadata
fn convert_table_to_tview(table_name: &str) -> TViewResult<()> {
    info!("pg_tviews: convert_table_to_tview called for '{}'", table_name);

    // Extract entity name
    let entity_name = table_name.strip_prefix("tv_")
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: "Table name must start with tv_".to_string(),
        })?;

    // Step 1: Validate table structure
    validate_tview_table_structure(table_name)?;

    // Step 2: Get the original SELECT from table definition
    // Since CREATE TABLE AS was used, we need to infer the SELECT
    // For now, we'll create a simple SELECT that reads from the materialized table
    let select_sql = infer_select_from_table(table_name)?;

    // Step 3: Rename the table to tv_entity_materialized
    let materialized_name = format!("{}_materialized", table_name);
    Spi::run(&format!("ALTER TABLE {} RENAME TO {}", table_name, materialized_name))?;
    info!("pg_tviews: Renamed {} → {}", table_name, materialized_name);

    // Step 4: Create view v_entity pointing to materialized table
    let view_name = format!("v_{}", entity_name);
    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM {}",
        view_name, materialized_name
    ))?;
    info!("pg_tviews: Created view {}", view_name);

    // Step 5: Create tv_entity as wrapper view
    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM {}",
        table_name, view_name
    ))?;
    info!("pg_tviews: Created wrapper view {}", table_name);

    // Step 6: Register metadata
    // TODO: Find dependencies, install triggers, etc.
    // For Phase 1, we just register basic metadata
    register_basic_metadata(entity_name, &view_name, table_name, &select_sql)?;

    info!("pg_tviews: TVIEW '{}' created successfully", table_name);
    Ok(())
}

/// Validate that the table has the required TVIEW structure
fn validate_tview_table_structure(table_name: &str) -> TViewResult<bool> {
    // Check for required columns: pk_*, id, data
    let entity_name = table_name.strip_prefix("tv_").unwrap();
    let pk_column = format!("pk_{}", entity_name);

    let has_pk = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = '{}'
        )",
        table_name, pk_column
    ))?.unwrap_or(false);

    let has_id = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = 'id'
        )",
        table_name
    ))?.unwrap_or(false);

    let has_data = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = 'data'
        )",
        table_name
    ))?.unwrap_or(false);

    if !has_pk || !has_id || !has_data {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!(
                "TVIEW table must have columns: {}, id, data. Found: pk={}, id={}, data={}",
                pk_column, has_pk, has_id, has_data
            ),
        });
    }

    Ok(true)
}

/// Infer the original SELECT statement from the table structure
fn infer_select_from_table(table_name: &str) -> TViewResult<String> {
    // For Phase 1, return a placeholder
    // In Phase 2, we'll improve this to reconstruct the original SELECT
    Ok(format!("SELECT * FROM {}", table_name))
}

/// Register basic metadata for the TVIEW
fn register_basic_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    definition: &str,
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
        "INSERT INTO pg_tview_meta (entity, view_oid, table_oid, definition)
         VALUES ('{}', {}, {}, '{}')
         ON CONFLICT (entity) DO UPDATE SET
            view_oid = EXCLUDED.view_oid,
            table_oid = EXCLUDED.table_oid,
            definition = EXCLUDED.definition",
        entity_name.replace("'", "''"),
        view_oid.as_u32(),
        table_oid.as_u32(),
        definition.replace("'", "''")
    ))?;

    Ok(())
}