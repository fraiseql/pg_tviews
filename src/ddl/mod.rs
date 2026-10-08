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
pub(crate) mod privileges;
pub mod rename;
pub mod replace;
pub(crate) mod uncascaded;

pub use drop::drop_tview;

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Schema and name of the backing view of the TVIEW whose table is
/// `schema.table`: `tviews.<schema>__<table>`, fitted to 63 bytes (#181). The
/// application's schema holds only its own objects, among them its `v_<entity>`
/// query view. This is the only place a backing view's name is built; everything
/// else finds it by OID (`pg_tview_meta.view_oid`).
pub(crate) fn backing_view_name(schema: &str, table: &str) -> (String, String) {
    (
        crate::utils::ext_schema().to_string(),
        crate::utils::fit_identifier(format!("{schema}__{table}")),
    )
}

/// After `ALTER TABLE … RENAME` or `SET SCHEMA` of relation `table`: if it is a
/// TVIEW's table, give its backing view the name the table now derives (#181).
///
/// # Errors
/// Returns an error if the catalog cannot be read, the name is taken, or the
/// rename fails.
pub(crate) fn follow_table_move(table: pg_sys::Oid) -> TViewResult<()> {
    let args = [crate::utils::spi::oid(table)];
    let view = Spi::get_one_with_args::<pg_sys::Oid>(
        &format!(
            "SELECT (SELECT m.view_oid::pg_catalog.oid FROM {} m \
                     WHERE m.table_oid::pg_catalog.oid = $1)",
            crate::utils::meta_table()
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: "Find the TVIEW of a moved table".to_string(),
        pg_error: e.to_string(),
    })?;
    let Some(view) = view else {
        return Ok(());
    };
    let (schema, name) = relation_name(table)?;
    let (view_schema, wanted) = backing_view_name(&schema, &name);
    let (_, current) = relation_name(view)?;
    if current == wanted {
        return Ok(());
    }
    let qualified = format!(
        "{}.{}",
        crate::utils::quote_identifier(&view_schema),
        crate::utils::quote_identifier(&wanted)
    );
    let taken = Spi::get_one_with_args::<bool>(
        "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
        &[crate::utils::spi::text(qualified.as_str())],
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check {view_schema}.{wanted}"),
        pg_error: e.to_string(),
    })?
    .unwrap_or(false);
    if taken {
        return Err(TViewError::InvalidInput {
            parameter: "table name".to_string(),
            reason: format!(
                "the backing view of {schema}.{name} would be {view_schema}.{wanted}, which is \
                 already taken by another relation"
            ),
        });
    }
    let sql = format!(
        "ALTER VIEW {} RENAME TO {}",
        crate::utils::qualified_relname_from_oid(view)?,
        crate::utils::quote_identifier(&wanted)
    );
    {
        let _owner = crate::owner::AsOwner::of_table(view)?;
        crate::utils::spi::run_ddl(&sql)?;
    }
    // Names are cached by OID: here at once, in other backends at commit.
    crate::cache::invalidate_all();
    // SAFETY: `table` is the TVIEW's existing table.
    unsafe { pg_sys::CacheInvalidateRelcacheByRelid(table) };
    Ok(())
}

/// Run `ddl`, which creates or replaces a backing view in the extension's
/// schema, as the current role: the TVIEW's owner owns its backing view, reads
/// the base tables with its own privileges, and may drop it. The role is granted
/// CREATE on the extension's schema for the statement when it lacks it, by the
/// extension's owner, and loses it right after; both are part of the current
/// transaction.
///
/// # Errors
/// Returns an error if the privilege cannot be checked, granted or revoked, or
/// if `ddl` fails.
pub(crate) fn in_extension_schema<T>(ddl: impl FnOnce() -> TViewResult<T>) -> TViewResult<T> {
    // SAFETY: reads the backend's current user id.
    in_extension_schema_for(unsafe { pg_sys::GetUserId() }, ddl)
}

/// Run `ddl` with `role` allowed to create in the extension's schema: granted
/// CREATE for the statement when it lacks it, as in [`in_extension_schema`].
/// `ALTER VIEW … OWNER TO` needs it of the new owner.
///
/// # Errors
/// Returns an error if the privilege cannot be checked, granted or revoked, or
/// if `ddl` fails.
pub(crate) fn in_extension_schema_for<T>(
    role: pg_sys::Oid,
    ddl: impl FnOnce() -> TViewResult<T>,
) -> TViewResult<T> {
    let schema = crate::utils::ext_schema();
    let args = [crate::utils::spi::oid(role)];
    let (can_create, name) = Spi::get_two_with_args::<bool, String>(
        &format!(
            "SELECT pg_catalog.has_schema_privilege($1, '{schema}', 'CREATE'), \
                    pg_catalog.quote_ident(pg_catalog.pg_get_userbyid($1))"
        ),
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check CREATE on schema {schema}"),
        pg_error: e.to_string(),
    })?;
    if can_create.unwrap_or(false) {
        return ddl();
    }
    let role = name.unwrap_or_default();
    let privilege = |verb: &str| -> TViewResult<()> {
        let sql = if verb == "GRANT" {
            format!("GRANT CREATE ON SCHEMA {schema} TO {role}")
        } else {
            format!("REVOKE CREATE ON SCHEMA {schema} FROM {role}")
        };
        let _owner = crate::owner::AsOwner::of_extension()?;
        crate::utils::spi::run_ddl(&sql)
    };
    privilege("GRANT")?;
    let result = ddl()?;
    privilege("REVOKE")?;
    Ok(result)
}

/// Schema and name of relation `oid`.
///
/// # Errors
/// Returns an error if the relation does not exist.
pub(crate) fn relation_name(oid: pg_sys::Oid) -> TViewResult<(String, String)> {
    let args = [crate::utils::spi::oid(oid)];
    let (name, schema) = Spi::get_two_with_args::<String, String>(
        "SELECT c.relname::text, n.nspname::text FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = $1",
        &args,
    )
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Name relation {oid:?}"),
        pg_error: e.to_string(),
    })?;
    match (schema, name) {
        (Some(schema), Some(name)) => Ok((schema, name)),
        _ => Err(TViewError::CatalogError {
            operation: format!("Name relation {oid:?}"),
            pg_error: "relation not found".to_string(),
        }),
    }
}

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
        &[
            crate::utils::spi::int4(REGISTRATION_LOCK_CLASS),
            crate::utils::spi::text(entity),
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
fn pg_tviews_create(tview_name: &str, select_sql: &str) -> Result<String, ErrorReport> {
    crate::revision::check();
    create_reported(tview_name, select_sql, replace::Options::default())
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
) -> Result<String, ErrorReport> {
    crate::revision::check();
    let keys: aggregate::GroupKeys = serde_json::from_value(group_keys.0)
        .ok()
        .filter(|keys: &aggregate::GroupKeys| !keys.is_empty())
        .ok_or_else(|| TViewError::InvalidInput {
            parameter: "group_keys".to_string(),
            reason: "a JSON object mapping source table names to column names is expected, \
                     e.g. '{\"tb_order\": \"fk_user\"}'"
                .to_string(),
        })?;
    create_reported(tview_name, select_sql, replace::Options::aggregate(keys))
}

/// `pg_tviews_create[_aggregate]()`: create-only, reported as text.
fn create_reported(
    tview_name: &str,
    select_sql: &str,
    options: replace::Options,
) -> Result<String, ErrorReport> {
    // A session that loaded the library lazily gets the ProcessUtility hook now.
    // SAFETY: called from a backend function, where installing the hook is valid.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }
    match replace::create_only(tview_name, select_sql, options, false) {
        Ok(replace::Created::Rows(_) | replace::Created::Skipped) => {
            Ok(format!("TVIEW '{tview_name}' created successfully"))
        }
        Ok(replace::Created::Exists(name)) => Err(TViewError::RelationExists { name }.into()),
        Err(e) => Err(e.report_in("Failed to create TVIEW")),
    }
}

/// Internal: called by the `sql_drop` event trigger for a TVIEW whose backing view
/// or table was dropped as a dependent (see [`drop::handle_dropped`]).
#[pg_extern]
fn pg_tviews_handle_dropped(entity: &str) -> Result<(), ErrorReport> {
    drop::handle_dropped(entity)
        .map_err(|e| e.report_in(&format!("Failed to deregister TVIEW '{entity}'")))
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
) -> Result<String, ErrorReport> {
    crate::revision::check();
    replace::create_or_replace(tview_name, query, &options.0)
        .map(str::to_string)
        .map_err(|e| e.report_in(&format!("Failed to create or replace TVIEW '{tview_name}'")))
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
) -> Result<String, ErrorReport> {
    crate::revision::check();
    match drop_tview(tview_name, if_exists, cascade) {
        Ok(true) => Ok(format!("TVIEW '{tview_name}' dropped successfully")),
        Ok(false) => Ok(format!(
            "TVIEW '{tview_name}' does not exist, nothing dropped"
        )),
        Err(e) => Err(e.report_in("Failed to drop TVIEW")),
    }
}

/// SQL function: re-derive a TVIEW's metadata and base-table triggers from its
/// stored definition with this release's analysis, and clear `needs_reregister`
/// (issue #137). The TVIEW's rows are not touched. Requires owning the TVIEW or
/// the extension.
///
/// Usage: `SELECT tviews.pg_tviews_reregister('post');`
#[pg_extern]
fn pg_tviews_reregister(tview_name: &str) -> Result<String, ErrorReport> {
    crate::revision::check();
    crate::validation::validate_sql_identifier(tview_name, "tview_name")?;
    let entity = tview_name.strip_prefix("tv_").unwrap_or(tview_name);
    create::reregister_tview(entity)
        .map(|()| "reregistered".to_string())
        .map_err(|e| e.report_in(&format!("Failed to re-register TVIEW '{entity}'")))
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
) -> Result<Vec<String>, ErrorReport> {
    crate::revision::check();
    create::rebind_cascade_paths(view_oid, &cascade_paths)
        .map_err(|e| e.report_in("Failed to rebind cascade paths"))
}

/// SQL function: deprecated, always raises an error.
///
/// It replaced `tv_x` with a view over a literal `VALUES` snapshot (no triggers,
/// no refresh), and could not run on PG18. Use [`pg_tviews_create`] or
/// `CREATE TABLE tv_x AS SELECT ...` instead. The function is kept only so that
/// callers get this message; it is removed in the next breaking release.
#[pg_extern]
fn pg_tviews_convert_existing_table(table_name: &str) -> Result<String, ErrorReport> {
    crate::validation::validate_sql_identifier(table_name, "table_name")?;

    Err(TViewError::DefinitionRefused {
        reason: format!(
            "pg_tviews_convert_existing_table() is deprecated and no longer converts \
             '{table_name}'; use pg_tviews_create() or CREATE TABLE tv_<entity> AS SELECT ... \
             instead"
        ),
    }
    .into())
}
