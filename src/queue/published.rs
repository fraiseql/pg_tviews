//! The per-transaction set of changed TVIEW rows that commit publishes
//! (`pg_tviews.publish_changes`, ADR 0231).
//!
//! It is fed by the same writes as the report's journal ([`super::affected`]) but
//! kept apart from it: the report resets and caps its journal as it reports, and
//! publishing needs everything the transaction changed. Entries are appended in
//! write order; a savepoint records the position and a rollback to it truncates
//! back, so a rolled-back write is never published. At commit the entries fold
//! into one net change per row ([`with_folded`]), a row being its type and public
//! id.
//!
//! Memory is bounded by the bytes of the ids kept: past [`KEPT_BYTES`], a type's
//! further rows are not kept and the type is published by name only. A row
//! without an id (a TVIEW without an `id` column) is published by its type too.
//! In `types` mode no id is kept at all. Type names are shared, not copied.

use super::affected::{Change, Recorded, fold_change};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    typename: Rc<str>,
    /// The row's id as JSON text.
    id: String,
    change: Change,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Set {
    entries: Vec<Entry>,
    /// The bytes of the ids in `entries`.
    bytes: usize,
    /// Types published by name only.
    types: BTreeSet<Rc<str>>,
}

thread_local! {
    static SET: RefCell<Set> = RefCell::new(Set::default());
}

/// Where the set stood when a subtransaction started.
#[derive(Debug)]
pub struct Mark {
    position: usize,
    bytes: usize,
    types: BTreeSet<Rc<str>>,
}

/// Keep a write for publishing, as `publish` asks.
pub fn add(publish: Publish, recorded: &Recorded) {
    SET.with_borrow_mut(|set| {
        if set.types.contains(&recorded.typename) {
            return;
        }
        let id = match (publish, &recorded.id) {
            (Publish::Rows, Some(id)) => Some(id.json()),
            _ => None,
        };
        match id {
            Some(id) if set.bytes + id.len() <= KEPT_BYTES => {
                set.bytes += id.len();
                set.entries.push(Entry {
                    typename: Rc::clone(&recorded.typename),
                    id,
                    change: recorded.change,
                });
            }
            _ => {
                set.types.insert(Rc::clone(&recorded.typename));
            }
        }
    });
}

/// Publish `typename` by name: its rows changed without being listed (a rebuild).
pub fn add_type(typename: &Rc<str>) {
    SET.with_borrow_mut(|set| {
        set.types.insert(Rc::clone(typename));
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

/// The transaction's changes, folded: what commit publishes, borrowed from the set.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Folded<'a> {
    /// Rows inserted or updated, as `(type, ids)`, types in name order, ids in the
    /// order of each row's first change.
    pub upserted: Vec<(&'a str, Vec<&'a str>)>,
    /// Rows deleted, likewise.
    pub deleted: Vec<(&'a str, Vec<&'a str>)>,
    /// Types published by name only.
    pub types: BTreeSet<&'a str>,
}

/// `f` of the set, folded into one net change per row.
pub fn with_folded<T>(f: impl FnOnce(&Folded) -> T) -> T {
    SET.with_borrow(|set| f(&fold(set)))
}

fn fold(set: &Set) -> Folded<'_> {
    let types: BTreeSet<&str> = set.types.iter().map(|t| &**t).collect();
    let mut order: Vec<(&str, &str)> = Vec::new();
    let mut net: HashMap<(&str, &str), Option<Change>> = HashMap::new();
    for entry in &set.entries {
        let row = (&*entry.typename, entry.id.as_str());
        // A type published by name covers its rows.
        if types.contains(row.0) {
            continue;
        }
        let prev = if let Some(prev) = net.get(&row) {
            *prev
        } else {
            order.push(row);
            None
        };
        net.insert(row, fold_change(prev, entry.change));
    }
    let mut folded = Folded {
        types,
        ..Folded::default()
    };
    for row in order {
        let Some(change) = net.get(&row).copied().flatten() else {
            continue;
        };
        let lists = if change == Change::Deleted {
            &mut folded.deleted
        } else {
            &mut folded.upserted
        };
        match lists.iter_mut().find(|(t, _)| *t == row.0) {
            Some((_, ids)) => ids.push(row.1),
            None => lists.push((row.0, vec![row.1])),
        }
    }
    folded.upserted.sort_by_key(|(t, _)| *t);
    folded.deleted.sort_by_key(|(t, _)| *t);
    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(typename: &str, pk: &str, id: Option<&str>, change: Change) -> Recorded {
        Recorded {
            entity: typename.to_lowercase(),
            typename: typename.into(),
            pk: pk.to_string(),
            // Already JSON text: kept as is.
            id: id.map(|text| crate::catalog::RowId {
                text: text.to_string(),
                kind: crate::catalog::PublicId::Number,
            }),
            change,
        }
    }

    fn owned(lists: &[(&str, Vec<&str>)]) -> Vec<(String, Vec<String>)> {
        lists
            .iter()
            .map(|(t, ids)| {
                (
                    (*t).to_string(),
                    ids.iter().map(|i| (*i).to_string()).collect(),
                )
            })
            .collect()
    }

    type Snapshot = (
        Vec<(String, Vec<String>)>,
        Vec<(String, Vec<String>)>,
        Vec<String>,
    );

    fn snapshot() -> Snapshot {
        with_folded(|f| {
            (
                owned(&f.upserted),
                owned(&f.deleted),
                f.types.iter().map(|t| (*t).to_string()).collect(),
            )
        })
    }

    fn list(t: &str, ids: &[&str]) -> (String, Vec<String>) {
        (
            t.to_string(),
            ids.iter().map(|i| (*i).to_string()).collect(),
        )
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
        let (upserted, deleted, types) = snapshot();
        assert_eq!(
            upserted,
            [list("Post", &["\"p3\""]), list("User", &["\"u1\""])]
        );
        assert_eq!(deleted, [list("Post", &["\"p2\""])]);
        assert!(types.is_empty());
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
        let (upserted, _, types) = snapshot();
        assert_eq!(types, ["Tag"]);
        assert_eq!(upserted, [list("User", &["\"u1\""])]);
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
        assert_eq!(snapshot().2, ["Post", "User"]);
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
        let (upserted, _, types) = snapshot();
        assert!(upserted.is_empty(), "the type's ids are not partial");
        assert_eq!(types, ["User"]);
        clear();
    }

    #[test]
    fn a_rollback_restores_rows_and_types() {
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
        let (upserted, _, types) = snapshot();
        assert_eq!(upserted, [list("User", &["\"u1\""])]);
        assert_eq!(types, ["Tag"]);
        rollback(outer);
        let (upserted, _, types) = snapshot();
        assert!(types.is_empty());
        assert_eq!(upserted, [list("User", &["\"u1\""])]);
        assert!(!is_empty());
        clear();
        assert!(is_empty());
    }
}
