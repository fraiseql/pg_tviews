//! A backing view's privileges follow its TVIEW's table.
//!
//! A backing view lives in the extension's schema, where a grant on the
//! application's schema (`GRANT SELECT ON ALL TABLES IN SCHEMA app`, default
//! privileges on `app`) never reaches it. Whoever can `SELECT` from a TVIEW's
//! table can therefore `SELECT` from its backing view: the view's `SELECT`
//! grants are made those of the table when the view is created or rebuilt, and
//! again after every `GRANT` or `REVOKE` on a table. `ALTER TABLE tv_* OWNER TO`
//! gives the view the new owner. Only `SELECT` is copied: the view reads the base
//! tables with its owner's privileges, and a write through it would too.

use crate::error::{TViewError, TViewResult};
use pgrx::prelude::*;

/// For each TVIEW (the one whose table is `$1`, every one when `$1` is NULL), the
/// statements that give its backing view the `SELECT` grants of its table, as
/// `(view, statement)`. The view's owner is left out: it has every privilege.
const SELECT_GRANT_CHANGES: &str = "\
    WITH tview AS ( \
        SELECT m.table_oid::pg_catalog.oid AS tab, v.oid AS view, v.relowner AS owner, \
               pg_catalog.format('%I.%I', n.nspname, v.relname) AS name \
        FROM {meta} m JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid \
        JOIN pg_catalog.pg_namespace n ON n.oid = v.relnamespace \
        WHERE $1 IS NULL OR m.table_oid::pg_catalog.oid = $1 \
    ), wanted AS ( \
        SELECT t.view, t.name, a.grantee FROM tview t JOIN pg_catalog.pg_class c ON c.oid = t.tab, \
               pg_catalog.aclexplode(c.relacl) a \
        WHERE a.privilege_type = 'SELECT' AND a.grantee <> t.owner \
    ), held AS ( \
        SELECT t.view, t.name, a.grantee FROM tview t JOIN pg_catalog.pg_class c ON c.oid = t.view, \
               pg_catalog.aclexplode(c.relacl) a \
        WHERE a.privilege_type = 'SELECT' AND a.grantee <> t.owner \
    ), changes AS ( \
        SELECT view, name, grantee, true AS adds FROM (TABLE wanted EXCEPT TABLE held) g \
        UNION ALL \
        SELECT view, name, grantee, false FROM (TABLE held EXCEPT TABLE wanted) r \
    ) \
    SELECT view, pg_catalog.format(CASE WHEN adds THEN 'GRANT SELECT ON %s TO %s' \
                                        ELSE 'REVOKE SELECT ON %s FROM %s CASCADE' END, \
               name, \
               pg_catalog.string_agg(CASE WHEN grantee = 0 THEN 'PUBLIC' \
                   ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(grantee)) END, \
                   ', ' ORDER BY grantee)) \
    FROM changes GROUP BY view, name, adds ORDER BY view, adds";

/// Give the backing view of the TVIEW whose table is `table` (of every TVIEW
/// when `None`) that table's owner, when `owners`, and its `SELECT`
/// grants. Run after a statement that may have changed them, and when a
/// backing view is created or rebuilt.
///
/// # Errors
/// Returns an error if the catalog cannot be read, or an owner or grant cannot
/// be changed.
pub(crate) fn follow(table: Option<pg_sys::Oid>, owners: bool) -> TViewResult<()> {
    // The statements below are pg_tviews' own: the hook must not follow them.
    let _internal = crate::internal_ddl::InternalDdl::begin();
    // SAFETY: makes the changes of the statement just run visible to the queries below.
    unsafe { pg_sys::CommandCounterIncrement() };
    if owners {
        follow_owners(table)?;
    }
    follow_grants(table)
}

/// Each statement runs as the view's owner, who may always grant on it: the
/// caller needs no privilege on the view.
fn follow_grants(table: Option<pg_sys::Oid>) -> TViewResult<()> {
    let args = [crate::utils::spi::oid(table)];
    // Read-write: a fresh snapshot, which sees the statement just run.
    let changes = Spi::connect_mut(|client| {
        client
            .update(
                &SELECT_GRANT_CHANGES.replace("{meta}", &crate::catalog::meta_table()),
                None,
                &args,
            )?
            .map(|row| Ok((row.get::<pg_sys::Oid>(1)?, row.get::<String>(2)?)))
            .collect::<Result<Vec<_>, spi::Error>>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Compare backing view grants with their tables'".to_string(),
        pg_error: e.to_string(),
    })?;
    for (view, statement) in changes {
        let (Some(view), Some(statement)) = (view, statement) else {
            continue;
        };
        let _owner = crate::owner::AsOwner::of_table(view)?;
        crate::utils::spi::run_ddl(&statement)?;
    }
    Ok(())
}

/// Give each backing view whose owner is not its table's that owner, as the
/// current role (who just changed the table's owner, so may change the view's).
/// The new owner is granted CREATE on the extension's schema for the statement.
fn follow_owners(table: Option<pg_sys::Oid>) -> TViewResult<()> {
    let args = [crate::utils::spi::oid(table)];
    let moves = Spi::connect_mut(|client| {
        client
            .update(
                &format!(
                    "SELECT t.relowner, pg_catalog.format('ALTER VIEW %I.%I OWNER TO %I', \
                            n.nspname, v.relname, pg_catalog.pg_get_userbyid(t.relowner)) \
                     FROM {} m \
                     JOIN pg_catalog.pg_class t ON t.oid = m.table_oid::pg_catalog.oid \
                     JOIN pg_catalog.pg_class v ON v.oid = m.view_oid::pg_catalog.oid \
                     JOIN pg_catalog.pg_namespace n ON n.oid = v.relnamespace \
                     WHERE v.relowner <> t.relowner AND ($1 IS NULL OR t.oid = $1) \
                     ORDER BY v.oid",
                    crate::catalog::meta_table()
                ),
                None,
                &args,
            )?
            .map(|row| Ok((row.get::<pg_sys::Oid>(1)?, row.get::<String>(2)?)))
            .collect::<Result<Vec<_>, spi::Error>>()
    })
    .map_err(|e| TViewError::CatalogError {
        operation: "Compare backing view owners with their tables'".to_string(),
        pg_error: e.to_string(),
    })?;
    for (owner, statement) in moves {
        let (Some(owner), Some(statement)) = (owner, statement) else {
            continue;
        };
        super::in_extension_schema_for(owner, || crate::utils::spi::run_ddl(&statement))?;
    }
    Ok(())
}
