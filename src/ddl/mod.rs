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

pub use create::create_tview;
pub use drop::drop_tview;

use pgrx::prelude::*;

/// SQL function: Create a TVIEW
///
/// Usage: SELECT `pg_tviews_create`('`my_entity`', 'SELECT id, name FROM users WHERE active = true');
#[pg_extern]
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, String> {
    crate::revision::check();
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;

    // Ensure ProcessUtility hook is installed for DDL syntax support.
    // SAFETY: Called from PostgreSQL backend context, hook installation is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    match create_tview(tview_name, select_sql, None, false) {
        Ok(()) => Ok(format!("TVIEW '{tview_name}' created successfully")),
        Err(e) => Err(format!("Failed to create TVIEW: {e}")),
    }
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
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;
    let keys: aggregate::GroupKeys = serde_json::from_value(group_keys.0).map_err(|_| {
        "group_keys must be a JSON object mapping source table names to column names".to_string()
    })?;
    // SAFETY: Called from PostgreSQL backend context, hook installation is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }
    create::create_aggregate_tview(tview_name, select_sql, &keys)
        .map(|()| format!("TVIEW '{tview_name}' created successfully"))
        .map_err(|e| format!("Failed to create aggregate TVIEW: {e}"))
}

/// Internal: called by the `sql_drop` event trigger for a TVIEW whose backing view
/// or table was dropped as a dependent (see [`drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), String> {
    drop::handle_dropped(entity).map_err(|e| format!("Failed to deregister TVIEW '{entity}': {e}"))
}

/// SQL function: Drop a TVIEW
///
/// Usage: SELECT `pg_tviews_drop`('`my_entity`', true);        -- true = IF EXISTS
///        SELECT `pg_tviews_drop`('`my_entity`', true, true);  -- IF EXISTS + CASCADE
#[pg_extern]
fn pg_tviews_drop(
    tview_name: &str,
    if_exists: default!(bool, false),
    cascade: default!(bool, false),
) -> Result<String, String> {
    crate::revision::check();
    crate::validation::validate_sql_identifier(tview_name, "tview_name")
        .map_err(|e| format!("Invalid TVIEW name: {e}"))?;

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
        WITH RECURSIVE reads(entity, relid) AS (
            SELECT m.entity, m.view_oid::oid FROM @extschema@.pg_tview_meta m
          UNION
            SELECT r.entity, d.refobjid
            FROM reads r
            JOIN pg_catalog.pg_class v ON v.oid = r.relid AND v.relkind = 'v'
            JOIN pg_catalog.pg_rewrite w ON w.ev_class = v.oid
            JOIN pg_catalog.pg_depend d
              ON d.classid = 'pg_catalog.pg_rewrite'::pg_catalog.regclass
             AND d.objid = w.oid
             AND d.refclassid = 'pg_catalog.pg_class'::pg_catalog.regclass
             AND d.refobjid <> v.oid
        ),
        edges(entity, dependency) AS (
            SELECT DISTINCT r.entity, m.entity
            FROM reads r
            JOIN @extschema@.pg_tview_meta m
              ON r.relid IN (m.view_oid::oid, m.table_oid::oid)
            WHERE m.entity <> r.entity
        ),
        depth(entity, level) AS (
            SELECT m.entity, 0 FROM @extschema@.pg_tview_meta m
          UNION ALL
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
    requires = [pg_tviews_reregister, "create_metadata_tables"],
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
