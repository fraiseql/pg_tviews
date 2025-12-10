use pgrx::prelude::*;
/// Trigger Handler: Change Detection and Queue Management
///
/// This module implements PostgreSQL triggers for TVIEW change tracking:
/// - **Row-level Triggers**: Detects INSERT/UPDATE/DELETE on base tables
/// - **Primary Key Extraction**: Identifies changed rows for selective refresh
/// - **Queue Enqueueing**: Adds refresh requests to transaction queue
/// - **Bulk Operations**: Handles multi-row changes efficiently
///
/// ## Trigger Lifecycle
///
/// 1. PostgreSQL calls trigger for each changed row
/// 2. Extract primary key of changed row
/// 3. Map table OID to entity name
/// 4. Enqueue (entity, pk) pair for refresh
/// 5. Transaction commit processes the queue
///
/// ## Performance Considerations
///
/// - Triggers run in critical path - must be fast
/// - Bulk enqueueing for multi-row operations
/// - Minimal database queries during trigger execution
/// - Queue processing deferred to commit time
use pgrx::spi;
use crate::queue::{enqueue_refresh, enqueue_refresh_bulk, register_commit_callback_once};
use crate::catalog::entity_for_table;
use crate::refresh::bulk::quote_identifier;

/// Trigger handler function for TVIEW cascades
/// This is called by triggers installed on base tables when rows change
#[pg_trigger]
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract table OID and PK
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };
    let pk_value = match crate::utils::extract_pk(trigger) {
        Ok(pk) => pk,
        Err(e) => {
            warning!("Failed to extract primary key from trigger: {:?}", e);
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
            warning!("Failed to resolve entity for table OID {:?}: {:?}", table_oid, e);
            return Ok(None);
        }
    };

    // Enqueue refresh request (deferred to commit)
    if let Err(e) = enqueue_refresh(&entity, pk_value) {
        warning!("Failed to enqueue refresh for {}[{}]: {:?}", entity, pk_value, e);
        return Ok(None);
    }

    // Register commit callback (once per transaction)
    if let Err(e) = register_commit_callback_once() {
        warning!("Failed to register commit callback: {:?}", e);
        return Ok(None);
    }

    Ok(None)
}

/// Statement-level trigger handler for bulk operations (Phase 9A)
/// This is called once per statement instead of once per row
#[pg_trigger]
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
            warning!("Failed to resolve entity for table OID {:?}: {:?}", table_oid, e);
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
    if let Err(e) = enqueue_refresh_bulk(&entity, changed_pks) {
        warning!("Failed to bulk enqueue refresh for {}: {:?}", entity, e);
        return Ok(None);
    }

    // Register commit callback (once per transaction)
    if let Err(e) = register_commit_callback_once() {
        warning!("Failed to register commit callback: {:?}", e);
        return Ok(None);
    }

    Ok(None)
}

/// Extract primary keys from PostgreSQL transition tables
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
    let pk_column = get_pk_column_name(trigger.relation().map_err(|_| crate::TViewError::SpiError {
        query: "get relation".to_string(),
        error: "Failed to get trigger relation".to_string(),
    })?.oid())?;

    // Query transition table for all PKs
    // IMPORTANT: Transition table references don't need quote_ident()
    // They are special PostgreSQL identifiers visible only in trigger context
    let query = format!(
        "SELECT DISTINCT {} FROM {}",
        quote_identifier(&pk_column),
        transition_table_name  // No quoting - it's a special reference
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
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
/// Uses convention: pk_<entity> where entity is derived from table name tb_<entity>
fn get_pk_column_name(table_oid: pg_sys::Oid) -> spi::Result<String> {
    // Get entity name from table OID
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => return Err(crate::TViewError::SpiError {
            query: "entity_for_table".to_string(),
            error: "Table not managed by pg_tviews".to_string(),
        }.into()),
        Err(e) => return Err(crate::TViewError::SpiError {
            query: "entity_for_table".to_string(),
            error: format!("Failed to get entity: {:?}", e),
        }.into()),
    };

    // Convention: pk_<entity>
    Ok(format!("pk_{}", entity))
}