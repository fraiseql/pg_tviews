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

use crate::error::TViewResult;
use pgrx::prelude::*;

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
    // Read-write: a fresh snapshot, which sees the statement just run.
    let changes = crate::catalog::queries::grant_changes(table)?;
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
    let moves = crate::catalog::queries::owner_changes(table)?;
    for (owner, statement) in moves {
        let (Some(owner), Some(statement)) = (owner, statement) else {
            continue;
        };
        super::in_extension_schema_for(owner, || crate::utils::spi::run_ddl(&statement))?;
    }
    Ok(())
}
