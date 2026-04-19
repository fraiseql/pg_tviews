//! Extension lifecycle: initialization, version, and runtime checks.

use pgrx::PgBuiltInOids;
use pgrx::PgOid;
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::utils::quote_identifier;

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
pub fn pg_tviews_recover_after_crash(entity_name: &str) -> crate::TViewResult<bool> {
    if detect_post_crash_truncation(entity_name)? {
        // Perform full refresh of the TVIEW
        Spi::run_with_args(
            "SELECT pg_tviews_refresh($1)",
            &[unsafe {
                DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            }],
        )?;
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
    // Get the table OID and view OID from pg_tview_meta
    let (table_oid_opt, view_oid_opt): (Option<Oid>, Option<Oid>) = Spi::get_two_with_args(
        "SELECT table_oid, view_oid FROM pg_tview_meta WHERE entity = $1",
        &[unsafe {
            DatumWithOid::new(entity_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
        }],
    )?;

    let table_oid = match table_oid_opt {
        Some(t) => t,
        None => return Ok(false), // Entity not found
    };

    let view_oid = match view_oid_opt {
        Some(v) => v,
        None => return Ok(false), // Entity not found
    };

    // Check if TVIEW table is UNLOGGED
    let is_unlogged: Option<bool> = Spi::get_one_with_args(
        "SELECT relpersistence = 'u' FROM pg_class WHERE oid = $1 AND relkind = 'r'",
        &[unsafe { DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }],
    )?;

    // If table doesn't exist or isn't UNLOGGED, no crash detection needed
    if !is_unlogged.unwrap_or(false) {
        return Ok(false);
    }

    // Get schema name for qualified queries
    let schema: Option<String> = Spi::get_one_with_args(
        "SELECT n.nspname::text FROM pg_class c JOIN pg_namespace n ON c.relnamespace = n.oid WHERE c.oid = $1",
        &[unsafe { DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }],
    )?;

    let schema = schema.unwrap_or_else(|| "public".to_string());

    // Get table and view names
    let tview_table: Option<String> = Spi::get_one_with_args(
        "SELECT relname::text FROM pg_class WHERE oid = $1",
        &[unsafe { DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }],
    )?;

    let backing_view: Option<String> = Spi::get_one_with_args(
        "SELECT relname::text FROM pg_class WHERE oid = $1",
        &[unsafe { DatumWithOid::new(view_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }],
    )?;

    let tview_table = tview_table.unwrap_or_default();
    let backing_view = backing_view.unwrap_or_default();

    // Check if table has any rows (O(1) emptiness check)
    let table_has_rows: Option<bool> = Spi::get_one(&format!(
        "SELECT EXISTS(SELECT 1 FROM {}.{} LIMIT 1)",
        quote_identifier(&schema),
        quote_identifier(&tview_table)
    ))?;

    // If table has data, no crash detected
    if table_has_rows.unwrap_or(false) {
        return Ok(false);
    }

    // Check if backing view has any data
    let view_has_rows: Option<bool> = Spi::get_one(&format!(
        "SELECT EXISTS(SELECT 1 FROM {}.{} LIMIT 1)",
        quote_identifier(&schema),
        quote_identifier(&backing_view)
    ))?;

    // If backing view has data but table is empty, crash detected
    Ok(view_has_rows.unwrap_or(false))
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
