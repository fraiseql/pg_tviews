//! Per-transaction journal of the TVIEW rows refreshes actually changed (issue #76).
//!
//! Every refresh write (guarded upsert, delete, direct patch) records the keys it
//! really inserted, updated or deleted: a refresh that found nothing to change
//! records nothing. The statement-level flush trigger flushes after every
//! statement, so this journal, not the queue, is what still knows at the end of a
//! mutation function which read-model rows the transaction changed.
//! [`crate::report::pg_tviews_flush_and_report`] reads it.
//!
//! Entries are appended in write order. A savepoint records the journal length and
//! a rollback to it truncates back, so rolled-back writes are never reported. The
//! journal is cleared when the transaction ends. Past `pg_tviews.report_max_tracked`
//! entries only the entity names are kept and the report says it was truncated.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

/// What a refresh write did to one TVIEW row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Inserted,
    Updated,
    /// The row's public `id`, captured by the DELETE (the row is gone afterwards).
    Deleted(Option<String>),
}

/// One row's net change over the journal, with the position of its first entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetChange {
    pub entity: String,
    pub pk: String,
    pub change: Change,
}

#[derive(Default)]
struct Journal {
    entries: Vec<(String, String, Change)>,
    /// Journal length at each open savepoint.
    marks: Vec<usize>,
    /// Entities with changes that were not journaled because the cap was reached.
    overflow: BTreeSet<String>,
}

thread_local! {
    static JOURNAL: RefCell<Journal> = RefCell::new(Journal::default());
    /// Rows changed during the current flush, uncapped: propagation reads it to
    /// skip parents of rows whose refresh changed nothing (issue #85).
    static FLUSH_CHANGED: RefCell<std::collections::HashSet<(String, String)>> =
        RefCell::new(std::collections::HashSet::new());
}

/// A flush starts: forget the previous flush's changed rows.
pub fn begin_flush() {
    FLUSH_CHANGED.with(|c| c.borrow_mut().clear());
}

/// Whether `entity`'s row `pk` was inserted, updated or deleted in this flush.
pub fn changed_in_flush(entity: &str, pk: i64) -> bool {
    FLUSH_CHANGED.with(|c| c.borrow().contains(&(entity.to_string(), pk.to_string())))
}

/// Record that a refresh write changed `entity`'s row `pk`.
pub fn record(entity: &str, pk: String, change: Change) {
    FLUSH_CHANGED.with(|c| c.borrow_mut().insert((entity.to_string(), pk.clone())));
    let cap = crate::config::report_max_tracked();
    if cap == 0 {
        return;
    }
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        if j.entries.len() < cap {
            j.entries.push((entity.to_string(), pk, change));
        } else {
            j.overflow.insert(entity.to_string());
        }
    });
}

/// A savepoint was opened.
pub fn savepoint_start() {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        let len = j.entries.len();
        j.marks.push(len);
    });
}

/// The innermost savepoint was rolled back: forget what it journaled.
pub fn savepoint_abort() {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        if let Some(len) = j.marks.pop() {
            j.entries.truncate(len);
        }
    });
}

/// The innermost savepoint was released: its entries now belong to the parent.
pub fn savepoint_commit() {
    JOURNAL.with(|j| {
        j.borrow_mut().marks.pop();
    });
}

/// Forget everything (transaction end).
pub fn clear() {
    JOURNAL.with(|j| *j.borrow_mut() = Journal::default());
}

/// The net change per row, in order of each row's first entry, and the entities
/// whose changes overflowed the cap. With `reset` the journal is emptied (open
/// savepoint marks are kept at zero so a later rollback stays consistent).
pub fn summarize(reset: bool) -> (Vec<NetChange>, BTreeSet<String>) {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        let net = net_changes(&j.entries);
        let overflow = j.overflow.clone();
        if reset {
            j.entries.clear();
            j.overflow.clear();
            j.marks.fill(0);
        }
        (net, overflow)
    })
}

/// Fold journal entries into one net change per `(entity, pk)`.
///
/// Inserted then deleted in the same span leaves nothing; deleted then inserted is
/// an update; an insert stays an insert across later updates.
fn net_changes(entries: &[(String, String, Change)]) -> Vec<NetChange> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut state: HashMap<(String, String), Option<Change>> = HashMap::new();
    for (entity, pk, change) in entries {
        let key = (entity.clone(), pk.clone());
        let prev = if let Some(prev) = state.get(&key) {
            prev.clone()
        } else {
            order.push(key.clone());
            None
        };
        let next = match (prev, change) {
            (None, c) => Some(c.clone()),
            (Some(Change::Inserted), Change::Deleted(_)) => None,
            (Some(Change::Inserted), _) => Some(Change::Inserted),
            (Some(Change::Deleted(_)), Change::Inserted | Change::Updated) => Some(Change::Updated),
            (Some(_), c) => Some(c.clone()),
        };
        state.insert(key, next);
    }
    order
        .into_iter()
        .filter_map(|key| {
            let change = state.remove(&key).flatten()?;
            Some(NetChange {
                entity: key.0,
                pk: key.1,
                change,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Change, net_changes};

    fn e(entity: &str, pk: &str, c: Change) -> (String, String, Change) {
        (entity.to_string(), pk.to_string(), c)
    }

    fn net(entries: &[(String, String, Change)]) -> Vec<(String, String, Change)> {
        net_changes(entries)
            .into_iter()
            .map(|n| (n.entity, n.pk, n.change))
            .collect()
    }

    #[test]
    fn keeps_first_seen_order_and_folds_repeats() {
        let got = net(&[
            e("post", "2", Change::Updated),
            e("user", "1", Change::Updated),
            e("post", "2", Change::Updated),
        ]);
        assert_eq!(
            got,
            [
                e("post", "2", Change::Updated),
                e("user", "1", Change::Updated)
            ]
        );
    }

    #[test]
    fn insert_then_delete_cancels_and_delete_then_insert_is_update() {
        let got = net(&[
            e("post", "1", Change::Inserted),
            e("post", "1", Change::Updated),
            e("post", "1", Change::Deleted(Some("a".into()))),
            e("post", "2", Change::Deleted(Some("b".into()))),
            e("post", "2", Change::Inserted),
            e("post", "3", Change::Inserted),
            e("post", "3", Change::Updated),
        ]);
        assert_eq!(
            got,
            [
                e("post", "2", Change::Updated),
                e("post", "3", Change::Inserted)
            ]
        );
    }

    #[test]
    fn update_then_delete_is_delete() {
        let got = net(&[
            e("post", "1", Change::Updated),
            e("post", "1", Change::Deleted(Some("a".into()))),
        ]);
        assert_eq!(got, [e("post", "1", Change::Deleted(Some("a".into())))]);
    }
}
