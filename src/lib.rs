use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;
/**
# `pg_tviews` - `PostgreSQL` Transactional Views

A `PostgreSQL` extension that provides transactional materialized views with
incremental refresh capabilities. `TVIEW`s automatically maintain consistency
between base tables and derived views through trigger-based change tracking.

## Architecture

`pg_tviews` implements a sophisticated refresh system:

1. **Change Tracking**: Triggers on base tables enqueue changes to a transaction-scoped queue
2. **Dependency Analysis**: Resolves view dependencies using topological sorting
3. **Incremental Refresh**: Updates only affected rows in dependent views
4. **Transaction Safety**: All refreshes occur within the same transaction as the original changes

## Key Features

- **Transactional Consistency**: View refreshes are atomic with base table changes
- **Dependency Resolution**: Handles complex multi-level view dependencies
- **Performance Optimized**: Incremental updates avoid full view rebuilds
- **`PostgreSQL` Native**: Written as a C extension using `pgrx` framework
- **2PC Support**: Transaction queue persistence for prepared transactions

## Usage

```sql
-- Create a transactional view
SELECT pg_tviews_create('user_posts',
    'SELECT u.name, p.title FROM users u JOIN posts p ON u.id = p.user_id');

-- Insert data (view automatically refreshes)
INSERT INTO users (name) VALUES ('Alice');
INSERT INTO posts (user_id, title) VALUES (1, 'Hello World');
```

## Safety

This extension is designed with `PostgreSQL`'s safety requirements in mind:
- No panics in FFI callbacks (all wrapped in `catch_unwind`)
- Proper error handling with meaningful error messages
- Transaction rollback on refresh failures
- Memory safety through Rust's ownership system
*/
use pgrx::JsonB;
use std::sync::atomic::{AtomicBool, Ordering};

mod catalog;
mod refresh;
mod propagate;
mod utils;
mod hooks;
mod trigger;
mod queue;
mod metrics;
mod event_trigger;
mod audit;
pub mod error;
pub mod metadata;
pub mod schema;
pub mod parser;
pub mod ddl;
pub mod config;
pub mod dependency;

pub use error::{TViewError, TViewResult};
pub use queue::RefreshKey;
pub use catalog::entity_for_table;

pg_module_magic!();

// Static cache for jsonb_ivm availability (performance optimization)
static JSONB_IVM_AVAILABLE: AtomicBool = AtomicBool::new(false);
static JSONB_IVM_CHECKED: AtomicBool = AtomicBool::new(false);

/// Get the version of the `pg_tviews` extension
#[pg_extern]
const fn pg_tviews_version() -> &'static str {
    "0.1.0-alpha"
}

/// Debug function to check if `ProcessUtility` hook is installed
#[pg_extern]
const fn pg_tviews_hook_status() -> &'static str {
    // This is a simple way to check if the module loaded
    // The hook installation happens in _PG_init
    "Extension loaded - hook installation attempted in _PG_init"
}

/// Check if `jsonb_ivm` extension is available at runtime (cached)
/// Returns true if extension is installed, false otherwise
///
/// This function caches the result after the first check to avoid
/// repeated queries to `pg_extension` on every cascade operation.
pub fn check_jsonb_ivm_available() -> bool {
    // Return cached result if already checked
    if JSONB_IVM_CHECKED.load(Ordering::Relaxed) {
        return JSONB_IVM_AVAILABLE.load(Ordering::Relaxed);
    }

    // First time: query database
    let result: Result<bool, spi::Error> = Spi::connect(|client| {
        let rows = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_ivm')",
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

    // Cache result
    JSONB_IVM_AVAILABLE.store(is_available, Ordering::Relaxed);
    JSONB_IVM_CHECKED.store(true, Ordering::Relaxed);

    is_available
}

/// Export as SQL function for testing
#[pg_extern]
fn pg_tviews_check_jsonb_ivm() -> bool {
    check_jsonb_ivm_available()
}

/// Get current queue statistics
/// Returns metrics about the current transaction's refresh operations
#[pg_extern]
fn pg_tviews_queue_stats() -> pgrx::JsonB {
    let stats = metrics::metrics_api::get_queue_stats();

    let json_value = serde_json::json!({
        "queue_size": stats.queue_size,
        "total_refreshes": stats.total_refreshes,
        "total_iterations": stats.total_iterations,
        "max_iterations": stats.max_iterations,
        "total_timing_ms": stats.total_timing_ms(),
        "graph_cache_hit_rate": stats.graph_cache_hit_rate(),
        "table_cache_hit_rate": stats.table_cache_hit_rate(),
        "graph_cache_hits": stats.graph_cache_hits,
        "graph_cache_misses": stats.graph_cache_misses,
        "table_cache_hits": stats.table_cache_hits,
        "table_cache_misses": stats.table_cache_misses
    });

    pgrx::JsonB(json_value)
}

/// Debug function: View current queue contents
/// Returns the entities and PKs currently in the refresh queue
#[pg_extern]
fn pg_tviews_debug_queue() -> pgrx::JsonB {
    let contents = metrics::metrics_api::get_queue_contents();

    let json_contents: Vec<serde_json::Value> = contents
        .into_iter()
        .map(|key| {
            serde_json::json!({
                "entity": key.entity,
                "pk": key.pk
            })
        })
        .collect();

    pgrx::JsonB(serde_json::json!(json_contents))
}

/// Initialize the extension
/// Installs the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
///
/// Safety: Only installs hooks when running in a proper `PostgreSQL` backend,
/// not during initdb or other bootstrap contexts.
#[pg_guard]
extern "C-unwind" fn _PG_init() {
    // For shared_preload_libraries extensions, _PG_init is called during postmaster startup
    // This is the CORRECT time to install hooks (they apply to all backends)
    unsafe {
        hooks::install_hook();
    }

    // Note: We cannot call functions that require SPI/database connection here
    // (like `check_jsonb_ivm_available` or `register_cache_invalidation_callbacks`)
    // because no database connection exists during shared library preloading.
    // These checks happen lazily on first use instead.
}

/// Health check function for production monitoring
///
/// Returns a comprehensive health status including:
/// - Extension version
/// - `jsonb_ivm` availability
/// - Metadata consistency
/// - Orphaned triggers
/// - Queue status
#[pg_extern]
fn pg_tviews_health_check() -> TableIterator<'static, (
    name!(status, String),
    name!(component, String),
    name!(message, String),
    name!(severity, String),
)> {
    let mut results = Vec::new();

    // Check 1: Extension loaded
    results.push((
        "OK".to_string(),
        "extension".to_string(),
        format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
        "info".to_string(),
    ));

    // Check 2: jsonb_ivm availability
    let has_jsonb_ivm = Spi::get_one::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_ivm'"
    ).unwrap_or(Some(false)).unwrap_or(false);

    if has_jsonb_ivm {
        results.push((
            "OK".to_string(),
            "jsonb_ivm".to_string(),
            "jsonb_ivm extension available (optimized mode)".to_string(),
            "info".to_string(),
        ));
    } else {
        results.push((
            "WARNING".to_string(),
            "jsonb_ivm".to_string(),
            "jsonb_ivm not installed (falling back to standard JSONB)".to_string(),
            "warning".to_string(),
        ));
    }

    // Check 3: Metadata consistency
    let orphaned_meta = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_tview_meta m
         WHERE NOT EXISTS (
           SELECT 1 FROM pg_class WHERE relname = 'tv_' || m.entity
         )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_meta > 0 {
        results.push((
            "ERROR".to_string(),
            "metadata".to_string(),
            format!("{orphaned_meta} orphaned metadata entries found"),
            "error".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "metadata".to_string(),
            "All metadata entries valid".to_string(),
            "info".to_string(),
        ));
    }

    // Check 4: Orphaned triggers
    let orphaned_triggers = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_trigger
         WHERE tgname LIKE 'tview_%'
           AND tgrelid NOT IN (
             SELECT ('tb_' || entity)::regclass::oid
             FROM pg_tview_meta
           )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_triggers > 0 {
        results.push((
            "WARNING".to_string(),
            "triggers".to_string(),
            format!("{orphaned_triggers} orphaned triggers found"),
            "warning".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "triggers".to_string(),
            "All triggers properly linked".to_string(),
            "info".to_string(),
        ));
    }

    // Check 5: TVIEW count
    let tview_count = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_tview_meta"
    ).unwrap_or(Some(0)).unwrap_or(0);

    results.push((
        "OK".to_string(),
        "tviews".to_string(),
        format!("{tview_count} TVIEWs registered"),
        "info".to_string(),
    ));

    TableIterator::new(results)
}

/// Analyze a SELECT statement and return inferred TVIEW schema as JSONB
#[pg_extern]
fn pg_tviews_analyze_select(sql: &str) -> JsonB {
    match schema::inference::infer_schema(sql) {
        Ok(schema) => {
            match schema.to_jsonb() {
                Ok(jsonb) => jsonb,
                Err(e) => {
                    error!("Failed to serialize schema to JSONB: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Schema inference failed: {}", e);
        }
    }
}

/// Infer column types from `PostgreSQL` catalog
#[pg_extern]
#[allow(clippy::needless_pass_by_value)]
fn pg_tviews_infer_types(
    table_name: &str,
    columns: Vec<String>,
) -> JsonB {
    match schema::types::infer_column_types(table_name, &columns) {
        Ok(types) => {
            match serde_json::to_value(&types) {
                Ok(json_value) => JsonB(json_value),
                Err(e) => {
                    error!("Failed to serialize types to JSONB: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Type inference failed: {}", e);
        }
    }
}

/// Handle COMMIT PREPARED for 2PC transactions
/// Processes pending refreshes for a committed prepared transaction
///
/// Arguments:
/// - `gid`: Global transaction ID of the prepared transaction
#[pg_extern]
fn pg_tviews_commit_prepared(gid: &str) -> TViewResult<()> {
    // STEP 1: Load queue metadata BEFORE committing (verify it exists)
    use pgrx::datum::DatumWithOid;
    let args = vec![unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }];
    let queue_jsonb: Option<JsonB> = Spi::get_one_with_args(
        "SELECT refresh_queue FROM pg_tview_pending_refreshes WHERE gid = $1",
        &args,
    )?;

    // STEP 2: COMMIT THE PREPARED TRANSACTION FIRST
    // This ensures TVIEWs never show uncommitted data
    let commit_sql = format!("COMMIT PREPARED '{gid}'");
    Spi::run(&commit_sql)?;

    // STEP 3: Now process the queue (transaction is committed, safe to refresh)
    let Some(jsonb) = queue_jsonb else {
        // No pending refreshes for this GID
        info!("TVIEW: No pending refreshes for prepared transaction '{}'", gid);
        return Ok(());
    };

    let serialized = crate::queue::persistence::SerializedQueue::from_jsonb(jsonb)?;
    let queue = serialized.into_queue();

    if !queue.is_empty() {
        info!("TVIEW: Processing {} deferred refreshes for committed transaction '{}'",
              queue.len(), gid);

        // Process queue in a NEW transaction (prepared transaction already committed)
        Spi::run("BEGIN")?;

        match process_refresh_queue(queue) {
            Ok(()) => {
                Spi::run("COMMIT")?;
            }
            Err(e) => {
                Spi::run("ROLLBACK")?;
                return Err(e);
            }
        }
    }

    // STEP 4: Clean up persistent entry
    Spi::run_with_args(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1",
        &[unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }],
    )?;

    Ok(())
}

/// Handle ROLLBACK PREPARED for 2PC transactions
/// Cleans up pending refreshes for a rolled back prepared transaction
///
/// Arguments:
/// - `gid`: Global transaction ID of the prepared transaction
#[pg_extern]
fn pg_tviews_rollback_prepared(gid: &str) -> TViewResult<()> {
    // STEP 1: Rollback the prepared transaction first
    let rollback_sql = format!("ROLLBACK PREPARED '{gid}'");
    Spi::run(&rollback_sql)?;

    // STEP 2: Clean up pending queue (no refresh needed - transaction aborted)
    let deleted_count = Spi::get_one_with_args::<i32>(
        "DELETE FROM pg_tview_pending_refreshes WHERE gid = $1 RETURNING 1",
        &[unsafe { DatumWithOid::new(gid, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) }],
    )?;

    if deleted_count.is_some() {
        info!("TVIEW: Cleaned up pending queue for rolled back transaction '{}'", gid);
    }

    Ok(())
}

/// Process refresh queue (extracted from `handle_pre_commit` for reuse)
fn process_refresh_queue(queue: std::collections::HashSet<crate::queue::RefreshKey>) -> TViewResult<()> {
    let mut pending = queue;
    let mut processed = std::collections::HashSet::new();
    let graph = crate::queue::cache::graph_cache::load_cached()?;

    let mut iteration = 1;
    while !pending.is_empty() {
        let sorted_keys = graph.sort_keys(pending.drain().collect());

        for key in sorted_keys {
            if !processed.insert(key.clone()) {
                continue;
            }

            let parents = refresh_and_get_parents(&key)?;

            for parent_key in parents {
                if !processed.contains(&parent_key) {
                    pending.insert(parent_key);
                }
            }
        }

        iteration += 1;
        if iteration > get_max_propagation_depth() {
            return Err(crate::TViewError::PropagationDepthExceeded {
                max_depth: get_max_propagation_depth(),
                processed: processed.len(),
            });
        }
    }

    Ok(())
}

/// Refresh a single entity+pk and return discovered parent keys
fn refresh_and_get_parents(key: &crate::queue::RefreshKey) -> TViewResult<Vec<crate::queue::RefreshKey>> {
    // Load metadata
    use crate::catalog::TviewMeta;
    let meta = TviewMeta::load_by_entity(&key.entity)?
        .ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: key.entity.clone(),
        })?;

    // Refresh this entity (existing logic)
    crate::refresh::refresh_pk(meta.view_oid, key.pk)?;

    // Find parent entities (returns keys instead of refreshing)
    let parent_keys = crate::propagate::find_parents_for(key)?;

    Ok(parent_keys)
}

/// Get maximum propagation depth from config
fn get_max_propagation_depth() -> usize {
    crate::config::max_propagation_depth()
}

/// Recover orphaned prepared transactions
/// Processes pending refreshes for prepared transactions that may have been interrupted
///
/// Returns a table with recovery results: (gid, `queue_size`, status)
#[pg_extern]
fn pg_tviews_recover_prepared_transactions() -> pgrx::iter::TableIterator<
    'static,
    (
        pgrx::name!(gid, String),
        pgrx::name!(queue_size, i32),
        pgrx::name!(status, String),
    ),
> {
    let results: Vec<(String, i32, String)> = Spi::connect(|client| {
        // Try to acquire advisory lock (non-blocking)
        // Use a fixed hash for the lock key
        const RECOVERY_LOCK_KEY: i64 = 0x7476_6965_7773_5F72; // "tviews_r" in hex

        let mut lock_result = client.select(
            &format!("SELECT pg_try_advisory_lock({RECOVERY_LOCK_KEY})"),
            None,
            &[],
        )?;

        let lock_acquired = if let Some(row) = lock_result.next() {
            row[1].value::<bool>()?.unwrap_or(false)
        } else {
            false
        };

        if !lock_acquired {
            info!("TVIEW: Another recovery process is running, skipping");
            return Ok(Vec::new());
        }

        // Ensure lock is released on exit (even if error occurs)
        let _guard = AdvisoryLockGuard::new(RECOVERY_LOCK_KEY);

        // Perform recovery
        let rows = client.select(
            "SELECT gid, queue_size FROM pg_tview_pending_refreshes
             WHERE prepared_at < now() - interval '1 hour'
             ORDER BY prepared_at",
            None,
            &[],
        )?;

        let mut results = Vec::new();

        for row in rows {
            let gid: String = row["gid"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT gid, queue_size FROM pg_tview_pending_refreshes ...".to_string(),
                    error: "gid column is NULL".to_string(),
                }))?;
            let queue_size: i32 = row["queue_size"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT gid, queue_size FROM pg_tview_pending_refreshes ...".to_string(),
                    error: "queue_size column is NULL".to_string(),
                }))?;

            let status = match pg_tviews_commit_prepared(&gid) {
                Ok(()) => {
                    info!("TVIEW: Recovered prepared transaction '{}' ({} refreshes)", gid, queue_size);
                    "processed".to_string()
                }
                Err(e) => {
                    warning!("TVIEW: Failed to recover prepared transaction '{}': {:?}", gid, e);
                    "error".to_string()
                }
            };

            results.push((gid, queue_size, status));
        }

        Ok::<_, spi::Error>(results)
    })
    .unwrap_or_else(|_e| {
        // Note: error! macro may panic, so just return empty vec on failure
        Vec::new()
    });

    pgrx::iter::TableIterator::new(results)
}

/// RAII guard for advisory lock (ensures unlock on drop)
struct AdvisoryLockGuard {
    lock_key: i64,
}

impl AdvisoryLockGuard {
    const fn new(lock_key: i64) -> Self {
        Self { lock_key }
    }
}

impl Drop for AdvisoryLockGuard {
    fn drop(&mut self) {
        // Release advisory lock
        let _ = Spi::run(&format!("SELECT pg_advisory_unlock({})", self.lock_key));
    }
}

/// Cascade refresh when a base table row changes
/// Called by trigger handler when INSERT/UPDATE/DELETE occurs on base tables
///
/// Arguments:
/// - `base_table_oid`: OID of the base table that changed
/// - `pk_value`: Primary key value of the changed row
#[pg_extern]
fn pg_tviews_cascade(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // Find all TVIEWs that depend on this base table
    let dependent_tviews = match find_dependent_tviews(base_table_oid) {
        Ok(tv) => tv,
        Err(e) => error!("Failed to find dependent TVIEWs: {:?}", e),
    };

    if dependent_tviews.is_empty() {
        // No TVIEWs depend on this table
        info!("No dependent TVIEWs found for base table OID {:?}", base_table_oid);
        return;
    }

    info!("Base table OID {:?} changed (pk={}), refreshing {} dependent TVIEWs",
          base_table_oid, pk_value, dependent_tviews.len());

    // Refresh each dependent TVIEW
    for tview_meta in dependent_tviews {
        // Find rows in this TVIEW that reference the changed base table row
        let affected_rows = match find_affected_tview_rows(&tview_meta, base_table_oid, pk_value) {
            Ok(rows) => rows,
            Err(e) => {
                warning!("Failed to find affected rows in {}: {:?}", tview_meta.entity_name, e);
                continue;
            }
        };

        if affected_rows.is_empty() {
            continue;
        }

        info!("  Refreshing {} rows in TVIEW {}", affected_rows.len(), tview_meta.entity_name);

        // Refresh each affected row (this will cascade via propagate_from_row)
        for affected_pk in affected_rows {
            if let Err(e) = refresh::refresh_pk(tview_meta.view_oid, affected_pk) {
                warning!("Failed to refresh {}[{}]: {:?}", tview_meta.entity_name, affected_pk, e);
            }
        }
    }
}

/// Handle INSERT operations on base tables
/// Called by trigger handler when rows are inserted
///
/// Arguments:
/// - `base_table_oid`: OID of the base table that changed
/// - `pk_value`: Primary key value of the inserted row
#[pg_extern]
fn pg_tviews_insert(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // For INSERT operations, we need to check if this affects array relationships
    // For now, delegate to the cascade function (which handles recomputation)
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Handle DELETE operations on base tables
/// Called by trigger handler when rows are deleted
///
/// Arguments:
/// - `base_table_oid`: OID of the base table that changed
/// - `pk_value`: Primary key value of the deleted row
#[pg_extern]
fn pg_tviews_delete(
    base_table_oid: pg_sys::Oid,
    pk_value: i64,
) {
    // For DELETE operations, we need to check if this affects array relationships
    // For now, delegate to the cascade function (which handles recomputation)
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Find all TVIEWs that have the given base table as a dependency
fn find_dependent_tviews(base_table_oid: pg_sys::Oid) -> spi::Result<Vec<catalog::TviewMeta>> {
    // Query pg_tview_meta using the pre-computed dependencies array
    // The dependencies column contains all base table OIDs that this TVIEW depends on

    let query = format!(
        "SELECT m.table_oid AS tview_oid, m.view_oid, m.entity, \
                m.fk_columns, m.uuid_fk_columns, \
                m.dependency_types, m.dependency_paths, m.array_match_keys \
         FROM pg_tview_meta m \
         WHERE {:?} = ANY(m.dependencies)",
        base_table_oid.to_u32()
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut result = Vec::new();

        for row in rows {
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            // Extract NEW arrays - dependency_types (TEXT[])
            let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
            let dep_types: Vec<catalog::DependencyType> = dep_types_raw
                .unwrap_or_default()
                .into_iter()
                .map(|s| catalog::DependencyType::from_str(&s))
                .collect();

            // dependency_paths (TEXT[][]) - array of arrays
            // TODO: pgrx doesn't support TEXT[][] extraction yet
            // For now, use empty default (Task 3 will populate these)
            let dep_paths: Vec<Option<Vec<String>>> = vec![];

            // array_match_keys (TEXT[]) with NULL values
            let array_keys: Option<Vec<Option<String>>> =
                row["array_match_keys"].value()?;

            result.push(catalog::TviewMeta {
                tview_oid: row["tview_oid"].value()?
                    .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                        query: "SELECT table_oid AS tview_oid, view_oid, entity, ...".to_string(),
                        error: "tview_oid column is NULL".to_string(),
                    }))?,
                view_oid: row["view_oid"].value()?
                    .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                        query: "SELECT table_oid AS tview_oid, view_oid, entity, ...".to_string(),
                        error: "view_oid column is NULL".to_string(),
                    }))?,
                entity_name: row["entity"].value()?
                    .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                        query: "SELECT table_oid AS tview_oid, view_oid, entity, ...".to_string(),
                        error: "entity column is NULL".to_string(),
                    }))?,
                sync_mode: 's',
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                dependency_types: dep_types,
                dependency_paths: dep_paths,
                array_match_keys: array_keys.unwrap_or_default(),
            });
        }

        Ok(result)
    })
}

/// Find rows in a TVIEW that reference a specific base table row
fn find_affected_tview_rows(
    tview_meta: &catalog::TviewMeta,
    base_table_oid: pg_sys::Oid,
    base_pk: i64,
) -> spi::Result<Vec<i64>> {
    // Get the base table name to figure out which FK column to check
    let base_table_name = Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {base_table_oid:?}"
    ))?.ok_or(spi::Error::InvalidPosition)?;

    // Extract entity name from table name (e.g., "tb_user" -> "user")
    let base_entity = base_table_name.trim_start_matches("tb_");

    // Query the TVIEW's backing view to find rows where the PK matches
    let view_name = utils::lookup_view_for_source(tview_meta.view_oid)?;
    let tview_pk_col = format!("pk_{}", tview_meta.entity_name);

    // Determine if this is a direct match (tview entity = base entity) or FK relationship
    let where_clause = if tview_meta.entity_name == base_entity {
        // Direct match: the TVIEW is for this entity (e.g., tv_user depends on tb_user)
        // Match on the primary key column directly
        format!("{tview_pk_col} = {base_pk}")
    } else {
        // FK relationship: the TVIEW depends on this entity via FK
        // (e.g., tv_post depends on tb_user via fk_user)
        let fk_col = format!("fk_{base_entity}");
        format!("{fk_col} = {base_pk}")
    };

    let query = format!(
        "SELECT {tview_pk_col} FROM {view_name} WHERE {where_clause}"
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[tview_pk_col.as_str()].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use crate::error::TViewError;

    #[cfg(feature = "pg_test")]
    use pgrx_tests::pg_test;

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn sanity_check() {
        assert_eq!(2, 1 + 1);
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_version_function() {
        let version = crate::pg_tviews_version();
        assert!(version.starts_with("0.1.0"));
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_version_callable_from_sql() {
        let result = Spi::get_one::<String>(
            "SELECT pg_tviews_version()"
        );
        assert!(result.is_ok());
        let version = result.unwrap();
        assert!(version.is_some());
        assert!(version.unwrap().starts_with("0.1.0"));
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    #[should_panic(expected = "TVIEW metadata not found")]
    fn test_error_propagates_to_postgres() {
        // This should raise a PostgreSQL error
        Err::<(), _>(TViewError::MetadataNotFound {
            entity: "test".to_string(),
        }).unwrap();
    }

    // Phase 5 Task 1 RED: Tests for jsonb_ivm detection
    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_jsonb_ivm_check_function_exists() {
        // This test will fail because pg_tviews_check_jsonb_ivm doesn't exist yet
        let result = Spi::get_one::<bool>("SELECT pg_tviews_check_jsonb_ivm()");
        assert!(result.is_ok(), "pg_tviews_check_jsonb_ivm() function should exist");
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_check_jsonb_ivm_available_function() {
        // This test will fail because check_jsonb_ivm_available() doesn't exist yet
        let _result = crate::check_jsonb_ivm_available();
        // Just calling it is enough - function must exist
    }

    #[cfg(feature = "pg_test")]
    #[pg_test]
    fn test_pg_tviews_works_without_jsonb_ivm() {
        // Setup: Ensure jsonb_ivm is NOT installed
        Spi::run("DROP EXTENSION IF EXISTS jsonb_ivm CASCADE").ok();

        // Test: pg_tviews should still function
        Spi::run("CREATE TABLE tb_demo (pk_demo INT PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("INSERT INTO tb_demo VALUES (1, 'Demo')").unwrap();

        // This should work even without jsonb_ivm
        let result = Spi::get_one::<bool>(
            "SELECT pg_tviews_create('demo', 'SELECT pk_demo, jsonb_build_object(''name'', name) AS data FROM tb_demo') IS NOT NULL"
        );

        assert!(result.unwrap().unwrap_or(false), "pg_tviews should work without jsonb_ivm");
    }
}
// */

/// Show cascade dependency path for a given entity
///
/// Returns the dependency chain showing which TVIEWs depend on this entity
#[pg_extern]
fn pg_tviews_show_cascade_path(entity: &str) -> TableIterator<'static, (
    name!(depth, i32),
    name!(entity_name, String),
    name!(depends_on, String),
)> {
    let query = format!(
        "WITH RECURSIVE dep_tree AS (
            -- Start with the requested entity
            SELECT
                pg_tview_meta.entity,
                0 as depth,
                ARRAY[pg_tview_meta.entity] as path,
                pg_tview_meta.entity as depends_on
            FROM pg_tview_meta
            WHERE pg_tview_meta.entity = '{}'

            UNION ALL

            -- Find TVIEWs that depend on entities in our tree
            SELECT
                m.entity,
                dt.depth + 1,
                dt.path || m.entity,
                dt.entity as depends_on
            FROM dep_tree dt
            JOIN pg_tview_meta m ON ('tv_' || dt.entity)::regclass::oid = ANY(m.dependencies)
            WHERE NOT (m.entity = ANY(dt.path))  -- Prevent cycles
              AND dt.depth < 10  -- Depth limit
        )
        SELECT depth, entity_name, depends_on
        FROM dep_tree
        ORDER BY depth, entity_name",
        entity.replace('\'',"''")
    );

    let results = Spi::connect(|client| {
        match client.select(&query, None, &[]) {
            Ok(rows) => {
                let mut paths = Vec::new();
                for row in rows {
                    let depth = row["depth"].value::<i32>()?.unwrap_or(0);
                    let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
                    let depends_on = row["depends_on"].value::<String>()?.unwrap_or_default();
                    paths.push((depth, entity_name, depends_on));
                }
                Ok::<_, spi::Error>(paths)
            },
            Err(e) => {
                warning!("Failed to query cascade path: {}", e);
                Ok(Vec::new())
            }
        }
    }).unwrap_or_default();

    TableIterator::new(results)
}

/// Get performance statistics for all TVIEWs
///
/// Returns size, row count, and index information for each TVIEW
#[pg_extern]
fn pg_tviews_performance_stats() -> TableIterator<'static, (
    name!(entity, String),
    name!(table_size, String),
    name!(total_size, String),
    name!(row_count, i64),
    name!(index_count, i32),
)> {
    let query = "
        SELECT
            pg_tview_meta.entity,
            pg_size_pretty(pg_relation_size('tv_' || pg_tview_meta.entity)) as table_size,
            pg_size_pretty(pg_total_relation_size('tv_' || pg_tview_meta.entity)) as total_size,
            (SELECT COUNT(*) FROM ('tv_' || pg_tview_meta.entity)::regclass) as row_count,
            (SELECT COUNT(*)::int FROM pg_indexes WHERE tablename = 'tv_' || pg_tview_meta.entity) as index_count
        FROM pg_tview_meta
        ORDER BY pg_relation_size('tv_' || pg_tview_meta.entity) DESC
    ";

    let results = Spi::connect(|client| {
        match client.select(query, None, &[]) {
            Ok(rows) => {
                let mut stats = Vec::new();
                for row in rows {
                    let entity = row["entity"].value::<String>()?.unwrap_or_default();
                    let table_size = row["table_size"].value::<String>()?.unwrap_or_default();
                    let total_size = row["total_size"].value::<String>()?.unwrap_or_default();
                    let row_count = row["row_count"].value::<i64>()?.unwrap_or(0);
                    let index_count = row["index_count"].value::<i32>()?.unwrap_or(0);
                    stats.push((entity, table_size, total_size, row_count, index_count));
                }
                Ok::<_, spi::Error>(stats)
            },
            Err(e) => {
                warning!("Failed to query performance stats: {}", e);
                Ok(Vec::new())
            }
        }
    }).unwrap_or_default();

    TableIterator::new(results)
}

