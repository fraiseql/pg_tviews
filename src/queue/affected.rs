//! Per-transaction journal of the TVIEW rows refreshes actually changed.
//!
//! Every refresh write (guarded upsert, delete, direct patch) records the keys it
//! really inserted, updated or deleted: a refresh that found nothing to change
//! records nothing. The statement-level flush trigger flushes after every
//! statement, so this journal, not the queue, is what still knows at the end of a
//! mutation function which read-model rows the transaction changed.
//! [`crate::report::pg_tviews_flush_and_report`] reads it.
//!
//! Entries are appended in write order. A savepoint records the journal position and
//! a rollback to it truncates back, so rolled-back writes are never reported. A
//! report with reset hides what it reported; inside a subtransaction the entries are
//! kept until no subtransaction is open, so rolling one back undoes its resets. The
//! journal is cleared when the transaction ends. Past `pg_tviews.report_max_tracked`
//! unreported entries only the entity names are kept and the report says it was
//! truncated.

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
    /// Entries dropped before `entries[0]`: positions count from the start of the
    /// transaction, so a savepoint taken before a reset still rolls back exactly
    /// what came after it.
    base: usize,
    /// The position of the first entry not yet reported (with reset). Entries
    /// before it are dropped once no subtransaction is open.
    reported: usize,
    /// Entities with changes that were not journaled because the cap was reached.
    overflow: BTreeSet<String>,
    /// Subtransactions open.
    open: usize,
}

/// Where the journal stood when a subtransaction started.
#[derive(Debug)]
pub struct Mark {
    position: usize,
    reported: usize,
    overflow: BTreeSet<String>,
}

impl Journal {
    /// Drop the reported entries, unless a subtransaction could still undo the
    /// reset that reported them.
    fn compact(&mut self) {
        if self.open == 0 {
            self.entries.drain(..self.reported - self.base);
            self.base = self.reported;
        }
    }
}

thread_local! {
    static JOURNAL: RefCell<Journal> = RefCell::new(Journal::default());
    /// Rows changed during the current flush, uncapped: propagation reads it to
    /// skip parents of rows whose refresh changed nothing.
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
    let counter = match change {
        Change::Deleted(_) => crate::stats::Counter::RowsDeleted,
        _ => crate::stats::Counter::RowsWritten,
    };
    crate::stats::add(entity, counter, 1);
    FLUSH_CHANGED.with(|c| c.borrow_mut().insert((entity.to_string(), pk.clone())));
    let cap = crate::config::report_max_tracked();
    if cap == 0 {
        return;
    }
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        if j.base + j.entries.len() - j.reported < cap {
            j.entries.push((entity.to_string(), pk, change));
        } else {
            j.overflow.insert(entity.to_string());
        }
    });
}

/// A subtransaction started: where the journal stands.
pub fn mark() -> Mark {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        j.open += 1;
        Mark {
            position: j.base + j.entries.len(),
            reported: j.reported,
            overflow: j.overflow.clone(),
        }
    })
}

/// The subtransaction of `mark` committed: its entries and resets belong to its
/// parent.
pub fn release(_mark: Mark) {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        j.open = j.open.saturating_sub(1);
        j.compact();
    });
}

/// The subtransaction of `mark` rolled back: forget what was journaled since, and
/// undo the resets made since.
pub fn rollback(mark: Mark) {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        j.open = j.open.saturating_sub(1);
        let keep = mark.position - j.base;
        j.entries.truncate(keep);
        j.reported = mark.reported;
        j.overflow.extend(mark.overflow);
        j.compact();
    });
}

/// Forget everything (transaction end).
pub fn clear() {
    JOURNAL.with(|j| *j.borrow_mut() = Journal::default());
}

/// The net change per unreported row, in order of each row's first entry, and the
/// entities whose changes overflowed the cap. With `reset` they count as reported.
pub fn summarize(reset: bool) -> (Vec<NetChange>, BTreeSet<String>) {
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        let net = net_changes(&j.entries[j.reported - j.base..]);
        let overflow = j.overflow.clone();
        if reset {
            j.reported = j.base + j.entries.len();
            j.overflow.clear();
            j.compact();
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
    use super::{Change, JOURNAL, clear, mark, net_changes, release, rollback, summarize};

    /// Journal a change without `record`, which reads a GUC.
    fn push(pk: &str) {
        JOURNAL.with(|j| {
            j.borrow_mut()
                .entries
                .push(("user".to_string(), pk.to_string(), Change::Updated));
        });
    }

    fn report(reset: bool) -> Vec<String> {
        summarize(reset).0.into_iter().map(|n| n.pk).collect()
    }

    #[test]
    fn a_rolled_back_reset_is_undone() {
        clear();
        push("1");
        let m = mark();
        assert_eq!(report(true), ["1"]);
        push("2");
        rollback(m);
        assert_eq!(report(true), ["1"]);
        assert!(report(false).is_empty());
        clear();
    }

    #[test]
    fn a_released_reset_holds_until_an_outer_rollback() {
        clear();
        push("1");
        let outer = mark();
        let inner = mark();
        assert_eq!(report(true), ["1"]);
        release(inner);
        assert!(report(false).is_empty());
        rollback(outer);
        assert_eq!(report(false), ["1"]);
        clear();
    }

    #[test]
    fn reported_entries_are_dropped_once_no_subtransaction_is_open() {
        clear();
        push("1");
        let m = mark();
        report(true);
        release(m);
        push("2");
        assert_eq!(report(true), ["2"]);
        JOURNAL.with(|j| assert!(j.borrow().entries.is_empty()));
        clear();
    }

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
