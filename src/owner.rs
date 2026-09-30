//! Run the work of `pg_tviews` as the role that owns it (issues #136, #134).
//!
//! The flush refreshes TVIEWs on behalf of whichever role wrote to a base table.
//! As `REFRESH MATERIALIZED VIEW` does, every read and write of a `tv_*` table in
//! the flush runs as that table's owner, inside a security-restricted operation
//! and with `search_path` set to `pg_catalog, pg_temp`. The writer then needs no
//! privilege on the TVIEW, its backing view or the tables the view reads, and
//! cannot get the owner to run a function it planted on its `search_path`.
//!
//! The registration catalog is writable only by the extension's owner. A caller
//! allowed to change a TVIEW (checked with [`require_owner`] beforehand) has its
//! catalog write run as the extension's owner, the same way.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// While alive, the current user is a TVIEW's owner. Dropping it restores the
/// previous user, security context and `search_path`. On an error, the
/// (sub)transaction abort restores them instead, so the guard leaves them alone
/// while unwinding.
pub struct AsOwner {
    saved_user: Oid,
    saved_context: i32,
    guc_level: i32,
}

impl AsOwner {
    /// Switch to the owner of `entity`'s `tv_*` table.
    ///
    /// # Errors
    /// Returns an error if the entity is not registered or its table is gone.
    pub fn of_entity(entity: &str) -> TViewResult<Self> {
        let meta = crate::catalog::TviewMeta::load_by_entity(entity)?.ok_or_else(|| {
            TViewError::MetadataNotFound {
                entity: entity.to_string(),
            }
        })?;
        Self::of_table(meta.tview_oid)
    }

    /// Switch to the owner of the relation `table`.
    ///
    /// # Errors
    /// Returns an error if the relation does not exist.
    pub fn of_table(table: Oid) -> TViewResult<Self> {
        Ok(Self::role(relation_owner(table)?))
    }

    /// Switch to the owner of the `pg_tviews` extension, to write its catalog.
    ///
    /// # Errors
    /// Returns an error if the extension's row cannot be read.
    pub fn of_extension() -> TViewResult<Self> {
        let owner = Spi::connect(|client| {
            client
                .select(
                    "SELECT extowner FROM pg_catalog.pg_extension WHERE extname = 'pg_tviews'",
                    None,
                    &[],
                )?
                .first()
                .get_one::<Oid>()
        })
        .map_err(|e| TViewError::CatalogError {
            operation: "Look up the owner of pg_tviews".to_string(),
            pg_error: e.to_string(),
        })?
        .ok_or_else(|| TViewError::CatalogError {
            operation: "Look up the owner of pg_tviews".to_string(),
            pg_error: "extension pg_tviews is not installed".to_string(),
        })?;
        Ok(Self::role(owner))
    }

    fn role(owner: Oid) -> Self {
        let mut saved_user = pg_sys::InvalidOid;
        let mut saved_context = 0;
        // SAFETY: plain backend-global state, as REFRESH MATERIALIZED VIEW sets it;
        // the GUC nest level opened here is closed by Drop or by the abort.
        let guc_level = unsafe {
            pg_sys::GetUserIdAndSecContext(&raw mut saved_user, &raw mut saved_context);
            pg_sys::SetUserIdAndSecContext(
                owner,
                saved_context
                    | (pg_sys::SECURITY_LOCAL_USERID_CHANGE
                        | pg_sys::SECURITY_RESTRICTED_OPERATION)
                        .cast_signed(),
            );
            let level = pg_sys::NewGUCNestLevel();
            pg_sys::set_config_option(
                c"search_path".as_ptr(),
                c"pg_catalog, pg_temp".as_ptr(),
                pg_sys::GucContext::PGC_USERSET,
                pg_sys::GucSource::PGC_S_SESSION,
                pg_sys::GucAction::GUC_ACTION_SAVE,
                true,
                0,
                false,
            );
            level
        };
        Self {
            saved_user,
            saved_context,
            guc_level,
        }
    }
}

impl Drop for AsOwner {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        // SAFETY: undoes exactly what `of_table` did, innermost first.
        unsafe {
            pg_sys::AtEOXact_GUC(false, self.guc_level);
            pg_sys::SetUserIdAndSecContext(self.saved_user, self.saved_context);
        }
    }
}

/// Owner of the relation `table`, from the syscache: cheap enough for the row
/// trigger, and never stale after an `ALTER TABLE … OWNER TO`.
fn relation_owner(table: Oid) -> TViewResult<Oid> {
    // SAFETY: SearchSysCache1 returns a valid pg_class tuple or null; the tuple's
    // fixed part is a FormData_pg_class, and it is released before returning.
    let owner = unsafe {
        let tuple = pg_sys::SearchSysCache1(
            pg_sys::SysCacheIdentifier::RELOID.cast_signed(),
            pg_sys::Datum::from(table),
        );
        if tuple.is_null() {
            None
        } else {
            #[allow(clippy::cast_ptr_alignment)]
            // Reason: GETSTRUCT points at MAXALIGNed tuple data
            let form = pg_sys::GETSTRUCT(tuple).cast::<pg_sys::FormData_pg_class>();
            let owner = (*form).relowner;
            pg_sys::ReleaseSysCache(tuple);
            Some(owner)
        }
    };
    owner.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Look up the owner of relation {table:?}"),
        pg_error: "relation does not exist".to_string(),
    })
}
