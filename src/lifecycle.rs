//! Extension lifecycle: initialization, version, and runtime checks.

use pgrx::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

// Static cache for jsonb_delta availability (performance optimization)
static JSONB_IVM_AVAILABLE: AtomicBool = AtomicBool::new(false);
static JSONB_IVM_CHECKED: AtomicBool = AtomicBool::new(false);

/// Get the version of the `pg_tviews` extension
#[pg_extern]
#[allow(clippy::missing_const_for_fn)] // pgrx #[pg_extern] is incompatible with const fn
fn pg_tviews_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Debug function to check if `ProcessUtility` hook is installed
#[pg_extern]
const fn pg_tviews_hook_status() -> &'static str {
    "Extension loaded - hook installation attempted in _PG_init"
}

/// Check if `jsonb_delta` extension is available at runtime (cached)
/// Returns true if extension is installed, false otherwise
///
/// This function caches the result after the first check to avoid
/// repeated queries to `pg_extension` on every cascade operation.
pub fn check_jsonb_delta_available() -> bool {
    if JSONB_IVM_CHECKED.load(Ordering::Relaxed) {
        return JSONB_IVM_AVAILABLE.load(Ordering::Relaxed);
    }

    let result: Result<bool, spi::Error> = Spi::connect(|client| {
        let rows = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')",
            None,
            &[],
        )?;

        for row in rows {
            if let Some(exists) = row[1].value::<bool>()? {
                return Ok(exists);
            }
        }
        Ok(false)
    });

    let is_available = result.unwrap_or(false);

    JSONB_IVM_AVAILABLE.store(is_available, Ordering::Relaxed);
    JSONB_IVM_CHECKED.store(true, Ordering::Relaxed);

    is_available
}

/// Export as SQL function for testing
#[pg_extern]
fn pg_tviews_check_jsonb_delta() -> bool {
    check_jsonb_delta_available()
}

/// Reset the jsonb_delta availability cache
/// Called during cache invalidation when the extension is created or dropped
pub fn invalidate_jsonb_delta_cache() {
    JSONB_IVM_CHECKED.store(false, Ordering::Relaxed);
    JSONB_IVM_AVAILABLE.store(false, Ordering::Relaxed);
}

/// Initialize the extension
/// Installs the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
///
/// Safety: Only installs hooks when running in a proper `PostgreSQL` backend,
/// not during initdb or other bootstrap contexts.
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    crate::config::register_gucs();

    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    // Register transaction callbacks once at startup.
    // PostgreSQL's RegisterXactCallback appends to a persistent linked list,
    // so registering per-transaction would accumulate N copies after N transactions.
    unsafe {
        crate::queue::xact::register_xact_callback();
        crate::queue::xact::register_subxact_callback();
    }
}
