//! Event Trigger handler for DDL interception
//!
//! This module provides the `pg_tviews_convert_table()` C function called by the
//! PL/pgSQL `pg_tviews_handle_ddl_event()` event trigger (defined in `metadata.rs`).
//!
//! ## Why PL/pgSQL for the event trigger handler?
//!
//! pgrx always generates `RETURNS VOID` for `#[pg_extern]` functions, but `PostgreSQL`
//! requires event trigger handlers to return the `event_trigger` pseudo-type.
//! The PL/pgSQL wrapper satisfies `PostgreSQL`'s type requirement and calls this C
//! function for the actual conversion logic.

use crate::utils::quote_identifier;
use pgrx::prelude::*;

/// Convert a `tv_*` table (just created by `CREATE TABLE tv_* AS SELECT …`) to a TVIEW.
///
/// Called by the PL/pgSQL event trigger `pg_tviews_handle_ddl_event()` after `PostgreSQL`
/// creates the table.  Runs in a safe SPI context (DDL already completed).
///
/// ## Two code paths that produce `tv_*` tables
///
/// 1. `CREATE TABLE tv_post AS SELECT …` — the `ProcessUtility` hook stores the
///    SELECT in the pending cache; this function reads the cache and converts.
/// 2. `pg_tviews_create('post', '…')` — creates `tv_post` itself via `spi_run_ddl`;
///    the event trigger fires but the cache is empty → skip silently.
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires String by value
fn pg_tviews_convert_table(table_name: String) -> Result<(), Box<dyn std::error::Error>> {
    // Log event trigger entry
    notice!(
        "===== EVENT TRIGGER: pg_tviews_convert_table START for table '{}' =====",
        table_name
    );

    // Retrieve (and consume) the pending (schema, SELECT) pair.
    // Empty cache = table was created by pg_tviews_create(), not DDL interception.
    let Some((schema_name, select_sql)) = crate::hooks::take_pending_tview_select(&table_name)
    else {
        notice!("DEBUG:   No pending SELECT found (likely created by pg_tviews_create, not CTAS)");
        return Ok(());
    };

    notice!("DEBUG:   Found cached SELECT ({} chars)", select_sql.len());
    notice!(
        "DEBUG:   Schema: '{}'",
        if schema_name.is_empty() {
            "(empty - will use current_schema())"
        } else {
            &schema_name
        }
    );

    // Resolve the target schema:
    // - Non-empty schema_name → the user wrote `CREATE TABLE schema.tv_* AS SELECT …`
    // - Empty schema_name → schema was omitted; defer to current_schema() inside create_tview
    let schema_override: Option<&str> = if schema_name.is_empty() {
        None
    } else {
        Some(schema_name.as_str())
    };

    // Drop the regular table PostgreSQL just created — we replace it with TVIEW semantics.
    // Use the resolved schema when available to avoid search_path ambiguity.
    let drop_sql = match schema_override {
        Some(s) => format!(
            "DROP TABLE IF EXISTS {}.{} CASCADE",
            quote_identifier(s),
            quote_identifier(&table_name),
        ),
        None => format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            quote_identifier(&table_name)
        ),
    };

    notice!("DEBUG:   Dropping existing table: {}", drop_sql);
    Spi::run(&drop_sql).map_err(|e| format!("Failed to drop table '{table_name}': {e}"))?;

    // Create the proper TVIEW: backing view, materialized table, triggers.
    notice!("DEBUG:   Calling create_tview()...");
    match crate::ddl::create_tview(&table_name, &select_sql, schema_override, true) {
        Ok(()) => {
            notice!("DEBUG: ✅ create_tview() SUCCEEDED for '{}'", table_name);
            notice!("DEBUG: ===== EVENT TRIGGER: COMPLETE =====");
            Ok(())
        }
        Err(e) => {
            notice!("DEBUG: ❌ create_tview() FAILED for '{}'", table_name);
            notice!("DEBUG:   Error: {:#?}", e);
            notice!("DEBUG: ===== EVENT TRIGGER: FAILED =====");
            Err(format!("Failed to create TVIEW '{table_name}': {e}").into())
        }
    }
}
