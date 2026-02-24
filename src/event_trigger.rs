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

    // Get information about the DDL command that just executed
    let commands = match get_ddl_commands() {
        Ok(cmds) => cmds,
        Err(e) => {
            warning!("pg_tviews: Failed to get DDL commands: {}", e);
            return;
        }
    };

    for cmd in commands {
        // Only process CREATE TABLE, CREATE TABLE AS, and SELECT INTO
        if !matches!(cmd.command_tag.as_str(), "CREATE TABLE" | "CREATE TABLE AS" | "SELECT INTO") {
            continue;
        }

        let table_name = cmd.object_identity;

        // object_identity is schema-qualified: "public.tv_post" or "myschema.tv_post"
        // Strip the schema prefix to get the bare table name for the tv_ check.
        let bare_name = table_name.split('.').next_back().unwrap_or(&table_name);

        // Check if this is a tv_* table
        if !bare_name.starts_with("tv_") {
            continue;
        }


        // Convert the newly-created table to a TVIEW (use bare_name: the hook
        // cache stores entries under the unqualified table name)
        match convert_table_to_tview(bare_name) {
            Ok(()) => {
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
#[allow(clippy::needless_pass_by_value)]
fn pg_tviews_convert_table(table_name: String) -> Result<(), Box<dyn std::error::Error>> {

    // Retrieve the original SELECT from the hook cache
    let select_sql = crate::hooks::take_pending_tview_select(&table_name)
        .ok_or_else(|| {
            format!("No SELECT statement found for '{table_name}' - was the hook called?")
        })?;


    // Drop the table that PostgreSQL created
    // We need to create our own structure with proper TVIEW semantics
    Spi::run(&format!("DROP TABLE IF EXISTS {table_name} CASCADE"))
        .map_err(|e| format!("Failed to drop table '{table_name}': {e}"))?;


    // Create proper TVIEW using the original SELECT
    // This has all the TVIEW semantics: backing view, materialized table, triggers, etc.
    crate::ddl::create_tview(&table_name, &select_sql)
        .map_err(|e| format!("Failed to create TVIEW '{table_name}': {e}"))?;


    Ok(())
}

/// Convert a table created by `CREATE TABLE tv_* AS SELECT` to a proper TVIEW.
///
/// Called by the event trigger after PostgreSQL creates the table.
///
/// There are two code paths that produce `tv_*` tables:
///   1. `CREATE TABLE tv_post AS SELECT …` — the ProcessUtility hook stores the
///      SELECT in the pending cache; this event trigger converts it using the
///      original SELECT statement.
///   2. `pg_tviews_create('post', '…')` — the function creates `tv_post` itself
///      via `spi_run_ddl`; the event trigger fires but must NOT convert again.
///
/// Guard: if no pending SELECT is in the cache, the table was created by
/// `pg_tviews_create` — skip silently.
fn convert_table_to_tview(table_name: &str) -> TViewResult<()> {

    // Retrieve (and consume) the pending SELECT.  If none exists, the table
    // was created by pg_tviews_create — nothing to do here.
    let select_sql = match crate::hooks::take_pending_tview_select(table_name) {
        Some(sql) => sql,
        None => {
            return Ok(());
        }
    };


    // Drop the regular table PostgreSQL just created.  DDL needs non-atomic SPI.
    crate::utils::spi_run_ddl(&format!("DROP TABLE IF EXISTS {table_name} CASCADE"))
        .map_err(|e| crate::TViewError::SpiError {
            query: format!("DROP TABLE IF EXISTS {table_name} CASCADE"),
            error: e,
        })?;


    // Create the proper TVIEW using the original SELECT.
    crate::ddl::create_tview(table_name, &select_sql)
}

