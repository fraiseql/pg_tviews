//! The per-transaction set of changed TVIEW rows that commit publishes
//! (`pg_tviews.publish_changes`, ADR 0231).
//!
//! It is fed by the same writes as the report's journal ([`super::affected`]) but
//! kept apart from it: the report resets and caps its journal as it reports, and
//! publishing needs everything the transaction changed. Entries are appended in
//! write order; a savepoint records the position and a rollback to it truncates
//! back, so a rolled-back write is never published. At commit the entries fold
//! into one net change per row ([`fold`]).
//!
//! Memory is bounded by the bytes of the ids kept: past [`KEPT_BYTES`], a type's
//! further rows are not kept and the type is published by name only. A row
//! without an id (a TVIEW without an `id` column) is published by its type too.
//! In `types` mode no id is kept at all.

use super::affected::{Change, NetChange, Recorded};
use std::cell::RefCell;
use std::collections::BTreeSet;

/// The bytes of ids a transaction keeps for publishing: twice what the messages
/// carry ([`crate::publish::MAX_MESSAGES`] × [`crate::publish::MESSAGE_BYTES`]),
/// since entries fold (a row written twice, inserted then deleted) before they
/// are encoded.
pub const KEPT_BYTES: usize = 2 * crate::publish::MAX_MESSAGES * crate::publish::MESSAGE_BYTES;

/// What a transaction publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publish {
    /// Changed rows, by id.
    Rows,
    /// Changed types only.
    Types,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Set {
    entries: Vec<Recorded>,
    /// The bytes of the ids in `entries`.
    bytes: usize,
    /// Types published by name only.
    types: BTreeSet<String>,
}

thread_local! {
    static SET: RefCell<Set> = RefCell::new(Set::default());
}

/// Where the set stood when a subtransaction started.
#[derive(Debug)]
pub struct Mark {
    position: usize,
    bytes: usize,
    types: BTreeSet<String>,
}

/// Keep a write for publishing, as `publish` asks.
pub fn add(publish: Publish, recorded: &Recorded) {
    SET.with_borrow_mut(|set| {
        if set.types.contains(&recorded.typename) {
            return;
        }
        match (publish, &recorded.id) {
            (Publish::Rows, Some(id)) if set.bytes + id.len() <= KEPT_BYTES => {
                set.bytes += id.len();
                set.entries.push(recorded.clone());
            }
            _ => {
                set.types.insert(recorded.typename.clone());
            }
        }
    });
}

/// A subtransaction started: where the set stands.
pub fn mark() -> Mark {
    SET.with_borrow(|set| Mark {
        position: set.entries.len(),
        bytes: set.bytes,
        types: set.types.clone(),
    })
}

/// The subtransaction of `mark` rolled back: forget what it added.
pub fn rollback(mark: Mark) {
    SET.with_borrow_mut(|set| {
        set.entries.truncate(mark.position);
        set.bytes = mark.bytes;
        set.types = mark.types;
    });
}

/// Forget everything (transaction end).
pub fn clear() {
    SET.with_borrow_mut(|set| *set = Set::default());
}

/// Whether the transaction has anything to publish.
pub fn is_empty() -> bool {
    SET.with_borrow(|set| set.entries.is_empty() && set.types.is_empty())
}

/// The transaction's changes, folded: what commit publishes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folded {
    /// Rows inserted or updated, as `(type, ids)`, types in name order, ids in the
    /// order of each row's first change.
    pub upserted: Vec<(String, Vec<String>)>,
    /// Rows deleted, likewise.
    pub deleted: Vec<(String, Vec<String>)>,
    /// Types published by name only.
    pub types: BTreeSet<String>,
}

/// The set, folded into one net change per row.
pub fn fold() -> Folded {
    SET.with_borrow(|set| fold_entries(&set.entries, &set.types))
}

fn fold_entries(entries: &[Recorded], types: &BTreeSet<String>) -> Folded {
    let mut folded = Folded {
        types: types.clone(),
        ..Folded::default()
    };
    let net: Vec<NetChange> = super::affected::net_changes(entries);
    for change in net {
        // A type published by name covers its rows.
        if folded.types.contains(&change.typename) {
            continue;
        }
        let Some(id) = change.id else {
            folded.types.insert(change.typename);
            continue;
        };
        let lists = if change.change == Change::Deleted {
            &mut folded.deleted
        } else {
            &mut folded.upserted
        };
        match lists.iter_mut().find(|(t, _)| *t == change.typename) {
            Some((_, ids)) => ids.push(id),
            None => lists.push((change.typename, vec![id])),
        }
    }
    folded.upserted.sort_by(|a, b| a.0.cmp(&b.0));
    folded.deleted.sort_by(|a, b| a.0.cmp(&b.0));
    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(typename: &str, pk: &str, id: Option<&str>, change: Change) -> Recorded {
        Recorded {
            entity: typename.to_lowercase(),
            typename: typename.to_string(),
            pk: pk.to_string(),
            id: id.map(str::to_string),
            change,
        }
    }

    fn ids(lists: &[(String, Vec<String>)]) -> Vec<(&str, Vec<&str>)> {
        lists
            .iter()
            .map(|(t, ids)| (t.as_str(), ids.iter().map(String::as_str).collect()))
            .collect()
    }

    #[test]
    fn rows_fold_to_net_changes_by_type() {
        clear();
        add(
            Publish::Rows,
            &r("User", "1", Some("\"u1\""), Change::Updated),
        );
        add(
            Publish::Rows,
            &r("Post", "1", Some("\"p1\""), Change::Inserted),
        );
        add(
            Publish::Rows,
            &r("Post", "1", Some("\"p1\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", "2", Some("\"p2\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", "3", Some("\"p3\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", "3", Some("\"p3\""), Change::Inserted),
        );
        let folded = fold();
        assert_eq!(
            ids(&folded.upserted),
            [("Post", vec!["\"p3\""]), ("User", vec!["\"u1\""])]
        );
        assert_eq!(ids(&folded.deleted), [("Post", vec!["\"p2\""])]);
        assert!(folded.types.is_empty());
        clear();
    }

    #[test]
    fn a_row_without_id_publishes_its_type() {
        clear();
        add(Publish::Rows, &r("Tag", "1", None, Change::Updated));
        add(
            Publish::Rows,
            &r("User", "1", Some("\"u1\""), Change::Updated),
        );
        let folded = fold();
        assert_eq!(folded.types.iter().collect::<Vec<_>>(), ["Tag"]);
        assert_eq!(ids(&folded.upserted), [("User", vec!["\"u1\""])]);
        clear();
    }

    #[test]
    fn types_mode_keeps_no_ids() {
        clear();
        add(
            Publish::Types,
            &r("User", "1", Some("\"u1\""), Change::Updated),
        );
        add(
            Publish::Types,
            &r("Post", "1", Some("\"p1\""), Change::Deleted),
        );
        SET.with_borrow(|set| assert!(set.entries.is_empty()));
        assert_eq!(fold().types.iter().collect::<Vec<_>>(), ["Post", "User"]);
        clear();
    }

    #[test]
    fn past_the_byte_budget_a_type_is_kept_by_name() {
        clear();
        let id = format!("\"{}\"", "x".repeat(36));
        for pk in 0..=KEPT_BYTES / id.len() {
            add(
                Publish::Rows,
                &r("User", &pk.to_string(), Some(&id), Change::Updated),
            );
        }
        SET.with_borrow(|set| assert!(set.bytes <= KEPT_BYTES));
        let folded = fold();
        assert!(folded.upserted.is_empty(), "the type's ids are not partial");
        assert_eq!(folded.types.iter().collect::<Vec<_>>(), ["User"]);
        clear();
    }

    #[test]
    fn a_rollback_restores_rows_and_types_a_release_keeps_them() {
        clear();
        add(
            Publish::Rows,
            &r("User", "1", Some("\"u1\""), Change::Updated),
        );
        let outer = mark();
        add(Publish::Rows, &r("Tag", "1", None, Change::Updated));
        let inner = mark();
        add(
            Publish::Rows,
            &r("Post", "1", Some("\"p1\""), Change::Inserted),
        );
        rollback(inner);
        let folded = fold();
        assert_eq!(ids(&folded.upserted), [("User", vec!["\"u1\""])]);
        assert_eq!(folded.types.iter().collect::<Vec<_>>(), ["Tag"]);
        rollback(outer);
        let folded = fold();
        assert!(folded.types.is_empty());
        assert_eq!(ids(&folded.upserted), [("User", vec!["\"u1\""])]);
        assert!(!is_empty());
        clear();
        assert!(is_empty());
    }
}
