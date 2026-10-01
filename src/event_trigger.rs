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
//! function, which only reports a `CREATE TABLE tv_* AS` the `ProcessUtility` hook did
//! not intercept: an intercepted one never creates a plain table.

use pgrx::prelude::*;

/// Called by the PL/pgSQL event trigger `pg_tviews_handle_ddl_event()` after
/// `PostgreSQL` created a `tv_*` table with `CREATE TABLE … AS` or `SELECT … INTO`.
///
/// The `ProcessUtility` hook turns such a statement into a TVIEW before PostgreSQL
/// creates anything (issue #134). A plain table reaching this point means the hook
/// never saw the statement (`pg_tviews` not in `shared_preload_libraries`): fail loudly
/// rather than leave a table that deploy tools would take for a TVIEW (issue #80).
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires String by value
fn pg_tviews_convert_table(table_name: String, command_tag: default!(Option<String>, "NULL")) {
    if matches!(
        command_tag.as_deref(),
        Some("CREATE TABLE AS" | "SELECT INTO")
    ) {
        pgrx::pg_sys::panic::ErrorReport::new(
            PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
            format!(
                "pg_tviews: cannot convert '{table_name}' to a TVIEW: the statement was \
                 not intercepted"
            ),
            function_name!(),
        )
        .set_detail("pg_tviews is not active in this session's ProcessUtility hook")
        .set_hint(
            "Add pg_tviews to shared_preload_libraries in postgresql.conf and restart \
             PostgreSQL, or create the TVIEW with tviews.pg_tviews_create_or_replace().",
        )
        .report(PgLogLevel::ERROR);
    }
}
