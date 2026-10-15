//! Every function SQL can call (ADR 0211).
//!
//! Each one checks the catalog revision, resolves the TVIEW it acts on, checks
//! that the caller may act on it, and calls the module that does the work. No
//! `#[pg_extern]` lives anywhere else, so these hold by construction:
//!
//! - a TVIEW is named one way everywhere: the first parameter, `tview text`, takes
//!   its entity, `tv_<entity>` or `schema.tv_<entity>` ([`crate::catalog::resolve`]);
//! - a function acting on one TVIEW requires owning it (or the extension);
//! - messages name a TVIEW by its relation.
//!
//! Relations `pg_tviews` publishes are nouns (`tviews.registry`, `tviews.stats`);
//! functions are `pg_tviews_<verb>`.

mod internal;
mod maintenance;
mod session;
mod status;
mod tview;

use crate::catalog::TviewMeta;
use crate::catalog::resolve::{self, Found};
use crate::error::TViewResult;

/// The TVIEW `tview` names, once the caller is known to own it (or the
/// extension); its plan is not read.
fn owned(tview: &str) -> TViewResult<Found> {
    let found = resolve::find(tview)?;
    crate::owner::require_owner(found.table, &table_name(found.table, &found.entity))?;
    Ok(found)
}

/// [`owned`], with the TVIEW's catalog row and plan.
fn owned_meta(tview: &str) -> TViewResult<TviewMeta> {
    owned(tview)?;
    resolve::resolve(tview)
}

/// The TVIEW's table, schema-qualified, for messages.
fn relation(meta: &TviewMeta) -> String {
    table_name(meta.tview_oid, &meta.entity_name)
}

/// Table `table` of TVIEW `entity`, schema-qualified; `tv_<entity>` once it is gone.
fn table_name(table: pgrx::pg_sys::Oid, entity: &str) -> String {
    crate::utils::qualified_relname_from_oid(table).unwrap_or_else(|_| format!("tv_{entity}"))
}
