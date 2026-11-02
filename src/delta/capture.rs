//! The value locks of a statement's capture, taken in one pass (ADR 0207,
//! #277): the writer's exclusive locks on the values it changed and, after a
//! link change, the refresh's shared locks on what the rows it reaches read,
//! for every TVIEW whose delta trigger fires for the statement. The rows are
//! then looked up again in a new snapshot, after the writer's locks; the flush
//! locks the read set of every row it computes, those found only now among them.

use crate::catalog::TviewMeta;
use crate::concurrency::Pass;
use crate::error::TViewResult;
use crate::queue::key::KeyValue;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::CStr;

/// One statement's delta triggers on one table for one event: its executor
/// frame, the table, the triggers' type and the transition tables they share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Statement {
    frame: u64,
    table: Oid,
    tgtype: i16,
    old: usize,
    new: usize,
}

thread_local! {
    /// The passes taken by a statement's first delta trigger, with the TVIEWs
    /// whose triggers are still to fire. A trigger takes its entity out; a
    /// statement's record goes once its last trigger fired.
    static RECORDED: RefCell<Vec<(Statement, HashSet<String>)>> =
        const { RefCell::new(Vec::new()) };
}

impl Statement {
    /// The statement `trigger` fires for; `None` outside the executor.
    pub(super) fn of(trigger: &PgTrigger<'_>, table: Oid) -> Option<Self> {
        let frame = crate::executor::frame()?;
        let td = trigger.trigger_data();
        // SAFETY: an AFTER statement trigger's TriggerData and its Trigger.
        let tgtype = unsafe { (*td.tg_trigger).tgtype };
        Some(Self {
            frame,
            table,
            tgtype,
            old: td.tg_oldtable as usize,
            new: td.tg_newtable as usize,
        })
    }

    /// Whether a pass of this statement took `entity`'s locks, taking it out
    /// of the record.
    pub(super) fn recorded(self, entity: &str) -> bool {
        RECORDED.with_borrow_mut(|recorded| {
            let Some(i) = recorded.iter().position(|(s, _)| *s == self) else {
                return false;
            };
            let found = recorded[i].1.remove(entity);
            if recorded[i].1.is_empty() {
                recorded.swap_remove(i);
            }
            found
        })
    }

    /// Whether a pass of this statement is recorded.
    pub(super) fn has_record(self) -> bool {
        RECORDED.with_borrow(|recorded| recorded.iter().any(|(s, _)| *s == self))
    }

    /// Record the pass of this statement: it took the locks of `entities`.
    pub(super) fn record(self, entities: HashSet<String>) {
        if !entities.is_empty() {
            RECORDED.with_borrow_mut(|recorded| recorded.push((self, entities)));
        }
    }
}

/// The transaction or a subtransaction ended: the locks recorded may be gone.
pub fn clear() {
    RECORDED.with_borrow_mut(Vec::clear);
}

/// The entities of the `pg_tviews` delta triggers that fire for `trigger`'s
/// statement, `trigger` among them, in the order PostgreSQL fires them (by
/// name): those of its table with its function and type, enabled in this
/// session. `None` when `trigger` isn't found among them.
pub(super) fn firing_entities(trigger: &PgTrigger<'_>) -> Option<Vec<String>> {
    let td = trigger.trigger_data();
    // SAFETY: the TriggerData of an AFTER statement trigger: its Trigger and its
    // relation's TriggerDesc (checked for null) are valid for the call, and
    // `numtriggers` bounds the array; each argument is a NUL-terminated string.
    unsafe {
        let this = &*td.tg_trigger;
        let desc = (*td.tg_relation).trigdesc;
        if desc.is_null() {
            return None;
        }
        let count = usize::try_from((*desc).numtriggers).unwrap_or(0);
        let mut entities = Vec::new();
        let mut found = false;
        for i in 0..count {
            let t = &*(*desc).triggers.add(i);
            if t.tgfoid != this.tgfoid
                || t.tgtype != this.tgtype
                || t.tgnargs < 1
                || !crate::executor::fires(t.tgenabled.cast_unsigned())
            {
                continue;
            }
            let entity = CStr::from_ptr(*t.tgargs).to_string_lossy().into_owned();
            found |= t.tgoid == this.tgoid;
            entities.push(entity);
        }
        found.then_some(entities)
    }
}

/// The rows of `meta`'s TVIEW that changed rows reach through `mapping`, for a
/// path that maps them one TVIEW at a time (a partitioned table's row trigger,
/// rows entering with a partition): one pass of the writer's locks (`values`, as
/// [`crate::concurrency::changed_rows`] runs it) and, after a link change, of the
/// read set of what `lookup` finds now, then `lookup` again, after the writer's
/// locks. `lookup` runs in a new snapshot, checked against the latest one under
/// REPEATABLE READ when its argument says so.
///
/// # Errors
/// What `values`, `lookup` or a read-set query returns.
pub(super) fn locked_keys(
    meta: &TviewMeta,
    mapping: &crate::lineage::KeyMapping,
    values: impl FnMut(&str) -> TViewResult<Vec<String>>,
    mut lookup: impl FnMut(bool) -> TViewResult<Vec<KeyValue>>,
) -> TViewResult<Vec<KeyValue>> {
    let mut pass = Pass::default();
    crate::concurrency::changed_rows(mapping, &mut pass, values)?;
    if crate::concurrency::links_changed() {
        add_read_set(meta, &lookup(false)?, &mut pass)?;
    }
    crate::concurrency::take(&pass);
    lookup(true)
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
