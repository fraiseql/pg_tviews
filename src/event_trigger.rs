//! Event Trigger handler for DDL interception
//!
//! This module handles PostgreSQL Event Triggers that fire AFTER DDL commands
//! complete. This provides a safe context for SPI operations, unlike ProcessUtility
//! hooks which cannot safely use SPI.

use pgrx::prelude::*;
use crate::error::TViewResult;

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
            }
        }
    }

    Ok(())
}

/// Get DDL commands from pg_event_trigger_ddl_commands()
fn get_ddl_commands() -> spi::Result<Vec<DdlCommand>> {
    Spi::connect(|client| {
        let query = "SELECT command_tag, object_identity
                     FROM pg_event_trigger_ddl_commands()";

        let results = client.select(query, None, None)?;
        let mut commands = Vec::new();

        for row in results {
            let command_tag: String = row["command_tag"].value()?.unwrap_or_default();
            let object_identity: String = row["object_identity"].value()?.unwrap_or_default();

            commands.push(DdlCommand {
                command_tag,
                object_identity,
            });
        }

        Ok::<_, spi::Error>(commands)
    })
}

struct DdlCommand {
    command_tag: String,
    object_identity: String,
}

/// Convert an existing table to a TVIEW
///
/// This function is called AFTER PostgreSQL has created the table.
/// Delegates to the ddl::convert module for the actual conversion logic.
fn convert_table_to_tview(table_name: &str) -> TViewResult<()> {
    info!("pg_tviews: convert_table_to_tview called for '{}'", table_name);
    crate::ddl::convert_existing_table_to_tview(table_name)
}

