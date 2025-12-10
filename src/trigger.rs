use pgrx::prelude::*;
use pgrx::spi;
use crate::queue::{enqueue_refresh, register_commit_callback_once};
use crate::catalog::entity_for_table;

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