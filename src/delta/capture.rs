//! The value locks of a capture taken in one pass (ADR 0207, #277): the
//! writer's exclusive locks on the values it changed and, after a link change,
//! the refresh's shared locks on what the rows it reaches read. The rows are
//! then looked up again in a new snapshot, after the writer's locks; the flush
//! locks the read set of every row it computes, those found only now among them.

use crate::catalog::TviewMeta;
use crate::concurrency::Pass;
use crate::error::TViewResult;
use crate::queue::key::KeyValue;

/// The rows of `meta`'s TVIEW that changed rows reach through `mapping`, for a
/// path that maps them one TVIEW at a time (a partitioned table's row trigger,
/// rows entering with a partition): one pass of the writer's locks (`values`, as
/// [`crate::concurrency::changed_rows`] runs it) and, after a link change, of the
/// read set of what `lookup` finds now, then `lookup` again, after the writer's
/// locks. `lookup` runs in a new snapshot.
///
/// # Errors
/// What `values`, `lookup` or a read-set query returns.
pub(super) fn locked_keys(
    meta: &TviewMeta,
    mapping: &crate::lineage::KeyMapping,
    values: impl FnMut(&str) -> TViewResult<Vec<String>>,
    mut lookup: impl FnMut() -> TViewResult<Vec<KeyValue>>,
) -> TViewResult<Vec<KeyValue>> {
    let mut pass = Pass::default();
    crate::concurrency::changed_rows(mapping, &mut pass, values)?;
    if crate::concurrency::links_changed() {
        add_read_set(meta, &lookup()?, &mut pass)?;
    }
    crate::concurrency::take(&pass);
    lookup()
}

/// Add the read set of `meta`'s rows `keys` to `pass`, read as the TVIEW's
/// owner.
///
/// # Errors
/// What a read-set query returns.
pub(super) fn add_read_set(
    meta: &TviewMeta,
    keys: &[KeyValue],
    pass: &mut Pass,
) -> TViewResult<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let _owner = crate::owner::AsOwner::of_entity(&meta.entity_name)?;
    crate::concurrency::reads::read_set(meta, keys, pass)
}
