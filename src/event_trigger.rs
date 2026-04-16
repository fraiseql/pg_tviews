//! Event Trigger handler for DDL interception
//!
//! This module provides the `pg_tviews_convert_table()` C function called by the
//! PL/pgSQL `pg_tviews_handle_ddl_event()` event trigger (defined in `metadata.rs`).
//!
//! ## Why PL/pgSQL for the event trigger handler?
//!
//! pgrx always generates `RETURNS VOID` for `#[pg_extern]` functions, but PostgreSQL
//! requires event trigger handlers to return the `event_trigger` pseudo-type.
//! The PL/pgSQL wrapper satisfies PostgreSQL's type requirement and calls this C
//! function for the actual conversion logic.

use pgrx::prelude::*;
use crate::utils::quote_identifier;

/// Convert a `tv_*` table (just created by `CREATE TABLE tv_* AS SELECT …`) to a TVIEW.
///
/// Called by the PL/pgSQL event trigger `pg_tviews_handle_ddl_event()` after PostgreSQL
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
    // Retrieve (and consume) the pending SELECT.  Empty cache = created by pg_tviews_create.
    let Some(select_sql) = crate::hooks::take_pending_tview_select(&table_name) else {
        return Ok(());
    };

    // Drop the regular table PostgreSQL created — we replace it with TVIEW semantics.
    let qi_table = quote_identifier(&table_name);
    Spi::run(&format!("DROP TABLE IF EXISTS {qi_table} CASCADE"))
        .map_err(|e| format!("Failed to drop table '{table_name}': {e}"))?;

    // Create the proper TVIEW: backing view, materialized table, triggers.
    crate::ddl::create_tview(&table_name, &select_sql)
        .map_err(|e| format!("Failed to create TVIEW '{table_name}': {e}"))?;

    Ok(())
}
