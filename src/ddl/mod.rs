//! DDL Operations: TVIEW Creation and Management
//!
//! This module handles Data Definition Language operations for TVIEWs:
//! - **CREATE TABLE tv_ AS SELECT**: Parses SQL, creates metadata, sets up triggers
//! - **DROP TABLE tv_***: Cleans up metadata, removes triggers and views
//! - **Validation**: Ensures TVIEW names and SQL are valid
//!
//! ## Architecture
//!
//! DDL operations follow this sequence:
//! 1. Parse and validate TVIEW name (`tv_*` format)
//! 2. Analyze SELECT statement for column types and dependencies
//! 3. Create metadata entries in `pg_tview_meta`
//! 4. Set up triggers on base tables for change tracking
//! 5. Create the actual view with refresh triggers

pub mod aggregate;
pub mod create;
pub mod drop;
pub mod rename;
pub mod replace;

pub use drop::drop_tview;

use crate::error::{TViewError, TViewResult};
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Advisory lock class of TVIEW registrations (`"tvie"`).
const REGISTRATION_LOCK_CLASS: i32 = 0x7476_6965;

/// Hold the registration lock of `entity` until the transaction ends, so that
/// calls registering, changing or dropping one entity run one after the other
/// (issue #134): `pg_advisory_xact_lock(<class>, hashtext(entity))`.
///
/// # Errors
/// Returns an error if the lock cannot be taken.
pub(crate) fn lock_entity(entity: &str) -> TViewResult<()> {
    Spi::run_with_args(
        "SELECT pg_catalog.pg_advisory_xact_lock($1, pg_catalog.hashtext($2))",
        // SAFETY: the datums copy the class and borrow `entity`, which outlives the call.
        &[
            unsafe {
                DatumWithOid::new(
                    REGISTRATION_LOCK_CLASS,
                    PgOid::BuiltIn(PgBuiltInOids::INT4OID).value(),
                )
            },
            unsafe { DatumWithOid::new(entity, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Lock the registration of TVIEW {entity}"),
        pg_error: e.to_string(),
    })
}

/// SQL function: create a TVIEW. An existing one is an error; use
/// [`pg_tviews_create_or_replace`] to change it.
///
/// Usage: `SELECT tviews.pg_tviews_create('tv_post', 'SELECT pk_post, id, … AS data FROM tb_post');`
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
    crate::revision::check();
    create_only(tview_name, select_sql, replace::Options::default())
}

/// SQL function: create an aggregate TVIEW (issue #58).
///
/// Usage:
/// `SELECT pg_tviews_create_aggregate('tv_user_summary', $$ SELECT o.fk_user AS
///  pk_user_summary, u.id, jsonb_build_object('orders', count(*)) AS data FROM tb_order o
///  JOIN tb_user u ON u.pk_user = o.fk_user GROUP BY o.fk_user, u.id $$,
///  '{"tb_order": "fk_user", "tb_user": "pk_user"}');`
///
/// `group_keys` maps each source table to the column whose value is the group key.
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires JsonB by value
fn pg_tviews_create_aggregate(
    tview_name: &str,
    select_sql: &str,
    group_keys: pgrx::JsonB,
) -> Result<String, String> {
    crate::revision::check();
    let keys: aggregate::GroupKeys = serde_json::from_value(group_keys.0)
        .ok()
        .filter(|keys: &aggregate::GroupKeys| !keys.is_empty())
        .ok_or_else(|| {
            "group_keys must be a JSON object mapping source table names to column names, e.g. \
             '{\"tb_order\": \"fk_user\"}'"
                .to_string()
        })?;
    create_only(tview_name, select_sql, replace::Options::aggregate(keys))
}

/// `pg_tviews_create[_aggregate]()`: create-only, reported as text.
fn create_only(
    tview_name: &str,
    select_sql: &str,
    options: replace::Options,
) -> Result<String, String> {
    // A session that loaded the library lazily gets the ProcessUtility hook now.
    // SAFETY: called from a backend function, where installing the hook is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }
    match replace::create_only(tview_name, select_sql, options, false) {
        Ok(replace::Created::Rows(_) | replace::Created::Skipped) => {
            Ok(format!("TVIEW '{tview_name}' created successfully"))
        }
        Ok(replace::Created::Exists(name)) => Err(format!(
            "TVIEW {name} already exists; pg_tviews_create_or_replace() changes an existing TVIEW"
        )),
        Err(e) => Err(format!("Failed to create TVIEW: {e}")),
    }
}

/// Internal: called by the `sql_drop` event trigger for a TVIEW whose backing view
/// or table was dropped as a dependent (see [`drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), String> {
    drop::handle_dropped(entity).map_err(|e| format!("Failed to deregister TVIEW '{entity}': {e}"))
}

/// SQL function: create a TVIEW, or bring an existing one to `query` and
/// `options` with the smallest change (issue #134). Returns `created`,
/// `unchanged`, `altered` or `rebuilt`.
///
/// Usage: `SELECT tviews.pg_tviews_create_or_replace('app.tv_post', $$SELECT …$$,
/// options => '{"logged": true, "fillfactor": 85}');`
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires JsonB by value
fn pg_tviews_create_or_replace(
    tview_name: &str,
    query: &str,
    options: default!(pgrx::JsonB, "'{}'"),
) -> Result<String, String> {
    crate::revision::check();
    replace::create_or_replace(tview_name, query, &options.0)
        .map(str::to_string)
        .map_err(|e| format!("Failed to create or replace TVIEW '{tview_name}': {e}"))
}

/// SQL function: Drop a TVIEW
///
/// Usage: SELECT `pg_tviews_drop`('`my_entity`', true);        -- true = IF EXISTS
///        SELECT `pg_tviews_drop`('`my_entity`', true, true);  -- IF EXISTS + CASCADE
///        SELECT `pg_tviews_drop`('`app.tv_post`');            -- schema-qualified
#[pg_extern]
fn pg_tviews_drop(
    tview_name: &str,
    if_exists: default!(bool, false),
    cascade: default!(bool, false),
) -> Result<String, String> {
    crate::revision::check();
    match drop_tview(tview_name, if_exists, cascade) {
        Ok(()) => Ok(format!("TVIEW '{tview_name}' dropped successfully")),
        Err(e) => Err(format!("Failed to drop TVIEW: {e}")),
    }
}

/// SQL function: re-derive a TVIEW's metadata and base-table triggers from its
/// stored definition with this release's analysis, and clear `needs_reregister`
/// (issue #137). The TVIEW's rows are not touched. Requires owning the TVIEW or
/// the extension.
///
/// Usage: `SELECT tviews.pg_tviews_reregister('post');`
#[pg_extern]
fn pg_tviews_reregister(tview_name: &str) -> Result<String, String> {
    crate::revision::check();
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;
    let entity = tview_name.strip_prefix("tv_").unwrap_or(tview_name);
    create::reregister_tview(entity)
        .map(|()| "reregistered".to_string())
        .map_err(|e| format!("Failed to re-register TVIEW '{entity}': {e}"))
}

// Every TVIEW, dependencies first: an entity comes after every TVIEW its backing
// view reads, through views. Each runs in its own subtransaction, so a failure
// becomes that entity's status and the others go on; `strict` raises at the end.
extension_sql!(
    r"
CREATE FUNCTION @extschema@.pg_tviews_reregister_all(strict BOOLEAN DEFAULT false)
RETURNS TABLE (entity TEXT, status TEXT)
LANGUAGE plpgsql
AS $$
#variable_conflict use_column
DECLARE
    next_entity TEXT;
    failures INTEGER := 0;
BEGIN
    FOR next_entity IN
        WITH RECURSIVE edges(entity, dependency) AS (
            SELECT DISTINCT r.entity, m.entity
            FROM @extschema@.pg_tview_reads r
            JOIN @extschema@.pg_tview_meta m
              ON r.relid IN (m.view_oid::oid, m.table_oid::oid)
            WHERE m.entity <> r.entity
        ),
        depth(entity, level) AS (
            SELECT m.entity, 0 FROM @extschema@.pg_tview_meta m
          UNION
            SELECT e.entity, d.level + 1
            FROM depth d JOIN edges e ON e.dependency = d.entity
            WHERE d.level < 100
        )
        SELECT d.entity FROM depth d GROUP BY d.entity ORDER BY max(d.level), d.entity
    LOOP
        entity := next_entity;
        BEGIN
            PERFORM @extschema@.pg_tviews_reregister(next_entity);
            status := 'reregistered';
        EXCEPTION WHEN OTHERS THEN
            status := SQLERRM;
            failures := failures + 1;
        END;
        RETURN NEXT;
    END LOOP;
    IF strict AND failures > 0 THEN
        RAISE EXCEPTION 'pg_tviews: % TVIEW(s) could not be re-registered', failures
            USING HINT = 'SELECT * FROM tviews.pg_tviews_reregister_all() lists them';
    END IF;
END;
$$;
    ",
    name = "reregister_all",
    requires = [
        pg_tviews_reregister,
        "create_metadata_tables",
        "tview_reads"
    ],
);

/// SQL function: rebind the relation OIDs inside `cascade_paths` to the current
/// catalog. Called by the `pg_tview_meta` insert trigger so that rows loaded by
/// `pg_restore` point at the restored relations; not meant to be called directly.
#[pg_extern]
#[allow(clippy::needless_pass_by_value)] // Reason: pgrx #[pg_extern] requires Vec by value
fn pg_tviews_rebind_cascade_paths(
    view_oid: pg_sys::Oid,
    cascade_paths: Vec<String>,
) -> Result<Vec<String>, String> {
    crate::revision::check();
    create::rebind_cascade_paths(view_oid, &cascade_paths)
        .map_err(|e| format!("Failed to rebind cascade paths: {e}"))
}

/// SQL function: deprecated, always raises an error.
///
/// It replaced `tv_x` with a view over a literal `VALUES` snapshot (no triggers,
/// no refresh), and could not run on PG18. Use [`pg_tviews_create`] or
/// `CREATE TABLE tv_x AS SELECT ...` instead. The function is kept only so that
/// callers get this message; it is removed in the next breaking release.
#[pg_extern]
fn pg_tviews_convert_existing_table(table_name: &str) -> Result<String, String> {
    crate::validation::validate_sql_identifier(table_name, "table_name")
        .map_err(|e| format!("Invalid table name: {e}"))?;

    Err(format!(
        "pg_tviews_convert_existing_table() is deprecated and no longer converts '{table_name}'; \
         use pg_tviews_create() or CREATE TABLE tv_<entity> AS SELECT ... instead"
    ))
}
