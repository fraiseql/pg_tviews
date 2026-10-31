//! The value locks of a statement's capture, taken in one pass (ADR 0207,
//! #277): the writer's exclusive locks on the values it changed and, after a
//! link change, the refresh's shared locks on what the rows it reaches read,
//! for every TVIEW whose delta trigger fires for the statement. The rows are
//! then looked up again in a new snapshot, and rows found only then are locked
//! (READ COMMITTED) or fail the transaction (REPEATABLE READ, SERIALIZABLE).

use crate::catalog::TviewMeta;
use crate::concurrency::{Pass, Policy};
use crate::error::TViewResult;
use crate::queue::key::KeyValue;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;

/// The rows a TVIEW's lookup found before its pass, when the transaction had
/// changed a link (`None` otherwise: no read set was locked).
pub(super) type Found = Option<Vec<KeyValue>>;

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
    /// The passes taken by a statement's first delta trigger, with what each
    /// TVIEW whose trigger is still to fire found. A trigger takes its entry;
    /// a statement's record goes once its last trigger fired.
    static RECORDED: RefCell<Vec<(Statement, HashMap<String, Found>)>> =
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

    /// What the pass of this statement recorded for `entity`, taken out of
    /// the record; `None` when no pass recorded it.
    pub(super) fn recorded(self, entity: &str) -> Option<Found> {
        RECORDED.with_borrow_mut(|recorded| {
            let i = recorded.iter().position(|(s, _)| *s == self)?;
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

    /// Record the pass of this statement: what each TVIEW in `found` found.
    pub(super) fn record(self, found: HashMap<String, Found>) {
        if !found.is_empty() {
            RECORDED.with_borrow_mut(|recorded| recorded.push((self, found)));
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

/// The rows a write reaches, from `lookup` run after the pass that took the
/// writer's locks: `found`, what it found before the pass, had its read set
/// locked. Rows found only now were created or re-pointed by a transaction that
/// committed in between: under READ COMMITTED their read set is locked and the
/// lookup runs again, until it finds no new row; under REPEATABLE READ and
/// SERIALIZABLE the snapshot can't follow them, and the transaction fails with
/// 40001.
///
/// # Errors
/// What `lookup` or a read-set query returns.
pub(super) fn settle(
    meta: &TviewMeta,
    found: Found,
    mut lookup: impl FnMut() -> TViewResult<Vec<KeyValue>>,
) -> TViewResult<Vec<KeyValue>> {
    let mut keys = lookup()?;
    let Some(found) = found else {
        return Ok(keys);
    };
    let mut known: HashSet<KeyValue> = found.into_iter().collect();
    loop {
        let new: Vec<KeyValue> = keys
            .iter()
            .filter(|key| !known.contains(*key))
            .cloned()
            .collect();
        if new.is_empty() {
            return Ok(keys);
        }
        if Policy::current() == Policy::FailFast {
            crate::concurrency::serialization_failure(
                "a concurrent transaction linked rows to the rows this write changed",
            );
        }
        let mut pass = Pass::default();
        add_read_set(meta, &new, &mut pass)?;
        crate::concurrency::take(&pass);
        known.extend(new);
        keys = lookup()?;
    }
}

/// The rows of `meta`'s TVIEW that changed rows reach through `mapping`, for a
/// path that maps them one TVIEW at a time (a partitioned table's row trigger,
/// rows entering with a partition): one pass of the writer's locks (`values`, as
/// [`crate::concurrency::changed_rows`] runs it) and, after a link change, of the
/// read set of what `lookup` finds now, then [`settle`]. `lookup` runs in a new
/// snapshot, checked against the latest one under REPEATABLE READ when its
/// argument says so.
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
    let found = if crate::concurrency::links_changed() {
        let keys = lookup(false)?;
        add_read_set(meta, &keys, &mut pass)?;
        Some(keys)
    } else {
        None
    };
    crate::concurrency::take(&pass);
    settle(meta, found, || lookup(true))
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
