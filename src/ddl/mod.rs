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
use pgrx::prelude::*;

/// Schema and name of the backing view of the TVIEW whose table is
/// `schema.table`: `tviews.<schema>__<table>`, fitted to 63 bytes. The
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
/// TVIEW's table, give its backing view the name the table now derives.
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
        crate::utils::ident::quoted(&view_schema),
        crate::utils::ident::quoted(&wanted)
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
        crate::utils::ident::quoted(&wanted)
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
    let args = [
        crate::utils::spi::oid(role),
        crate::utils::spi::text(schema),
    ];
    let (can_create, name) = Spi::get_two_with_args::<bool, String>(
        "SELECT pg_catalog.has_schema_privilege($1, $2, 'CREATE'), \
                pg_catalog.quote_ident(pg_catalog.pg_get_userbyid($1))",
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
/// calls registering, changing or dropping one entity run one after the other:
/// `pg_advisory_xact_lock(<class>, hashtext(entity))`.
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
