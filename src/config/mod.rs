//! Configuration: Compile-time and Runtime Settings
//!
//! This module centralizes all configuration for `pg_tviews`:
//! - **Compile-time constants**: Fixed safety limits
//! - **Runtime GUC settings**: Tunable via `SET pg_tviews.*` / `SHOW pg_tviews.*`
//!
//! ## GUC Parameters
//!
//! | Parameter | Type | Default | Description |
//! |-----------|------|---------|-------------|
//! | `pg_tviews.max_propagation_depth` | int | 100 | Max cascade iterations |
//! | `pg_tviews.graph_cache_enabled` | bool | true | Cache dependency graphs |
//! | `pg_tviews.table_cache_enabled` | bool | true | Cache table→entity mappings |
//! | `pg_tviews.metrics_enabled` | bool | false | Collect refresh metrics |
//! | `pg_tviews.audit_enabled` | bool | false | Audit logging (opt-in) |
//! | `pg_tviews.log_level` | string | "info" | Logging verbosity |
//! | `pg_tviews.suspend_triggers` | bool | false | Suspend trigger-based refresh |
//! | `pg_tviews.max_queue_size` | int | 10000 | Refresh-queue backpressure limit |
//! | `pg_tviews.max_dependency_depth` | int | 10 | Max `pg_depend` traversal depth |
//! | `pg_tviews.batch_size` | int | 1000 | Max PKs per bulk-refresh statement |
//! | `pg_tviews.cache_size` | int | 10000 | Max entries per in-memory cache |
//!
//! ## Compile-time Constants
//!
//! - `MAX_DEPENDENCY_DEPTH`: default for `pg_tviews.max_dependency_depth`
//! - `DEBUG_DEPENDENCIES`: Enable verbose dependency logging

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};

/// Maximum depth for `pg_depend` traversal
/// Prevents infinite recursion and overly complex view hierarchies
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Enable verbose dependency logging (for debugging)
pub const DEBUG_DEPENDENCIES: bool = false;

// ── GUC statics ──────────────────────────────────────────────────────────

static MAX_PROPAGATION_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(100);
static GRAPH_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static METRICS_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static LOG_LEVEL_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"info"));
static UNION_DUPLICATE_POLICY_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"error"));
static MAX_QUEUE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static AUDIT_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static UNLOGGED_BY_DEFAULT_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static SUSPEND_TRIGGERS_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
// Default must equal MAX_DEPENDENCY_DEPTH (10); GucSetting::new needs an i32 literal.
static MAX_DEPENDENCY_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(10);
static BATCH_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(1_000);
static CACHE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);

// ── GUC registration (called from _PG_init) ─────────────────────────────

/// Register all `pg_tviews.*` GUC parameters with `PostgreSQL`.
///
/// Must be called exactly once from `_PG_init()`, before any code reads
/// the GUC values.
pub fn register_gucs() {
    GucRegistry::define_int_guc(
        c"pg_tviews.max_propagation_depth",
        c"Maximum cascade propagation iterations before aborting.",
        c"Prevents infinite loops in circular dependency chains.",
        &MAX_PROPAGATION_DEPTH_GUC,
        1,      // min
        10_000, // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.graph_cache_enabled",
        c"Enable in-memory caching of entity dependency graphs.",
        c"When false, graphs are loaded from pg_tview_meta on every refresh.",
        &GRAPH_CACHE_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.table_cache_enabled",
        c"Enable in-memory caching of table OID to entity name mappings.",
        c"When false, entity lookups query pg_tview_meta on every trigger.",
        &TABLE_CACHE_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.metrics_enabled",
        c"Enable collection of refresh metrics.",
        c"When true, per-transaction refresh statistics are tracked.",
        &METRICS_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_string_guc(
        c"pg_tviews.log_level",
        c"Logging verbosity for pg_tviews operations.",
        c"Allowed values: debug, info, warning, error.",
        &LOG_LEVEL_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_string_guc(
        c"pg_tviews.union_duplicate_policy",
        c"Policy when a UNION ALL backing view returns multiple rows for the same key.",
        c"Allowed values: 'first' (silently take first row), 'error' (abort transaction).",
        &UNION_DUPLICATE_POLICY_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"pg_tviews.max_queue_size",
        c"Maximum number of refresh items allowed in the transaction queue.",
        c"When exceeded, new refresh enqueues raise an error to prevent unbounded queue growth.",
        &MAX_QUEUE_SIZE_GUC,
        1,         // min
        1_000_000, // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.audit_enabled",
        c"Enable audit logging of TVIEW operations to pg_tview_audit_log.",
        c"When false, refresh/create/drop operations are not logged.",
        &AUDIT_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.unlogged_by_default",
        c"Create TVIEW tables as UNLOGGED by default.",
        c"When true, new TVIEWs are created as UNLOGGED tables for better write performance.",
        &UNLOGGED_BY_DEFAULT_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.suspend_triggers",
        c"Suspend trigger-based refresh during bulk operations.",
        c"When true, row-level triggers will not enqueue refresh tasks.",
        &SUSPEND_TRIGGERS_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"pg_tviews.max_dependency_depth",
        c"Maximum pg_depend traversal depth when building the dependency graph.",
        c"Bounds how deep view-on-view hierarchies may nest before an error is raised.",
        &MAX_DEPENDENCY_DEPTH_GUC,
        1,   // min
        100, // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"pg_tviews.batch_size",
        c"Maximum primary keys processed per statement during bulk refresh.",
        c"Large multi-row changes are chunked into batches of this size to bound \
          statement size and memory on very large bulk operations.",
        &BATCH_SIZE_GUC,
        1,         // min
        1_000_000, // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"pg_tviews.cache_size",
        c"Maximum entries kept in each in-memory metadata cache.",
        c"When a cache exceeds this many entries it is cleared and repopulated \
          lazily, bounding per-backend memory on high-cardinality workloads.",
        &CACHE_SIZE_GUC,
        1,          // min
        10_000_000, // max
        GucContext::Userset,
        GucFlags::default(),
    );
}

// ── Public accessors (same signatures as the old const fns) ──────────────

/// Maximum propagation iteration depth (default: 100)
/// Prevents infinite loops in dependency chains
#[must_use]
pub fn max_propagation_depth() -> usize {
    MAX_PROPAGATION_DEPTH_GUC.get().unsigned_abs() as usize
}

/// Check if graph caching is enabled
#[must_use]
pub fn graph_cache_enabled() -> bool {
    GRAPH_CACHE_ENABLED_GUC.get()
}

/// Check if table caching is enabled
#[must_use]
pub fn table_cache_enabled() -> bool {
    TABLE_CACHE_ENABLED_GUC.get()
}

/// Get the current log level
#[must_use]
pub fn log_level() -> String {
    LOG_LEVEL_GUC.get().map_or_else(
        || "info".to_owned(),
        |cstr| cstr.to_str().unwrap_or("info").to_owned(),
    )
}

/// Check if metrics collection is enabled
#[must_use]
pub fn metrics_enabled() -> bool {
    METRICS_ENABLED_GUC.get()
}

/// Policy for UNION ALL backing views that return duplicate rows for the same key.
///
/// - `"error"` (default): abort the transaction with a clear error message.
/// - `"first"`: silently take the first row returned.
#[must_use]
pub fn union_duplicate_policy() -> String {
    UNION_DUPLICATE_POLICY_GUC.get().map_or_else(
        || "error".to_owned(),
        |cstr| cstr.to_str().unwrap_or("error").to_owned(),
    )
}

/// Maximum queue size before backpressure enforcement (default: 10000)
/// Prevents unbounded queue growth during high-load scenarios
#[must_use]
pub fn max_queue_size() -> usize {
    MAX_QUEUE_SIZE_GUC.get().unsigned_abs() as usize
}

/// Check if audit logging is enabled (default: false, opt-in)
#[must_use]
pub fn audit_enabled() -> bool {
    AUDIT_ENABLED_GUC.get()
}

/// Check if TVIEWs should be created as UNLOGGED by default (default: true)
#[must_use]
pub fn unlogged_by_default() -> bool {
    UNLOGGED_BY_DEFAULT_GUC.get()
}

/// Check if trigger-based refresh is suspended (default: false)
#[must_use]
pub fn suspend_triggers() -> bool {
    SUSPEND_TRIGGERS_GUC.get()
}

/// Maximum `pg_depend` traversal depth when building the dependency graph
/// (default: 10). Bounds view-on-view nesting.
#[must_use]
pub fn max_dependency_depth() -> usize {
    MAX_DEPENDENCY_DEPTH_GUC.get().unsigned_abs() as usize
}

/// Maximum primary keys processed per statement during bulk refresh (default: 1000).
/// Large multi-row changes are chunked into batches of this size.
#[must_use]
pub fn batch_size() -> usize {
    BATCH_SIZE_GUC.get().unsigned_abs().max(1) as usize
}

/// Maximum entries kept in each in-memory metadata cache before it is cleared and
/// repopulated lazily (default: 10000). Bounds per-backend cache memory.
#[must_use]
pub fn cache_size() -> usize {
    CACHE_SIZE_GUC.get().unsigned_abs().max(1) as usize
}
