//! The transaction's pending refresh work, and how a savepoint undoes it.
//!
//! [`Pending`] holds what the triggers queued and the flush has not taken yet:
//! the keys to refresh, the direct patches riding on them, and the fan-out
//! patches. One owner, so a savepoint covers all three at once.
//!
//! A subtransaction leaves the live state alone: work queued before it stays
//! queued while it runs. Inside one, every change to the pending work is logged
//! with what it replaced; rolling the subtransaction back replays that log in
//! reverse, releasing it keeps the entries for the enclosing level. A savepoint
//! costs O(1) to open, and each change O(1) to log, however much is queued (a
//! per-row EXCEPTION block in a bulk write opens one per row). Only a flush
//! that drains the queue inside a subtransaction copies what it takes.

use super::key::RefreshKey;
use super::patch::{FanoutKey, FanoutMap, PatchState};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Refresh work queued and not yet flushed.
#[derive(Debug, Default, Clone)]
pub struct Pending {
    /// The rows to refresh, deduplicated.
    pub queue: HashSet<RefreshKey>,
    /// The direct patch (or poison) of each queued key that has one.
    pub patches: HashMap<RefreshKey, PatchState>,
    /// Fields to write into every row a parent key reaches.
    pub fanout: FanoutMap,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.patches.is_empty() && self.fanout.is_empty()
    }
}

/// How to undo one change made inside a subtransaction.
enum Undo {
    /// The key was not queued before.
    Queued(RefreshKey),
    /// The key's patch before the change.
    Patch(RefreshKey, Option<PatchState>),
    /// The fan-out fields before the change.
    Fanout(FanoutKey, Option<Map<String, Value>>),
    /// A flush took everything that was pending.
    Drained(Pending),
}

/// Where a subtransaction started.
struct Mark {
    undo: usize,
    journal: usize,
}

#[derive(Default)]
struct Txn {
    pending: Pending,
    undo: Vec<Undo>,
    marks: Vec<Mark>,
}

impl Txn {
    fn logging(&self) -> bool {
        !self.marks.is_empty()
    }

    fn revert(&mut self, undo: Undo) {
        match undo {
            Undo::Queued(key) => {
                self.pending.queue.remove(&key);
            }
            Undo::Patch(key, Some(old)) => {
                self.pending.patches.insert(key, old);
            }
            Undo::Patch(key, None) => {
                self.pending.patches.remove(&key);
            }
            Undo::Fanout(key, Some(old)) => {
                self.pending.fanout.insert(key, old);
            }
            Undo::Fanout(key, None) => {
                self.pending.fanout.remove(&key);
            }
            // Every later change is already reverted, so the state is what the
            // drain left: nothing.
            Undo::Drained(taken) => self.pending = taken,
        }
    }
}

thread_local! {
    static TXN: RefCell<Txn> = RefCell::new(Txn::default());

    /// Entities checked for a post-crash truncation in this transaction, so the
    /// check runs at most once per entity.
    pub static TX_CRASH_RECOVERY_CHECKED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Queue `key`. Returns whether it was not queued yet.
pub fn queue_insert(key: RefreshKey) -> bool {
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        if t.pending.queue.contains(&key) {
            return false;
        }
        if t.logging() {
            t.undo.push(Undo::Queued(key.clone()));
        }
        t.pending.queue.insert(key);
        true
    })
}

/// Whether `key` is queued.
pub fn queue_contains(key: &RefreshKey) -> bool {
    TXN.with(|t| t.borrow().pending.queue.contains(key))
}

/// How many keys are queued.
pub fn get_queue_size() -> usize {
    TXN.with(|t| t.borrow().pending.queue.len())
}

/// The queued keys (diagnostics).
pub fn get_queue_contents() -> Vec<RefreshKey> {
    TXN.with(|t| t.borrow().pending.queue.iter().cloned().collect())
}

/// Change `key`'s patch through `change`, which sees `None` when it has none.
pub fn update_patch<R>(key: RefreshKey, change: impl FnOnce(&mut Option<PatchState>) -> R) -> R {
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        let mut slot = t.pending.patches.remove(&key);
        if t.logging() {
            t.undo.push(Undo::Patch(key.clone(), slot.clone()));
        }
        let result = change(&mut slot);
        if let Some(state) = slot {
            t.pending.patches.insert(key, state);
        }
        result
    })
}

/// `key`'s patch, if it has one.
#[cfg(test)]
pub fn patch_of(key: &RefreshKey) -> Option<PatchState> {
    TXN.with(|t| t.borrow().pending.patches.get(key).cloned())
}

/// Merge `fields` into the fan-out of `key` (a later value wins per field).
pub fn merge_fanout(key: FanoutKey, fields: Map<String, Value>) {
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        if t.logging() {
            let old = t.pending.fanout.get(&key).cloned();
            t.undo.push(Undo::Fanout(key.clone(), old));
        }
        t.pending.fanout.entry(key).or_default().extend(fields);
    });
}

/// Take everything pending: the flush consumes it.
pub fn drain() -> Pending {
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        let taken = std::mem::take(&mut t.pending);
        if t.logging() && !taken.is_empty() {
            t.undo.push(Undo::Drained(taken.clone()));
        }
        taken
    })
}

/// Forget everything pending: the transaction ended.
pub fn clear() {
    TXN.with(|t| *t.borrow_mut() = Txn::default());
}

/// A subtransaction started.
pub fn savepoint_start() {
    let journal = super::affected::position();
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        let undo = t.undo.len();
        t.marks.push(Mark { undo, journal });
    });
}

/// The innermost subtransaction committed: what it did now belongs to its
/// parent.
pub fn savepoint_commit() {
    TXN.with(|t| {
        let mut t = t.borrow_mut();
        t.marks.pop();
        if t.marks.is_empty() {
            t.undo.clear();
        }
    });
}

/// The innermost subtransaction rolled back: undo what it queued, patched and
/// drained, and what it journaled.
pub fn savepoint_abort() {
    let journal = TXN.with(|t| {
        let mut t = t.borrow_mut();
        let mark = t.marks.pop()?;
        while t.undo.len() > mark.undo {
            if let Some(undo) = t.undo.pop() {
                t.revert(undo);
            }
        }
        Some(mark.journal)
    });
    if let Some(journal) = journal {
        super::affected::rollback_to(journal);
    }
}

/// How many subtransactions are open around the pending work.
#[cfg(test)]
pub fn savepoint_depth() -> usize {
    TXN.with(|t| t.borrow().marks.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::key::KeyValue;

    fn key(pk: i64) -> RefreshKey {
        RefreshKey::pk("user", pk)
    }

    fn queued() -> HashSet<RefreshKey> {
        get_queue_contents().into_iter().collect()
    }

    fn direct(field: &str) -> PatchState {
        let mut fields = Map::new();
        fields.insert(field.to_string(), Value::Bool(true));
        PatchState::Direct(vec![(Vec::new(), fields)])
    }

    #[test]
    fn commit_keeps_what_was_queued_before_and_inside() {
        clear();
        queue_insert(key(1));
        savepoint_start();
        queue_insert(key(2));
        savepoint_commit();
        assert_eq!(queued(), HashSet::from([key(1), key(2)]));
        clear();
    }

    #[test]
    fn abort_keeps_what_was_queued_before_only() {
        clear();
        queue_insert(key(1));
        savepoint_start();
        queue_insert(key(1));
        queue_insert(key(2));
        savepoint_abort();
        assert_eq!(queued(), HashSet::from([key(1)]));
        clear();
    }

    #[test]
    fn nested_inner_commit_then_outer_abort_undoes_both() {
        clear();
        queue_insert(key(1));
        savepoint_start();
        queue_insert(key(2));
        savepoint_start();
        queue_insert(key(3));
        savepoint_commit();
        assert_eq!(queued(), HashSet::from([key(1), key(2), key(3)]));
        savepoint_abort();
        assert_eq!(queued(), HashSet::from([key(1)]));
        assert_eq!(savepoint_depth(), 0);
        clear();
    }

    #[test]
    fn abort_restores_patches_changed_inside() {
        clear();
        update_patch(key(1), |slot| *slot = Some(direct("name")));
        savepoint_start();
        update_patch(key(1), |slot| *slot = Some(PatchState::Poisoned));
        update_patch(key(2), |slot| *slot = Some(direct("bio")));
        savepoint_abort();
        assert_eq!(patch_of(&key(1)), Some(direct("name")));
        assert_eq!(patch_of(&key(2)), None);
        clear();
    }

    #[test]
    fn abort_restores_fanout_changed_inside() {
        clear();
        let fk = ("post".to_string(), "fk_user".to_string(), 1);
        let mut before = Map::new();
        before.insert("name".to_string(), Value::from("a"));
        merge_fanout(fk.clone(), before.clone());
        savepoint_start();
        let mut inside = Map::new();
        inside.insert("name".to_string(), Value::from("b"));
        merge_fanout(fk.clone(), inside);
        savepoint_abort();
        assert_eq!(drain().fanout.get(&fk), Some(&before));
        clear();
    }

    #[test]
    fn abort_after_a_drain_inside_gives_back_what_was_pending() {
        clear();
        queue_insert(key(1));
        update_patch(key(1), |slot| *slot = Some(direct("name")));
        savepoint_start();
        queue_insert(key(2));
        let taken = drain();
        assert_eq!(taken.queue.len(), 2);
        queue_insert(key(1));
        queue_insert(key(3));
        savepoint_abort();
        assert_eq!(queued(), HashSet::from([key(1)]));
        assert_eq!(patch_of(&key(1)), Some(direct("name")));
        clear();
    }

    #[test]
    fn insert_reports_whether_the_key_is_new() {
        clear();
        assert!(queue_insert(RefreshKey::new("user", KeyValue::Int(1))));
        assert!(!queue_insert(RefreshKey::new("user", KeyValue::Int(1))));
        assert!(queue_contains(&key(1)));
        clear();
    }
}
