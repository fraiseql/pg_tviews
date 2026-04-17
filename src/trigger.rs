use crate::catalog::entity_for_table;
use crate::queue::{enqueue_refresh, enqueue_refresh_bulk, enqueue_refresh_dedup};
use crate::utils::{quote_identifier, tuple_get_i64};
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
                match tuple.get_by_name::<String>(key_col) {
                    Ok(Some(key_val)) => {
                        enqueue_refresh_dedup(entity, &key_val);
                    }
                    Ok(None) => {
                        warning!("DISTINCT ON key '{key_col}' is NULL for entity '{entity}'");
                    }
                    Err(e) => {
                        warning!(
                            "Failed to extract DISTINCT ON key '{key_col}' for '{entity}': {e:?}"
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
    //    (e.g. tb_comment is in tv_post's dependencies array)
    //    PK extraction is handled inside — it reads FK values from the child row
    enqueue_indirect_parents(trigger, table_oid);

    Ok(None)
}

/// Look up parent entities for an indirect base table and enqueue their refreshes.
///
/// When a child table (e.g. `tb_comment`) changes, we find all parent TVIEWs whose
/// `dependencies` array includes this table's OID. For each parent, we extract the
/// FK value from the child row (e.g. `fk_post`) to determine which parent row to refresh.
fn enqueue_indirect_parents(trigger: &PgTrigger, table_oid: pg_sys::Oid) {
    let parent_entities = match crate::catalog::parent_entities_for_base_table(table_oid) {
        Ok(entities) => entities,
        Err(e) => {
            warning!(
                "Failed to find parent entities for table {:?}: {:?}",
                table_oid,
                e
            );
            return;
        }
    };

    if parent_entities.is_empty() {
        return; // table not managed by pg_tviews at all
    }

    // Use NEW for INSERT/UPDATE, OLD for DELETE
    let Some(tuple) = trigger.new().or_else(|| trigger.old()) else {
        warning!("No tuple available in trigger context");
        return;
    };

    for parent_entity in parent_entities {
        // Convention: child row has fk_{parent_entity} column
        let fk_col = format!("fk_{parent_entity}");
        match tuple_get_i64(&tuple, &fk_col) {
            Some(parent_pk) => {
                enqueue_refresh(&parent_entity, parent_pk);
            }
            None => {
                warning!(
                    "FK column {} is NULL or missing, skipping refresh for {}",
                    fk_col,
                    parent_entity
                );
            }
        }
    }
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
