//! Extension lifecycle: initialization, version, and runtime checks.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// Get the version of the `pg_tviews` extension
#[pg_extern]
#[allow(clippy::missing_const_for_fn)] // Reason: pgrx #[pg_extern] is incompatible with const fn
fn pg_tviews_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Detect and recover from post-crash truncation of UNLOGGED TVIEW tables.
///
/// Checks if a TVIEW table has been truncated due to crash and automatically
/// refreshes it if recovery is needed. This function is safe to call multiple times
/// and will only perform refresh when actually needed. Requires owning the TVIEW
/// (or the extension): the rebuild runs as its owner.
///
/// # Arguments
/// * `entity_name` - Name of the TVIEW entity (without tv_ prefix)
///
/// # Returns
/// `Ok(true)` if recovery was performed, `Ok(false)` if no recovery needed
#[pg_extern]
pub fn pg_tviews_recover_after_crash(entity_name: &str) -> Result<bool, ErrorReport> {
    crate::revision::check();
    let meta = crate::catalog::TviewMeta::load_by_entity(entity_name)?.ok_or_else(|| {
        crate::TViewError::MetadataNotFound {
            entity: entity_name.to_string(),
        }
    })?;
    // The rebuild runs as the TVIEW's owner: only its owner may ask for it.
    crate::owner::require_owner(meta.tview_oid, &format!("tv_{entity_name}"))?;
    if detect_post_crash_truncation(entity_name)? {
        // Only this TVIEW was reset; what reads it is unchanged.
        crate::admin::rebuild_one(entity_name)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Detect if a TVIEW table has been truncated due to UNLOGGED table crash recovery.
///
/// Returns `true` if the table is empty but the backing view contains data,
/// indicating a post-crash truncation that requires refresh.
///
/// # Arguments
/// * `entity_name` - Name of the TVIEW entity (without tv_ prefix)
///
/// # Returns
/// `Ok(true)` if crash recovery is needed, `Ok(false)` if table is healthy
pub fn detect_post_crash_truncation(entity_name: &str) -> crate::TViewResult<bool> {
    match crate::replication::TviewRelation::load(Some(entity_name))?.first() {
        Some(rel) => rel.needs_rebuild(),
        None => Ok(false), // Entity not found
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
