//! Event Trigger handler for DDL interception
//!
//! This module handles `PostgreSQL` Event Triggers that fire AFTER DDL commands
//! complete. This provides a safe context for SPI operations, unlike `ProcessUtility`
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
#[pg_extern(sql = r#"
CREATE OR REPLACE FUNCTION pg_tviews_handle_ddl_event() RETURNS event_trigger
AS 'MODULE_PATHNAME', 'pg_tviews_handle_ddl_event_wrapper'
LANGUAGE c;
"#)]
fn pg_tviews_handle_ddl_event() {
    info!("pg_tviews: Event trigger fired");

    // Get information about the DDL command that just executed
    let commands = match get_ddl_commands() {
        Ok(cmds) => cmds,
        Err(e) => {
            warning!("pg_tviews: Failed to get DDL commands: {}", e);
            return;
        }
    };

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
}

/// Get DDL commands from `pg_event_trigger_ddl_commands()`
fn get_ddl_commands() -> spi::Result<Vec<DdlCommand>> {
    Spi::connect(|client| {
        let query = "SELECT command_tag, object_identity
                     FROM pg_event_trigger_ddl_commands()";

        let results = client.select(query, None, &[])?;
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

/// Public API: Convert an existing table to a TVIEW
///
/// Called by the event trigger after `PostgreSQL` creates the table.
/// This runs in a safe SPI context (after DDL completed).
///
/// Strategy:
/// 1. Retrieve original SELECT from hook cache
/// 2. Drop the table `PostgreSQL` created
/// 3. Create proper TVIEW using standard `create_tview()` flow
#[pg_extern]
fn pg_tviews_convert_table(table_name: String) -> Result<(), Box<dyn std::error::Error>> {
    info!("pg_tviews_convert_table: Converting '{}' to TVIEW", table_name);

    // Retrieve the original SELECT from the hook cache
    let select_sql = crate::hooks::take_pending_tview_select(&table_name)
        .ok_or_else(|| {
            format!("No SELECT statement found for '{table_name}' - was the hook called?")
        })?;

    info!("pg_tviews_convert_table: Retrieved SELECT for '{}'", table_name);

    // Drop the table that PostgreSQL created
    // We need to create our own structure with proper TVIEW semantics
    Spi::run(&format!("DROP TABLE IF EXISTS {table_name} CASCADE"))
        .map_err(|e| format!("Failed to drop table '{table_name}': {e}"))?;

    info!("pg_tviews_convert_table: Dropped PostgreSQL's table '{}'", table_name);

    // Create proper TVIEW using the original SELECT
    // This has all the TVIEW semantics: backing view, materialized table, triggers, etc.
    crate::ddl::create_tview(&table_name, &select_sql)
        .map_err(|e| format!("Failed to create TVIEW '{table_name}': {e}"))?;

    info!("pg_tviews_convert_table: Successfully created TVIEW '{}'", table_name);

    Ok(())
}

/// Convert an existing table to a TVIEW
///
/// This function is called AFTER `PostgreSQL` has created the table.
/// Delegates to the `ddl::convert` module for the actual conversion logic.
fn convert_table_to_tview(table_name: &str) -> TViewResult<()> {
    info!("pg_tviews: convert_table_to_tview called for '{}'", table_name);
    crate::ddl::convert_existing_table_to_tview(table_name)
}

