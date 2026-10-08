//! Extension lifecycle: initialization, version, and runtime checks.

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

/// [`crate::utils::log_once`] key of "`jsonb_delta` is not installed" (issue #159).
pub const JSONB_DELTA_MISSING: &str = "jsonb_delta_missing";

/// Get the version of the `pg_tviews` extension
#[pg_extern]
#[allow(clippy::missing_const_for_fn)] // Reason: pgrx #[pg_extern] is incompatible with const fn
fn pg_tviews_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Check if `jsonb_delta` extension is available at runtime (cached)
/// Returns true if extension is installed, false otherwise
///
/// This function caches the result after the first check to avoid
/// repeated queries to `pg_extension` on every cascade operation.
#[must_use]
pub fn check_jsonb_delta_available() -> bool {
    jsonb_delta_schema().is_some()
}

/// Quoted schema of the `jsonb_delta` extension, for a patch about to be applied.
///
/// # Errors
/// [`crate::TViewError::JsonbDeltaMissing`] when it is not installed (dropped
/// since the patch was captured): an unqualified or `public` fallback would call
/// whatever function of that name a role with CREATE there planted.
pub fn require_jsonb_delta_schema() -> crate::TViewResult<String> {
    jsonb_delta_schema().ok_or(crate::TViewError::JsonbDeltaMissing)
}

/// Quoted schema of the `jsonb_delta` extension (cached), `None` when it is not
/// installed. Patch calls are qualified with it so they do not depend on the
/// session's `search_path`.
pub fn jsonb_delta_schema() -> Option<String> {
    if let Some(schema) = crate::cache::JSONB_DELTA_SCHEMA.with(|m| m.get(&())) {
        return schema;
    }
    let schema = Spi::connect(|client| {
        client
            .select(
                "SELECT pg_catalog.quote_ident(n.nspname) \
                 FROM pg_catalog.pg_extension e \
                 JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
                 WHERE e.extname = 'jsonb_delta'",
                None,
                &[],
            )?
            .first()
            .get_one::<String>()
    })
    .ok()
    .flatten();
    crate::cache::JSONB_DELTA_SCHEMA.with(|m| m.insert((), schema.clone()));
    schema
}

/// Detect and recover from post-crash truncation of UNLOGGED TVIEW tables.
///
/// Checks if a TVIEW table has been truncated due to crash and automatically
/// refreshes it if recovery is needed. This function is safe to call multiple times
/// and will only perform refresh when actually needed.
///
/// # Arguments
/// * `entity_name` - Name of the TVIEW entity (without tv_ prefix)
///
/// # Returns
/// `Ok(true)` if recovery was performed, `Ok(false)` if no recovery needed
#[pg_extern]
pub fn pg_tviews_recover_after_crash(entity_name: &str) -> Result<bool, ErrorReport> {
    crate::revision::check();
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

/// Export as SQL function for testing
#[pg_extern]
fn pg_tviews_check_jsonb_delta() -> bool {
    check_jsonb_delta_available()
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
        crate::queue::xact::register_xact_callback();
        crate::queue::xact::register_subxact_callback();
    }
}
