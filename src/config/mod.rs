//! Runtime settings (`pg_tviews.*`), and the policy names TVIEW options use.
//!
//! What a TVIEW is comes from its definition and its options, never from a
//! setting (ADR 0220): settings only tune how a session works. A setting that
//! decides whether a write or a creation succeeds can only be changed by a
//! superuser, so every session agrees on it.
//!
//! | Parameter | Type | Default | Context | Description |
//! |-----------|------|---------|---------|-------------|
//! | `pg_tviews.max_propagation_depth` | int | 100 | superuser | Max cascade iterations |
//! | `pg_tviews.max_dependency_depth` | int | 10 | superuser | Max `pg_depend` traversal depth |
//! | `pg_tviews.max_queue_size` | int | 10000 | superuser | Refresh-queue backpressure limit |
//! | `pg_tviews.lock_escalation_threshold` | int | 64 | superuser | Value locks per relation before a transaction locks the relation (ADR 0207) |
//! | `pg_tviews.audit_enabled` | bool | false | superuser | Audit logging (opt-in) |
//! | `pg_tviews.graph_cache_enabled` | bool | true | superuser, hidden | Cache dependency graphs |
//! | `pg_tviews.table_cache_enabled` | bool | true | superuser, hidden | Cache TVIEW catalog rows and plans |
//! | `pg_tviews.direct_patch_enabled` | bool | true | superuser, hidden | Direct-patch fast path |
//! | `pg_tviews.test_skip_ctas_intercept` | bool | false | superuser, hidden | Test only |
//! | `pg_tviews.batch_size` | int | 1000 | user | Max keys per bulk-refresh statement |
//! | `pg_tviews.cache_size` | int | 10000 | user | Max entries per in-memory cache |
//! | `pg_tviews.report_max_tracked` | int | 10000 | user | Changed rows journaled per transaction for `pg_tviews_flush_and_report` (0 = off) |
//! | `pg_tviews.auto_rebuild_databases` | string | `*` | postmaster | Databases whose reset UNLOGGED TVIEWs are rebuilt after recovery: `*` every one, a list, or empty for none |

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};

/// What a TVIEW does about a base table it reads whose writes no cascade maps to
/// its keys (option `uncascaded_policy`, default `error`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UncascadedPolicy {
    /// WARNING at create time; such writes leave rows stale until a mapped table changes.
    Warn,
    /// ERROR at create time; nothing is created.
    #[default]
    Error,
    /// NOTICE at create time; such writes refresh the whole TVIEW at flush.
    FullRefresh,
}

impl UncascadedPolicy {
    /// The name stored in `pg_tview_meta.uncascaded_policy`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
            Self::FullRefresh => "full_refresh",
        }
    }

    /// Parse a name as written in an option; `None` for anything else.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "full_refresh" => Some(Self::FullRefresh),
            _ => None,
        }
    }
}

// ── GUC statics ──────────────────────────────────────────────────────────

static MAX_PROPAGATION_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(100);
static MAX_DEPENDENCY_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(10);
static MAX_QUEUE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static LOCK_ESCALATION_THRESHOLD_GUC: GucSetting<i32> = GucSetting::<i32>::new(64);
static AUDIT_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static GRAPH_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static DIRECT_PATCH_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TEST_SKIP_CTAS_INTERCEPT_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static BATCH_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(1_000);
static CACHE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static REPORT_MAX_TRACKED_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static AUTO_REBUILD_DATABASES_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"*"));

// ── GUC registration (called from _PG_init) ─────────────────────────────

/// Register all `pg_tviews.*` GUC parameters with `PostgreSQL`.
///
/// Must be called exactly once from `_PG_init()`, before any code reads
/// the GUC values.
pub fn register_gucs() {
    register_limits();
    register_diagnostics();
    register_session_settings();
    register_postmaster_gucs();

    // Every pg_tviews.* setting is defined above: refuse any other name, so a
    // typo or a removed setting raises instead of doing nothing.
    // SAFETY: called from _PG_init with a static, NUL-terminated prefix.
    unsafe { pgrx::pg_sys::MarkGUCPrefixReserved(c"pg_tviews".as_ptr()) };
}

/// Settings that decide whether a write or a creation succeeds: the same for
/// every session, so only a superuser changes them.
fn register_limits() {
    GucRegistry::define_int_guc(
        c"pg_tviews.max_propagation_depth",
        c"Maximum cascade propagation iterations before aborting.",
        c"Prevents infinite loops in circular dependency chains.",
        &MAX_PROPAGATION_DEPTH_GUC,
        1,      // min
        10_000, // max
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_tviews.max_dependency_depth",
        c"Maximum pg_depend traversal depth when building the dependency graph.",
        c"Bounds how deep view-on-view hierarchies may nest before an error is raised.",
        &MAX_DEPENDENCY_DEPTH_GUC,
        1,   // min
        100, // max
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_tviews.max_queue_size",
        c"Maximum number of refresh items allowed in the transaction queue.",
        c"When exceeded, new refresh enqueues raise an error to prevent unbounded queue growth.",
        &MAX_QUEUE_SIZE_GUC,
        1,         // min
        1_000_000, // max
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_tviews.lock_escalation_threshold",
        c"Value locks a transaction takes on one relation before it locks the relation instead.",
        c"Counted per relation and per side (writes, refreshes). 0 always locks relations; \
          -1 never does, which can exhaust the shared lock table on bulk writes.",
        &LOCK_ESCALATION_THRESHOLD_GUC,
        -1,
        1_000_000,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.audit_enabled",
        c"Enable audit logging of TVIEW operations to pg_tview_audit_log.",
        c"When false, refresh/create/drop operations are not logged.",
        &AUDIT_ENABLED_GUC,
        GucContext::Suset,
        GucFlags::default(),
    );
}

/// Diagnostic switches: results are the same either way.
fn register_diagnostics() {
    GucRegistry::define_bool_guc(
        c"pg_tviews.graph_cache_enabled",
        c"Enable in-memory caching of entity dependency graphs.",
        c"When false, graphs are loaded from pg_tview_meta on every refresh.",
        &GRAPH_CACHE_ENABLED_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.table_cache_enabled",
        c"Enable in-memory caching of TVIEW catalog rows and their plans.",
        c"When false, every lookup reads pg_tview_meta.",
        &TABLE_CACHE_ENABLED_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.direct_patch_enabled",
        c"Enable the direct-patch fast path for eligible single-row UPDATEs.",
        c"When true (default), an UPDATE whose changed columns all map identity-style \
          to JSONB keys patches tv_<entity> directly, skipping the backing-view \
          recompute. When false, every change takes the recompute path. Purely a \
          performance switch: results are identical either way.",
        &DIRECT_PATCH_ENABLED_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.test_skip_ctas_intercept",
        c"TEST ONLY: make the ProcessUtility hook skip CREATE TABLE tv_* AS interception.",
        c"Simulates a session where the hook did not see the statement, to test the \
          missed-interception error. Never enable in production.",
        &TEST_SKIP_CTAS_INTERCEPT_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
}

/// Settings a session may tune for itself: performance, and its own report.
fn register_session_settings() {
    GucRegistry::define_int_guc(
        c"pg_tviews.batch_size",
        c"Maximum keys processed per statement during bulk refresh.",
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
    GucRegistry::define_int_guc(
        c"pg_tviews.report_max_tracked",
        c"Changed TVIEW rows journaled per transaction for pg_tviews_flush_and_report().",
        c"Beyond it only the entity types are kept and the report is marked truncated. \
          0 turns the journal off.",
        &REPORT_MAX_TRACKED_GUC,
        0,
        10_000_000,
        GucContext::Userset,
        GucFlags::default(),
    );
}

/// Settings read by the postmaster: defined only while the library is preloaded.
fn register_postmaster_gucs() {
    // A postmaster-level setting can only be defined while the library is preloaded;
    // a session that loads it lazily would fail with FATAL. The rebuild worker it
    // configures needs the preload anyway.
    // SAFETY: reads a postmaster-owned global set before libraries are preloaded.
    if unsafe { pgrx::pg_sys::process_shared_preload_libraries_in_progress } {
        GucRegistry::define_string_guc(
            c"pg_tviews.auto_rebuild_databases",
            c"Databases whose emptied UNLOGGED TVIEWs are rebuilt once recovery finishes.",
            c"* (the default): every database that accepts connections; a comma-separated \
              list: only those; empty: none. A launcher starts a worker per database running \
              pg_tviews_rebuild_all() at startup, after a crash restart and on promotion. \
              Requires a server restart.",
            &AUTO_REBUILD_DATABASES_GUC,
            GucContext::Postmaster,
            GucFlags::default(),
        );
    }
}

// ── Accessors ────────────────────────────────────────────────────────────

/// Maximum propagation iteration depth (default: 100)
/// Prevents infinite loops in dependency chains
#[must_use]
pub fn max_propagation_depth() -> usize {
    MAX_PROPAGATION_DEPTH_GUC.get().unsigned_abs() as usize
}

/// Maximum `pg_depend` traversal depth when building the dependency graph
/// (default: 10). Bounds view-on-view nesting.
#[must_use]
pub fn max_dependency_depth() -> usize {
    MAX_DEPENDENCY_DEPTH_GUC.get().unsigned_abs() as usize
}

/// Maximum queue size before backpressure enforcement (default: 10000)
/// Prevents unbounded queue growth during high-load scenarios
#[must_use]
pub fn max_queue_size() -> usize {
    MAX_QUEUE_SIZE_GUC.get().unsigned_abs() as usize
}

/// Value locks per relation and side before a transaction locks the relation
/// (`pg_tviews.lock_escalation_threshold`): 0 always, -1 never.
pub fn lock_escalation_threshold() -> i32 {
    LOCK_ESCALATION_THRESHOLD_GUC.get()
}

/// Check if audit logging is enabled (default: false, opt-in)
#[must_use]
pub fn audit_enabled() -> bool {
    AUDIT_ENABLED_GUC.get()
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

/// Check if the direct-patch fast path is enabled (default: true).
///
/// Checked at capture time (trigger) and apply time (flush); when false the
/// recompute path is used exclusively. A pure performance switch: the resulting
/// `tv_<entity>.data` is byte-identical either way.
#[must_use]
pub fn direct_patch_enabled() -> bool {
    DIRECT_PATCH_ENABLED_GUC.get()
}

/// TEST ONLY: true when the hook must skip CTAS interception (default: false)
#[must_use]
pub fn test_skip_ctas_intercept() -> bool {
    TEST_SKIP_CTAS_INTERCEPT_GUC.get()
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

/// Changed rows journaled per transaction (`pg_tviews.report_max_tracked`).
#[must_use]
pub fn report_max_tracked() -> usize {
    usize::try_from(REPORT_MAX_TRACKED_GUC.get()).unwrap_or(0)
}

/// The databases whose reset UNLOGGED TVIEWs are rebuilt after recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebuildDatabases {
    /// `*`: every database that accepts connections.
    All,
    /// These databases.
    Only(Vec<String>),
    /// Empty: no rebuild worker.
    None,
}

/// `pg_tviews.auto_rebuild_databases`.
#[must_use]
pub fn rebuild_databases() -> RebuildDatabases {
    let value = AUTO_REBUILD_DATABASES_GUC
        .get()
        .and_then(|s| s.to_str().ok().map(str::to_string))
        .unwrap_or_default();
    parse_rebuild_databases(&value)
}

fn parse_rebuild_databases(value: &str) -> RebuildDatabases {
    let names: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    match names.as_slice() {
        [] => RebuildDatabases::None,
        [all] if all == "*" => RebuildDatabases::All,
        _ => RebuildDatabases::Only(names),
    }
}

#[cfg(test)]
mod tests {
    use super::{RebuildDatabases, parse_rebuild_databases};

    #[test]
    fn rebuild_databases_parse() {
        assert_eq!(parse_rebuild_databases("*"), RebuildDatabases::All);
        assert_eq!(parse_rebuild_databases(" "), RebuildDatabases::None);
        assert_eq!(
            parse_rebuild_databases("app, reporting"),
            RebuildDatabases::Only(vec!["app".into(), "reporting".into()])
        );
    }
}
