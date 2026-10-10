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
//! | `pg_tviews.table_cache_enabled` | bool | true | Cache TVIEW catalog rows and plans |
//! | `pg_tviews.audit_enabled` | bool | false | Audit logging (opt-in) |
//! | `pg_tviews.log_level` | string | "info" | `debug` shows internal diagnostics as NOTICE |
//! | `pg_tviews.suspend_triggers` | bool | false | Suspend trigger-based refresh |
//! | `pg_tviews.max_queue_size` | int | 10000 | Refresh-queue backpressure limit |
//! | `pg_tviews.max_dependency_depth` | int | 10 | Max `pg_depend` traversal depth |
//! | `pg_tviews.batch_size` | int | 1000 | Max PKs per bulk-refresh statement |
//! | `pg_tviews.cache_size` | int | 10000 | Max entries per in-memory cache |
//! | `pg_tviews.direct_patch_enabled` | bool | true | Direct-patch fast path |
//! | `pg_tviews.data_gin_index` | bool | false | GIN index on `data` for new TVIEWs |
//! | `pg_tviews.fillfactor` | int | 85 | Heap fillfactor for new TVIEWs |
//! | `pg_tviews.report_max_tracked` | int | 10000 | Changed rows journaled per transaction for `pg_tviews_flush_and_report` (0 = off) |
//! | `pg_tviews.lock_escalation_threshold` | int | 64 | Value locks per relation before a transaction locks the relation (ADR 0207) |
//! | `pg_tviews.auto_rebuild_databases` | string | "" | Databases whose UNLOGGED TVIEWs are rebuilt after recovery (postmaster) |
//! | `pg_tviews.uncascaded_policy` | enum | `error` | What a new TVIEW does about base tables no cascade reaches |
//! | `pg_tviews.time_refresh` | enum | `none` | How a new TVIEW that reads the current time is brought up to date |
//!
//! `pg_tviews.uncascaded_policy` is read once, when a TVIEW is created without an
//! `uncascaded_policy` option, and stored with it: a tracked base table whose
//! writes no cascade maps to TVIEW keys refuses the create (`error`, the default),
//! is reported with a WARNING (`warn`), or makes every write to it refresh the
//! whole TVIEW at flush (`full_refresh`, which recomputes every row of the TVIEW
//! once per flush that wrote to such a table). The row trigger always uses the
//! stored value, never the writing session's.

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::PostgresGucEnum;

/// What a TVIEW does about a base table it reads whose writes no cascade maps to
/// its keys. Fixed per TVIEW when it is created.
#[derive(PostgresGucEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum UncascadedPolicy {
    /// WARNING at create time; such writes leave rows stale until a mapped table changes.
    #[name = c"warn"]
    Warn,
    /// ERROR at create time; nothing is created.
    #[name = c"error"]
    Error,
    /// NOTICE at create time; such writes refresh the whole TVIEW at flush.
    #[name = c"full_refresh"]
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

/// How a TVIEW whose definition reads the current time is brought up to date:
/// its rows change with no write.
#[derive(PostgresGucEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeRefreshSetting {
    /// Nothing declared: the TVIEW's `uncascaded_policy` refuses (`error`,
    /// `full_refresh`) or warns about it.
    #[name = c"none"]
    None,
    /// Something outside calls `pg_tviews_refresh_time_dependent()` at the
    /// boundary (`pg_cron`, the application).
    #[name = c"external"]
    External,
}

// ── GUC statics ──────────────────────────────────────────────────────────

static MAX_PROPAGATION_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(100);
static GRAPH_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static LOG_LEVEL_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"info"));
static UNION_DUPLICATE_POLICY_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"error"));
static MAX_QUEUE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static AUDIT_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static UNLOGGED_BY_DEFAULT_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TEST_SKIP_CTAS_INTERCEPT_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static SUSPEND_TRIGGERS_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static MAX_DEPENDENCY_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(10);
static BATCH_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(1_000);
static CACHE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static DIRECT_PATCH_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static DATA_GIN_INDEX_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static FILLFACTOR_GUC: GucSetting<i32> = GucSetting::<i32>::new(85);
static REPORT_MAX_TRACKED_GUC: GucSetting<i32> = GucSetting::<i32>::new(10_000);
static LOCK_ESCALATION_THRESHOLD_GUC: GucSetting<i32> = GucSetting::<i32>::new(64);
static AUTO_REBUILD_DATABASES_GUC: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(None);
static UNCASCADED_POLICY_GUC: GucSetting<UncascadedPolicy> =
    GucSetting::<UncascadedPolicy>::new(UncascadedPolicy::Error);
static TIME_REFRESH_GUC: GucSetting<TimeRefreshSetting> =
    GucSetting::<TimeRefreshSetting>::new(TimeRefreshSetting::None);

// ── GUC registration (called from _PG_init) ─────────────────────────────

/// Register all `pg_tviews.*` GUC parameters with `PostgreSQL`.
///
/// Must be called exactly once from `_PG_init()`, before any code reads
/// the GUC values.
pub fn register_gucs() {
    register_int_gucs();
    register_bool_gucs();
    register_string_gucs();
    register_enum_gucs();
    register_postmaster_gucs();

    // Every pg_tviews.* setting is defined above: refuse any other name, so a
    // typo or a setting that never existed raises instead of doing nothing.
    // SAFETY: called from _PG_init with a static, NUL-terminated prefix.
    unsafe { pgrx::pg_sys::MarkGUCPrefixReserved(c"pg_tviews".as_ptr()) };
}

fn register_int_gucs() {
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
    GucRegistry::define_int_guc(
        c"pg_tviews.fillfactor",
        c"Heap fillfactor for new TVIEW tables.",
        c"Free space kept on each page so refreshes can place the new row version on \
          the same page (HOT update). 100 packs pages fully, for append-mostly TVIEWs.",
        &FILLFACTOR_GUC,
        10,  // min (PostgreSQL's own lower bound for heap fillfactor)
        100, // max
        GucContext::Userset,
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

fn register_bool_gucs() {
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
        c"Enable in-memory caching of TVIEW catalog rows and their plans.",
        c"When false, every lookup reads pg_tview_meta.",
        &TABLE_CACHE_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.test_skip_ctas_intercept",
        c"TEST ONLY: make the ProcessUtility hook skip CREATE TABLE tv_* AS interception.",
        c"Simulates a session where the hook did not see the statement, to test the \
          missed-interception error. Never enable in production.",
        &TEST_SKIP_CTAS_INTERCEPT_GUC,
        GucContext::Userset,
        GucFlags::NO_SHOW_ALL,
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
        c"When true, new TVIEWs are created as UNLOGGED tables for better write performance. \
          A hot standby cannot read an UNLOGGED table, and promotion or a crash restart \
          empties it: turn this off for TVIEWs served from replicas.",
        &UNLOGGED_BY_DEFAULT_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );
    GucRegistry::define_bool_guc(
        c"pg_tviews.data_gin_index",
        c"Create a GIN index on the data column of new TVIEWs.",
        c"Off by default: nearly every refresh rewrites data, so an index on it makes \
          every refresh a non-HOT update. Enable per TVIEW (SET LOCAL) only when \
          top-level containment queries (data @> ...) need it.",
        &DATA_GIN_INDEX_GUC,
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
    GucRegistry::define_bool_guc(
        c"pg_tviews.direct_patch_enabled",
        c"Enable the direct-patch fast path for eligible single-row UPDATEs.",
        c"When true (default), an UPDATE whose changed columns all map identity-style \
          to JSONB keys patches tv_<entity> directly, skipping the backing-view \
          recompute. When false, every change takes the recompute path. Purely a \
          performance switch — results are identical either way.",
        &DIRECT_PATCH_ENABLED_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );
}

fn register_string_gucs() {
    GucRegistry::define_string_guc(
        c"pg_tviews.log_level",
        c"Logging verbosity for pg_tviews operations.",
        c"Set to 'debug' to show internal diagnostics (event trigger, DDL tracing) as NOTICE; otherwise they are DEBUG1 messages.",
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
}

fn register_enum_gucs() {
    GucRegistry::define_enum_guc(
        c"pg_tviews.uncascaded_policy",
        c"What a new TVIEW does about base tables whose writes no cascade reaches.",
        c"error (default): refuse the TVIEW; full_refresh: such writes refresh the whole \
          TVIEW; warn: WARNING, rows stay stale on such writes. Read once at create time \
          when the TVIEW declares no uncascaded_policy option, and stored with it.",
        &UNCASCADED_POLICY_GUC,
        GucContext::Userset,
        GucFlags::default(),
    );
    GucRegistry::define_enum_guc(
        c"pg_tviews.time_refresh",
        c"How a new TVIEW that reads the current time is brought up to date.",
        c"none (default): its uncascaded_policy refuses it (error, full_refresh) or warns \
          (warn); external: pg_tviews_refresh_time_dependent() is called at the boundary. \
          Read at create time when the TVIEW declares no time_refresh option, and stored \
          with a TVIEW that reads the time.",
        &TIME_REFRESH_GUC,
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
            c"Comma-separated database names. For each one a background worker runs \
              pg_tviews_rebuild_all() at startup, after a crash restart and on promotion. \
              Empty (the default) starts no worker. Requires a server restart.",
            &AUTO_REBUILD_DATABASES_GUC,
            GucContext::Postmaster,
            GucFlags::default(),
        );
    }
}

// ── Public accessors (same signatures as the old const fns) ──────────────

/// Maximum propagation iteration depth (default: 100)
/// Prevents infinite loops in dependency chains
#[must_use]
pub fn max_propagation_depth() -> usize {
    MAX_PROPAGATION_DEPTH_GUC.get().unsigned_abs() as usize
}

/// Database names listed in `pg_tviews.auto_rebuild_databases`.
#[must_use]
pub fn auto_rebuild_databases() -> Vec<String> {
    AUTO_REBUILD_DATABASES_GUC
        .get()
        .and_then(|s| s.to_str().ok().map(str::to_string))
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Value locks per relation and side before a transaction locks the relation
/// (`pg_tviews.lock_escalation_threshold`): 0 always, -1 never.
pub fn lock_escalation_threshold() -> i32 {
    LOCK_ESCALATION_THRESHOLD_GUC.get()
}

/// Changed rows journaled per transaction (`pg_tviews.report_max_tracked`).
#[must_use]
pub fn report_max_tracked() -> usize {
    usize::try_from(REPORT_MAX_TRACKED_GUC.get()).unwrap_or(0)
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

/// TEST ONLY: true when the hook must skip CTAS interception (default: false)
#[must_use]
pub fn test_skip_ctas_intercept() -> bool {
    TEST_SKIP_CTAS_INTERCEPT_GUC.get()
}

/// Check if TVIEWs should be created as UNLOGGED by default (default: true)
#[must_use]
pub fn unlogged_by_default() -> bool {
    UNLOGGED_BY_DEFAULT_GUC.get()
}

/// Whether new TVIEWs get a GIN index on `data` (default: false)
#[must_use]
pub fn data_gin_index() -> bool {
    DATA_GIN_INDEX_GUC.get()
}

/// Heap fillfactor for new TVIEW tables (default: 85)
#[must_use]
pub fn fillfactor() -> i32 {
    FILLFACTOR_GUC.get()
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

/// Check if the direct-patch fast path is enabled (default: true).
///
/// Checked at capture time (trigger) and apply time (flush); when false the
/// recompute path is used exclusively. A pure performance switch — the resulting
/// `tv_<entity>.data` is byte-identical either way.
#[must_use]
pub fn direct_patch_enabled() -> bool {
    DIRECT_PATCH_ENABLED_GUC.get()
}

/// `pg_tviews.uncascaded_policy` (default `error`): read when a TVIEW is created
/// without an `uncascaded_policy` option.
pub fn uncascaded_policy() -> UncascadedPolicy {
    UNCASCADED_POLICY_GUC.get()
}

/// `pg_tviews.time_refresh` (default `none`): read when a TVIEW is created without
/// a `time_refresh` option.
pub fn time_refresh() -> TimeRefreshSetting {
    TIME_REFRESH_GUC.get()
}
