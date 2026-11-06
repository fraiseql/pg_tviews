//! Runtime settings (`pg_tviews.*`), and the policy names TVIEW options use.
//!
//! What a TVIEW is comes from its definition and its options, never from a
//! setting (ADR 0220): settings only tune how a session works. A setting that
//! decides whether a write or a creation succeeds can only be changed by a
//! superuser, so every session agrees on it. Limits nobody decides per database
//! (the flush's steps, view nesting, the queue, batches, caches, the report's
//! rows) are constants of the module they bound, each named by its error.
//!
//! | Parameter | Type | Default | Context | Description |
//! |-----------|------|---------|---------|-------------|
//! | `pg_tviews.publish_changes` | enum | off | superuser | Publish changed TVIEW rows at commit on `tviews_changes`: `off`, `types`, `rows` (ADR 0231) |
//! | `pg_tviews.max_refresh_rows` | int | 0 | superuser | TVIEW rows one statement's refresh may write (0 = no limit, ADR 0233) |
//! | `pg_tviews.lock_escalation_threshold` | int | 64 | superuser | Value locks per relation before a transaction locks the relation (ADR 0207); 0 always |
//! | `pg_tviews.audit_enabled` | bool | false | superuser | Audit logging (opt-in) |
//! | `pg_tviews.graph_cache_enabled` | bool | true | superuser, hidden | Cache dependency graphs |
//! | `pg_tviews.table_cache_enabled` | bool | true | superuser, hidden | Cache TVIEW catalog rows and plans |
//! | `pg_tviews.direct_patch_enabled` | bool | true | superuser, hidden | Direct-patch fast path |
//! | `pg_tviews.max_queue_size` | int | 1000000 | superuser, hidden | Refresh keys a transaction may queue |
//! | `pg_tviews.test_skip_ctas_intercept` | bool | false | superuser, hidden | Test builds only (`test-hooks`) |
//! | `pg_tviews.test_diag_21000` | bool | false | superuser, hidden | Test builds only: log what a 21000 upsert conflicted with (#278) |
//! | `pg_tviews.test_duplicate_source` | bool | false | superuser, hidden | Test builds only: every refresh source returns each row twice |
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

static MAX_QUEUE_SIZE_GUC: GucSetting<i32> = GucSetting::<i32>::new(1_000_000);
static MAX_REFRESH_ROWS_GUC: GucSetting<i32> = GucSetting::<i32>::new(0);
static PUBLISH_CHANGES_GUC: GucSetting<PublishChanges> =
    GucSetting::<PublishChanges>::new(PublishChanges::Off);

/// `pg_tviews.publish_changes`.
#[derive(pgrx::PostgresGucEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PublishChanges {
    #[name = c"off"]
    Off,
    #[name = c"types"]
    Types,
    #[name = c"rows"]
    Rows,
}
static LOCK_ESCALATION_THRESHOLD_GUC: GucSetting<i32> = GucSetting::<i32>::new(64);
static AUDIT_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
static GRAPH_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static DIRECT_PATCH_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
#[cfg(feature = "test-hooks")]
static TEST_SKIP_CTAS_INTERCEPT_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
#[cfg(feature = "test-hooks")]
static TEST_DIAG_21000_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
#[cfg(feature = "test-hooks")]
static TEST_DUPLICATE_SOURCE_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
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
    register_postmaster_gucs();

    // Every pg_tviews.* setting is defined above: refuse any other name, so a
    // typo or a removed setting raises instead of doing nothing.
    // SAFETY: called from _PG_init with a static, NUL-terminated prefix.
    unsafe { pgrx::pg_sys::MarkGUCPrefixReserved(c"pg_tviews".as_ptr()) };
}

/// Settings that decide whether a write or a creation succeeds: the same for
/// every session, so only a superuser changes them.
fn register_limits() {
    GucRegistry::define_enum_guc(
        c"pg_tviews.publish_changes",
        c"Publish the TVIEW rows each transaction changed, at commit, on channel tviews_changes.",
        c"off: nothing. types: the changed GraphQL types. rows: the changed rows' ids, by type \
          (types past a cap). See ADR 0231.",
        &PUBLISH_CHANGES_GUC,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_tviews.max_refresh_rows",
        c"Maximum TVIEW rows one statement's refresh may write (0: no limit).",
        c"A statement whose refresh would write more fails with 54000 before writing them. \
          Recomputed keys, patched rows, fan-out children and a full refresh's rows are \
          counted; explicit rebuilds are not.",
        &MAX_REFRESH_ROWS_GUC,
        0,
        i32::MAX,
        GucContext::Suset,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"pg_tviews.lock_escalation_threshold",
        c"Value locks a transaction takes on one relation before it locks the relation instead.",
        c"Counted per relation and per side (writes, refreshes). 0 always locks relations.",
        &LOCK_ESCALATION_THRESHOLD_GUC,
        0,
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
    // Kept as a switch only so the queue-full error can be tested end to end.
    GucRegistry::define_int_guc(
        c"pg_tviews.max_queue_size",
        c"Refresh keys a transaction may queue before its next write fails.",
        c"A memory bound, not a policy: raise it only for a bulk change that queues more \
          than a million keys, or make the change after pg_tviews_suspend_triggers().",
        &MAX_QUEUE_SIZE_GUC,
        1,
        i32::MAX,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    #[cfg(feature = "test-hooks")]
    GucRegistry::define_bool_guc(
        c"pg_tviews.test_skip_ctas_intercept",
        c"TEST ONLY: make the ProcessUtility hook skip CREATE TABLE tv_* AS interception.",
        c"Simulates a session where the hook did not see the statement, to test the \
          missed-interception error. Never enable in production.",
        &TEST_SKIP_CTAS_INTERCEPT_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    #[cfg(feature = "test-hooks")]
    GucRegistry::define_bool_guc(
        c"pg_tviews.test_diag_21000",
        c"TEST ONLY: log what a refresh upsert failing with 21000 conflicted with.",
        c"Runs each refresh upsert in a subtransaction and, on 21000, logs its keys, the \
          duplicate source keys and the conflicting rows before re-raising (#278). Never \
          enable in production.",
        &TEST_DIAG_21000_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
    );
    #[cfg(feature = "test-hooks")]
    GucRegistry::define_bool_guc(
        c"pg_tviews.test_duplicate_source",
        c"TEST ONLY: make every refresh source return each row twice.",
        c"Forces the 21000 an upsert raises on a key it meets twice, to test \
          pg_tviews.test_diag_21000. Never enable in production.",
        &TEST_DUPLICATE_SOURCE_GUC,
        GucContext::Suset,
        GucFlags::NO_SHOW_ALL,
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

/// Refresh keys a transaction may queue (`pg_tviews.max_queue_size`).
#[must_use]
pub fn max_queue_size() -> usize {
    MAX_QUEUE_SIZE_GUC.get().unsigned_abs() as usize
}

/// What a transaction publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publish {
    /// Changed rows, by id.
    Rows,
    /// Changed types only.
    Types,
}

/// What commit publishes (`pg_tviews.publish_changes`); `None` when off.
#[must_use]
pub fn publish_changes() -> Option<Publish> {
    match PUBLISH_CHANGES_GUC.get() {
        PublishChanges::Off => None,
        PublishChanges::Types => Some(Publish::Types),
        PublishChanges::Rows => Some(Publish::Rows),
    }
}

/// TVIEW rows one statement's refresh may write (`pg_tviews.max_refresh_rows`);
/// `None` when unlimited.
#[must_use]
pub fn max_refresh_rows() -> Option<u64> {
    match MAX_REFRESH_ROWS_GUC.get() {
        0 => None,
        max => Some(u64::from(max.unsigned_abs())),
    }
}

/// Value locks per relation and side before a transaction locks the relation
/// (`pg_tviews.lock_escalation_threshold`): 0 always.
pub fn lock_escalation_threshold() -> usize {
    LOCK_ESCALATION_THRESHOLD_GUC.get().unsigned_abs() as usize
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
#[cfg(feature = "test-hooks")]
#[must_use]
pub fn test_skip_ctas_intercept() -> bool {
    TEST_SKIP_CTAS_INTERCEPT_GUC.get()
}

/// TEST ONLY: whether a refresh upsert logs what a 21000 conflicted with.
#[cfg(feature = "test-hooks")]
#[must_use]
pub fn test_diag_21000() -> bool {
    TEST_DIAG_21000_GUC.get()
}

/// TEST ONLY: whether every refresh source returns each row twice.
#[cfg(feature = "test-hooks")]
#[must_use]
pub fn test_duplicate_source() -> bool {
    TEST_DUPLICATE_SOURCE_GUC.get()
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
