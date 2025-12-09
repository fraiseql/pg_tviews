use pgrx::prelude::*;

use crate::refresh::refresh_pk;
use crate::util::extract_pk;

#[pg_trigger]
pub fn tview_trigger(trigger: &PgTrigger) -> Result<
    Option<pgrx::heap_tuple::PgHeapTuple<'_, pgrx::pgbox::AllocatedByRust>>,
    spi::Error,
> {
    let rel = trigger.relation()?;
    let source_oid = rel.oid();
    let pk = extract_pk(trigger)?;

    // Main: refresh the TVIEW graph starting from this PK
    refresh_pk(source_oid, pk)?;

    // For AFTER triggers, the return value is ignored, but NEW is conventional.
    Ok(trigger.new())
}

