use pgrx::prelude::*;

use crate::refresh::refresh_pk;
use crate::utils::extract_pk;

#[pg_trigger]
pub fn tview_trigger<'a>(trigger: &'a PgTrigger<'a>) -> Result<
    Option<pgrx::heap_tuple::PgHeapTuple<'a, pgrx::pgbox::AllocatedByPostgres>>,
    spi::Error,
> {
    let rel = trigger.relation().map_err(|_| spi::Error::NoTupleTable)?;
    let source_oid = rel.oid();
    let pk = extract_pk(trigger)?;

    // Main: refresh the TVIEW graph starting from this PK
    refresh_pk(source_oid, pk)?;

    // For AFTER triggers, the return value is ignored, but NEW is conventional.
    Ok(trigger.new())
}

