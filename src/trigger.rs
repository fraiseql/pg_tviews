use crate::catalog::entity_for_table;
use crate::queue::{enqueue_refresh, enqueue_refresh_bulk, enqueue_refresh_dedup};
use crate::utils::{IntExtraction, quote_identifier, tuple_get_i64};
use pgrx::prelude::*;
/// Trigger Handler: Change Detection and Queue Management
///
/// This module implements `PostgreSQL` triggers for TVIEW change tracking:
/// - **Row-level Triggers**: Detects INSERT/UPDATE/DELETE on base tables
/// - **Primary Key Extraction**: Identifies changed rows for selective refresh
/// - **Queue Enqueueing**: Adds refresh requests to transaction queue
/// - **Bulk Operations**: Handles multi-row changes efficiently
///
/// ## Trigger Lifecycle
///
/// 1. `PostgreSQL` calls trigger for each changed row
/// 2. Extract primary key of changed row
/// 3. Map table OID to entity name
/// 4. Enqueue `(entity, pk)` pair for refresh
/// 5. Transaction commit processes the queue
///
/// ## Performance Considerations
///
/// - Triggers run in critical path - must be fast
/// - Bulk enqueueing for multi-row operations
/// - Minimal database queries during trigger execution
/// - Queue processing deferred to commit time
use pgrx::spi;

/// Result of attempting to extract a DISTINCT ON key from a tuple
enum KeyExtraction {
    /// Successfully extracted and converted to String
    Value(String),
    /// Column exists but value is NULL
    Null,
    /// All typed extraction attempts failed (unsupported column type)
    TypeMismatch,
}

/// Extract DISTINCT ON key value from tuple, trying multiple types
/// Returns the extraction result: Value on success, Null if column is NULL, TypeMismatch if unsupported type
fn extract_distinct_on_key<'a>(
    tuple: &PgHeapTuple<'a, AllocatedByPostgres>,
    key_col: &str,
) -> KeyExtraction {
    // Try String (TEXT, UUID, VARCHAR)
    match tuple.get_by_name::<String>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {} // type mismatch, try next
    }
    // Try i64 (BIGINT)
    match tuple.get_by_name::<i64>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val.to_string()),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {}
    }
    // Try i32 (INTEGER)
    match tuple.get_by_name::<i32>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val.to_string()),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {}
    }
    KeyExtraction::TypeMismatch
}

/// Trigger handler function for TVIEW cascades
/// This is called by triggers installed on base tables when rows change
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract table OID
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };

    // 1. Direct entity: this table IS a TVIEW source (e.g. tb_user → entity "user")
    match crate::queue::cache::table_cache::entity_info_cached(table_oid) {
        Ok(Some(entity_info)) => {
            let entity = &entity_info.name;
            // Check if this is a DISTINCT ON TVIEW using cached distinct_on_key
            if let Some(key_col) = &entity_info.distinct_on_key {
                // DISTINCT ON TVIEW: enqueue dedup key value instead of base PK
                let tuple = match trigger.new().or_else(|| trigger.old()) {
                    Some(t) => t,
                    None => {
                        warning!("No tuple in trigger context for DISTINCT ON TVIEW '{entity}'");
                        return Ok(None);
                    }
                };
                match extract_distinct_on_key(&tuple, key_col) {
                    KeyExtraction::Value(key_val) => {
                        enqueue_refresh_dedup(entity, &key_val);
                    }
                    KeyExtraction::Null => {
                        warning!(
                            "DISTINCT ON key '{key_col}' is NULL for entity '{entity}' — skipping refresh"
                        );
                    }
                    KeyExtraction::TypeMismatch => {
                        warning!(
                            "Cannot extract DISTINCT ON key '{key_col}' for '{entity}': \
                             unsupported column type — skipping refresh for this row"
                        );
                    }
                }
            } else {
                // Standard PK-based TVIEW: extract pk_<entity>
                let pk_value = match crate::utils::extract_pk(trigger) {
                    Ok(pk) => pk,
                    Err(e) => {
                        warning!("Failed to extract primary key from trigger: {:?}", e);
                        return Ok(None);
                    }
                };
                enqueue_refresh(entity, pk_value);
            }
            return Ok(None);
        }
        Ok(None) => { /* fall through to indirect lookup */ }
        Err(e) => {
            warning!(
                "Failed to resolve entity for table OID {:?}: {:?}",
                table_oid,
                e
            );
            return Ok(None);
        }
    }

    // 2. Indirect: this table is a dependency of one or more TVIEWs
    //    Follow cascade paths to determine which TVIEW rows need refreshing
    enqueue_cascade_parents(trigger, table_oid);

    Ok(None)
}

/// Follow cascade paths from a base table change to enqueue parent TVIEW refreshes.
///
/// When a base table (e.g. `tb_item`) changes, loads cascade paths from the
/// transaction-scoped cache and follows each path hop-by-hop via SPI to
/// discover which TVIEW entity rows need refreshing.
fn enqueue_cascade_parents(trigger: &PgTrigger, table_oid: pg_sys::Oid) {
    let paths: Vec<crate::cascade_path::CascadePath> =
        match crate::queue::cache::cascade_cache::cascade_paths_for_table(table_oid) {
            Ok(p) => p,
            Err(e) => {
                warning!(
                    "Failed to load cascade paths for table {:?}: {:?}",
                    table_oid,
                    e
                );
                return;
            }
        };

    if paths.is_empty() {
        return;
    }

    let Some(tuple) = trigger.new().or_else(|| trigger.old()) else {
        warning!("No tuple available in trigger context");
        return;
    };

    for path in &paths {
        if let Err(e) = follow_cascade_path(path, &tuple) {
            warning!(
                "Cascade refresh failed for path {} → {}: {:?}",
                path.source_table,
                path.entity_name,
                e
            );
        }
    }
}

/// Follow a single cascade path to enqueue refresh(es) for the target entity.
fn follow_cascade_path(
    path: &crate::cascade_path::CascadePath,
    tuple: &PgHeapTuple<AllocatedByPostgres>,
) -> crate::TViewResult<()> {
    if path.unresolvable {
        // Full refresh fallback — enqueue with pk=0 sentinel
        // (the flush engine will treat this as "refresh all rows")
        warning!(
            "Unresolvable cascade path for entity '{}' — full refresh needed",
            path.entity_name
        );
        return Ok(());
    }

    // Step 1: Read initial_col from the changed tuple
    let mut current_ids = match tuple_get_i64(tuple, &path.initial_col) {
        IntExtraction::Value(pk) => vec![pk],
        IntExtraction::Null => return Ok(()), // FK is NULL, cascade stops
        IntExtraction::Missing => {
            warning!(
                "Initial column '{}' not found on tuple for cascade to '{}'",
                path.initial_col,
                path.entity_name
            );
            return Ok(());
        }
    };

    // Step 2: Follow each intermediate hop via SPI
    for hop in &path.hops {
        if current_ids.is_empty() {
            return Ok(());
        }
        current_ids = crate::queue::spi_batch_lookup(
            hop.table_oid,
            &hop.lookup_col,
            &hop.carry_col,
            &current_ids,
        )?;
    }

    // Step 3: Enqueue refresh for each resulting PK
    for pk in current_ids {
        enqueue_refresh(&path.entity_name, pk);
    }

    Ok(())
}

/// Statement-level AFTER trigger that flushes the refresh queue.
///
/// This fires once per statement (not per row) and processes all queued
/// refresh requests. It ensures auto-commit (implicit) transactions get
/// their TVIEWs refreshed, since the `ProcessUtility` hook only intercepts
/// explicit COMMIT statements.
///
/// For explicit transactions (BEGIN...COMMIT), both this trigger and the
/// `ProcessUtility` hook may run. The flush is idempotent — the second call
/// finds an empty queue and returns immediately.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_flush_trigger<'a>(
    _trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    if let Err(e) = crate::queue::flush_refresh_queue() {
        warning!("TVIEW refresh failed in statement trigger: {:?}", e);
    }
    if let Err(e) = crate::audit::flush_audit_buffer() {
        warning!("Audit flush failed in statement trigger: {:?}", e);
    }
    Ok(None)
}

/// Statement-level trigger handler for bulk operations
/// This is called once per statement instead of once per row
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_stmt_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract table OID
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };

    // Map table OID → entity name
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => {
            // Table not in pg_tview_meta, skip
            return Ok(None);
        }
        Err(e) => {
            warning!(
                "Failed to resolve entity for table OID {:?}: {:?}",
                table_oid,
                e
            );
            return Ok(None);
        }
    };

    // Extract all changed PKs from transition table
    let changed_pks = match extract_pks_from_transition_table(trigger) {
        Ok(pks) => pks,
        Err(e) => {
            warning!("Failed to extract PKs from transition table: {:?}", e);
            return Ok(None);
        }
    };

    if changed_pks.is_empty() {
        // No rows changed, nothing to do
        return Ok(None);
    }

    // Bulk enqueue all changed PKs
    enqueue_refresh_bulk(&entity, changed_pks);

    Ok(None)
}

/// Extract primary keys from `PostgreSQL` transition tables
/// Transition tables are special references visible only in trigger context
fn extract_pks_from_transition_table(trigger: &PgTrigger) -> spi::Result<Vec<i64>> {
    // Determine which transition table to use based on operation type
    // Check which transition table is available (INSERT has NEW, DELETE has OLD, UPDATE has both)
    let transition_table_name = if trigger.new().is_some() && trigger.old().is_none() {
        "new_table" // INSERT
    } else if trigger.new().is_none() && trigger.old().is_some() {
        "old_table" // DELETE
    } else if trigger.new().is_some() && trigger.old().is_some() {
        "new_table" // UPDATE (use NEW for consistency)
    } else {
        return Ok(Vec::new()); // Unsupported event
    };

    // Get PK column name (convention: pk_<entity>)
    let pk_column = get_pk_column_name(
        trigger
            .relation()
            .map_err(|_| crate::TViewError::SpiError {
                query: "get relation".to_string(),
                error: "Failed to get trigger relation".to_string(),
            })?
            .oid(),
    )?;

    // Query transition table for all PKs
    // IMPORTANT: Transition table references don't need quote_ident()
    // They are special PostgreSQL identifiers visible only in trigger context
    let query = format!(
        "SELECT DISTINCT {} FROM {}",
        quote_identifier(&pk_column),
        transition_table_name // No quoting - it's a special reference
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[&pk_column as &str].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

/// Get primary key column name for a table
/// Uses convention: `pk_<entity>` where entity is derived from table name `tb_<entity>`
fn get_pk_column_name(table_oid: pg_sys::Oid) -> spi::Result<String> {
    // Get entity name from table OID
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err(crate::TViewError::SpiError {
                query: "entity_for_table".to_string(),
                error: "Table not managed by pg_tviews".to_string(),
            }
            .into());
        }
        Err(e) => {
            return Err(crate::TViewError::SpiError {
                query: "entity_for_table".to_string(),
                error: format!("Failed to get entity: {e:?}"),
            }
            .into());
        }
    };

    // Convention: pk_<entity>
    Ok(format!("pk_{entity}"))
}
