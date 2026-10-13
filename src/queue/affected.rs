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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Inserted,
    Updated,
    Deleted,
}

/// One refresh write to one TVIEW row, captured by the write itself while SPI is
/// available: nothing about it has to be looked up later (at `PRE_COMMIT`, none
/// can).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub entity: String,
    /// The GraphQL type name of the row ([`crate::catalog::typename`]).
    pub typename: String,
    /// `pk_<entity>`, as text.
    pub pk: String,
    /// The row's public `id` as JSON text (`"…"` for a uuid); `None` for a TVIEW
    /// without an `id` column.
    pub id: Option<String>,
    pub change: Change,
}

impl Recorded {
    /// `meta`'s row `pk`, with public `id`, changed by `change`.
    #[must_use]
    pub fn of(
        meta: &crate::catalog::TviewMeta,
        pk: String,
        id: Option<String>,
        change: Change,
    ) -> Self {
        Self {
            entity: meta.entity_name.clone(),
            typename: meta.typename.clone(),
            pk,
            id,
            change,
        }
    }
}

/// One row's net change over the journal: its last id, in the position of its
/// first entry.
pub type NetChange = Recorded;

#[derive(Default)]
struct Journal {
    entries: Vec<Recorded>,
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

/// Record that a refresh write changed a TVIEW row.
pub fn record(recorded: Recorded) {
    let counter = match recorded.change {
        Change::Deleted => crate::stats::Counter::RowsDeleted,
        _ => crate::stats::Counter::RowsWritten,
    };
    crate::stats::add(&recorded.entity, counter, 1);
    FLUSH_CHANGED.with(|c| {
        c.borrow_mut()
            .insert((recorded.entity.clone(), recorded.pk.clone()))
    });
    if let Some(publish) = crate::config::publish_changes() {
        super::published::add(publish, &recorded);
    }
    let cap = crate::config::report_max_tracked();
    if cap == 0 {
        return;
    }
    JOURNAL.with(|j| {
        let mut j = j.borrow_mut();
        if j.base + j.entries.len() - j.reported < cap {
            j.entries.push(recorded);
        } else {
            j.overflow.insert(recorded.entity);
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

/// Fold journal entries into one net change per `(entity, pk)`, carrying the last
/// id seen for the row.
///
/// Inserted then deleted in the same span leaves nothing; deleted then inserted is
/// an update; an insert stays an insert across later updates.
pub fn net_changes(entries: &[Recorded]) -> Vec<NetChange> {
    let mut order: Vec<(String, String)> = Vec::new();
    let mut state: HashMap<(String, String), (Option<Change>, &Recorded)> = HashMap::new();
    for entry in entries {
        let key = (entry.entity.clone(), entry.pk.clone());
        let prev = if let Some((prev, _)) = state.get(&key) {
            *prev
        } else {
            order.push(key.clone());
            None
        };
        let next = match (prev, entry.change) {
            (None, c) => Some(c),
            (Some(Change::Inserted), Change::Deleted) => None,
            (Some(Change::Inserted), _) => Some(Change::Inserted),
            (Some(Change::Deleted), Change::Inserted | Change::Updated) => Some(Change::Updated),
            (Some(_), c) => Some(c),
        };
        let last = match state.get(&key) {
            Some((_, last)) if entry.id.is_none() => last,
            _ => entry,
        };
        state.insert(key, (next, last));
    }
    order
        .into_iter()
        .filter_map(|key| {
            let (change, last) = state.remove(&key)?;
            Some(Recorded {
                change: change?,
                ..last.clone()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Change, JOURNAL, Recorded, clear, mark, net_changes, release, rollback, summarize,
    };

    /// Journal a change without `record`, which reads a GUC.
    fn push(pk: &str) {
        JOURNAL.with(|j| {
            j.borrow_mut().entries.push(e("user", pk, Change::Updated));
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

    fn with_id(entity: &str, pk: &str, id: Option<&str>, change: Change) -> Recorded {
        Recorded {
            entity: entity.to_string(),
            typename: entity.to_uppercase(),
            pk: pk.to_string(),
            id: id.map(str::to_string),
            change,
        }
    }

    fn e(entity: &str, pk: &str, c: Change) -> Recorded {
        with_id(entity, pk, None, c)
    }

    fn net(entries: &[Recorded]) -> Vec<(String, String, Change)> {
        net_changes(entries)
            .into_iter()
            .map(|n| (n.entity, n.pk, n.change))
            .collect()
    }

    fn ids(entries: &[Recorded]) -> Vec<(String, Option<String>, Change)> {
        net_changes(entries)
            .into_iter()
            .map(|n| (n.pk, n.id, n.change))
            .collect()
    }

    #[test]
    fn the_net_change_keeps_the_last_id_and_the_type_name() {
        let got = net_changes(&[
            with_id("post", "1", Some("a"), Change::Inserted),
            with_id("post", "1", Some("b"), Change::Updated),
            with_id("post", "2", Some("c"), Change::Updated),
            with_id("post", "2", None, Change::Updated),
        ]);
        assert_eq!(
            got.iter()
                .map(|n| (
                    n.pk.as_str(),
                    n.id.as_deref(),
                    n.change,
                    n.typename.as_str()
                ))
                .collect::<Vec<_>>(),
            [
                ("1", Some("b"), Change::Inserted, "POST"),
                ("2", Some("c"), Change::Updated, "POST")
            ]
        );
    }

    #[test]
    fn a_delete_after_an_update_keeps_the_deletes_id() {
        assert_eq!(
            ids(&[
                with_id("post", "1", Some("old"), Change::Updated),
                with_id("post", "1", Some("gone"), Change::Deleted),
            ]),
            [("1".to_string(), Some("gone".to_string()), Change::Deleted)]
        );
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
                ("post".to_string(), "2".to_string(), Change::Updated),
                ("user".to_string(), "1".to_string(), Change::Updated)
            ]
        );
    }

    #[test]
    fn insert_then_delete_cancels_and_delete_then_insert_is_update() {
        let got = net(&[
            e("post", "1", Change::Inserted),
            e("post", "1", Change::Updated),
            e("post", "1", Change::Deleted),
            e("post", "2", Change::Deleted),
            e("post", "2", Change::Inserted),
            e("post", "3", Change::Inserted),
            e("post", "3", Change::Updated),
        ]);
        assert_eq!(
            got,
            [
                ("post".to_string(), "2".to_string(), Change::Updated),
                ("post".to_string(), "3".to_string(), Change::Inserted)
            ]
        );
    }

    #[test]
    fn update_then_delete_is_delete() {
        let got = net(&[
            e("post", "1", Change::Updated),
            e("post", "1", Change::Deleted),
        ]);
        assert_eq!(
            got,
            [("post".to_string(), "1".to_string(), Change::Deleted)]
        );
    }
}
