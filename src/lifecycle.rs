//! Extension lifecycle: initialization, version, and runtime checks.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Get the version of the `pg_tviews` extension
#[pg_extern]
#[allow(clippy::missing_const_for_fn)] // Reason: pgrx #[pg_extern] is incompatible with const fn
fn pg_tviews_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Fill `entity`'s TVIEW from its backing view if PostgreSQL reset it (an
/// UNLOGGED table emptied by a crash restart or a promotion), and say whether it
/// did. A TVIEW that is merely empty is left alone. Requires owning the TVIEW (or
/// the extension): the fill runs as its owner.
///
/// # Errors
/// Returns an error if the entity is not registered or the fill fails.
#[pg_extern]
pub fn pg_tviews_recover_after_crash(entity_name: &str) -> Result<bool, ErrorReport> {
    crate::revision::check();
    let meta = crate::catalog::TviewMeta::load_by_entity(entity_name)?.ok_or_else(|| {
        crate::TViewError::MetadataNotFound {
            entity: entity_name.to_string(),
        }
    })?;
    // The fill runs as the TVIEW's owner: only its owner may ask for it.
    crate::owner::require_owner(meta.tview_oid, &format!("tv_{entity_name}"))?;
    Ok(validity::fill_if_reset(entity_name)?)
}

/// Whether an UNLOGGED TVIEW's rows can be trusted, and its fill when they can't.
///
/// PostgreSQL empties every UNLOGGED table on a crash restart and on promotion,
/// and leaves no other trace of it. `pg_tview_valid` is UNLOGGED too and holds
/// a row per UNLOGGED TVIEW table whose rows are trusted, so the reset empties
/// both together. A missing row makes the next write fill that TVIEW from its
/// view; inserting the row is the claim, so concurrent writers wait for the
/// claimer instead of filling twice (#214). The row is not dumped: a restored
/// TVIEW is filled once.
pub mod validity {
    use crate::TViewResult;
    use crate::catalog::TviewMeta;
    use pgrx::pg_sys::{self, Oid};
    use pgrx::prelude::*;

    thread_local! {
        /// The TVIEW tables this backend found trusted (or filled).
        static CHECKED: std::cell::RefCell<std::collections::HashSet<u32>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
    }

    fn table() -> String {
        format!("{}.pg_tview_valid", crate::utils::ext_schema())
    }

    fn unlogged(table_oid: Oid) -> bool {
        // SAFETY: a syscache lookup; '\0' for a relation that no longer exists.
        let persistence = unsafe { pg_sys::get_rel_persistence(table_oid) };
        u8::try_from(persistence).ok() == Some(pg_sys::RELPERSISTENCE_UNLOGGED)
    }

    /// Whether `table_oid` is an UNLOGGED table whose rows can't be trusted.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be read.
    pub fn needs_fill(table_oid: Oid) -> TViewResult<bool> {
        if !unlogged(table_oid) {
            return Ok(false);
        }
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE table_oid = $1)",
            table()
        );
        // Read-only (`select`): it runs on a hot standby too.
        let marked = Spi::connect(|client| {
            client
                .select(&sql, Some(1), &[crate::utils::spi::oid(table_oid)])?
                .first()
                .get_one::<bool>()
        })
        .map_err(|e| crate::utils::spi::error(&sql, &e))?;
        Ok(marked != Some(true))
    }

    /// Record that the rows of `table_oid`, filled in this transaction, can be
    /// trusted. Nothing for a LOGGED table.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be written.
    pub fn mark(table_oid: Oid) -> TViewResult<bool> {
        if !unlogged(table_oid) {
            return Ok(false);
        }
        let sql = format!(
            "INSERT INTO {} VALUES ($1) ON CONFLICT DO NOTHING RETURNING table_oid",
            table()
        );
        let _owner = crate::owner::AsOwner::of_extension()?;
        // No row when it was there already (`first()` of an empty result is an error).
        let inserted = Spi::connect_mut(|client| {
            Ok::<_, pgrx::spi::Error>(
                !client
                    .update(&sql, None, &[crate::utils::spi::oid(table_oid)])?
                    .is_empty(),
            )
        })
        .map_err(|e| crate::utils::spi::error(&sql, &e))?;
        Ok(inserted)
    }

    /// Forget `table_oid`: it is dropped, or no longer UNLOGGED.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be written.
    pub fn forget(table_oid: Oid) -> TViewResult<()> {
        let sql = format!("DELETE FROM {} WHERE table_oid = $1", table());
        let _owner = crate::owner::AsOwner::of_extension()?;
        crate::utils::spi::run(&sql, &[crate::utils::spi::oid(table_oid)])
    }

    /// Make `entity`'s rows trustworthy before a write refreshes them: once per
    /// entity and backend, since a reset ends every backend. See [`fill_if_reset`].
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or a fill fails.
    pub fn ensure(entity: &str) -> TViewResult<bool> {
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(false);
        };
        let table = meta.tview_oid.to_u32();
        if CHECKED.with_borrow(|checked| checked.contains(&table)) {
            return Ok(false);
        }
        let filled = fill_if_reset(entity)?;
        CHECKED.with_borrow_mut(|checked| checked.insert(table));
        Ok(filled)
    }

    /// Check the tables again: a rolled-back (sub)transaction may have undone a
    /// fill, and a prepared one may not commit.
    pub fn forget_checks() {
        CHECKED.with_borrow_mut(std::collections::HashSet::clear);
    }

    /// If `entity`'s table is UNLOGGED and was reset, fill the TVIEWs it reads,
    /// then claim and fill it. Returns whether this transaction filled it.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or a fill fails.
    pub fn fill_if_reset(entity: &str) -> TViewResult<bool> {
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(false);
        };
        if !needs_fill(meta.tview_oid)? {
            return Ok(false);
        }
        let graph = crate::flush::EntityDepGraph::load()?;
        for dependency in graph.children.get(entity).into_iter().flatten() {
            fill_if_reset(dependency)?;
        }
        // Another transaction may have claimed it since: the claim then waits
        // for it and finds the row.
        if !mark(meta.tview_oid)? {
            return Ok(false);
        }
        crate::admin::refill(entity)?;
        Ok(true)
    }
}

/// Initialize the extension
/// Installs the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
///
/// Safety: Only installs hooks when running in a proper `PostgreSQL` backend,
/// not during initdb or other bootstrap contexts.
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    crate::config::register_gucs();
    crate::cache::register_relcache_callback();
    crate::rebuild_worker::register();

    // SAFETY: _PG_init runs in PostgreSQL backend context. Installing hooks and
    // registering callbacks is valid in this context.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    // Register transaction callbacks once at startup.
    // PostgreSQL's RegisterXactCallback appends to a persistent linked list,
    // so registering per-transaction would accumulate N copies after N transactions.
    // SAFETY: Transaction callbacks are registered in backend initialization context.
    unsafe {
        crate::flush::register_xact_callback();
        crate::flush::register_subxact_callback();
    }
}
