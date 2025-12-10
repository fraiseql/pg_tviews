use pgrx::prelude::*;
use pgrx::spi;

/// Trigger handler function for TVIEW cascades
/// This is called by triggers installed on base tables when rows change
#[pg_trigger]
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract the table that triggered this event
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };

    // Extract the primary key value from the changed row
    let pk_value = match crate::utils::extract_pk(trigger) {
        Ok(pk) => pk,
        Err(e) => {
            warning!("Failed to extract primary key from trigger: {:?}", e);
            return Ok(None);
        }
    };

    // Call the cascade refresh function
    crate::pg_tviews_cascade(table_oid, pk_value);

    // Return None to indicate no row modification
    Ok(None)
}