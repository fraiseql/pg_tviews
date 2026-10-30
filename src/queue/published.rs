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
//!
//! A written row of a TVIEW that references others also names the collections
//! it belongs to, before and after the write: `Comment.fk_post` and the post.
//! A recompute returns the parent's public id; a patch returns its key, which the
//! flush resolves ([`resolve`]) while SPI is available, from the rows the
//! transaction wrote or by query. A key still unresolved at commit (a write
//! outside a flush) publishes the child type by name, and so does a parent
//! without an `id` column. A parent the flush no longer finds (deleted, or never
//! a row of its TVIEW) names nothing: its own row is published, or no cached
//! result holds it. Each collection's parents are kept once.

use super::affected::{Change, Recorded, fold_change};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
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
    /// Its `pk_<entity>`: a child naming it as a parent finds its id here.
    pk: i64,
    change: Change,
}

/// A collection a written row belongs to, and its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Member {
    collection: Rc<crate::catalog::Collection>,
    /// The parent's key.
    key: String,
    parent: Parent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Parent {
    /// Not resolved yet: the write returned the parent's key only.
    Key,
    /// The parent's public id as JSON text.
    Id(String),
    /// No row of the parent TVIEW holds the key.
    Gone,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Set {
    entries: Vec<Entry>,
    /// The bytes of the ids in `entries` and of the keys and ids in `members`.
    bytes: usize,
    /// Types published by name only.
    types: BTreeSet<Rc<str>>,
    /// Each collection's parents once: a parent named again is not kept twice.
    members: Vec<Member>,
    /// `members` before this position are resolved.
    resolved: usize,
    /// `(collection key, parent key)` of every member.
    seen: HashSet<(Rc<str>, String)>,
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
    members: usize,
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
                    pk: recorded.pk,
                    change: recorded.change,
                });
            }
            _ => {
                set.types.insert(Rc::clone(&recorded.typename));
                return;
            }
        }
        for (collection, parent) in &recorded.parents {
            let child = &collection.child_type;
            if set.types.contains(child) {
                continue;
            }
            let Some(kind) = collection.parent_id else {
                set.types.insert(Rc::clone(child));
                continue;
            };
            let resolved = parent.id.as_ref().map(|id| {
                crate::catalog::RowId {
                    text: id.clone(),
                    kind,
                }
                .json()
            });
            let bytes = parent.key.len() + resolved.as_ref().map_or(0, String::len);
            if set.bytes + bytes > KEPT_BYTES {
                set.types.insert(Rc::clone(child));
                continue;
            }
            if !set
                .seen
                .insert((Rc::clone(&collection.key), parent.key.clone()))
            {
                continue;
            }
            set.bytes += bytes;
            set.members.push(Member {
                collection: Rc::clone(collection),
                key: parent.key.clone(),
                parent: resolved.map_or(Parent::Key, Parent::Id),
            });
        }
    });
}

/// Resolve the parents' keys the writes since the last call returned to their
/// public ids: those the transaction wrote itself from its own rows, the others
/// with one query for every parent TVIEW. Runs at the end of a flush.
///
/// # Errors
/// What the query returns.
pub fn resolve() -> crate::TViewResult<()> {
    let pending = SET.with_borrow_mut(|set| {
        let pending = set.members[set.resolved..]
            .iter()
            .any(|m| m.parent == Parent::Key);
        if !pending {
            set.resolved = set.members.len();
        }
        pending
    });
    if !pending {
        return Ok(());
    }
    let lookups = SET.with_borrow(|set| {
        let written = written_ids(set);
        let mut lookups: Vec<(Rc<crate::catalog::Collection>, Vec<String>)> = Vec::new();
        for member in &set.members[set.resolved..] {
            let c = &member.collection;
            if member.parent != Parent::Key || own_id(&written, c, &member.key).is_some() {
                continue;
            }
            let at = lookups
                .iter()
                .position(|(other, _)| other.parent_table == c.parent_table)
                .unwrap_or_else(|| {
                    lookups.push((Rc::clone(c), Vec::new()));
                    lookups.len() - 1
                });
            if !lookups[at].1.contains(&member.key) {
                lookups[at].1.push(member.key.clone());
            }
        }
        lookups
    });
    let found = if lookups.is_empty() {
        HashMap::new()
    } else {
        parent_ids(&lookups)?
    };
    settle(&|collection, key| {
        found
            .get(&(collection.parent_table.clone(), key.to_string()))
            .cloned()
    });
    Ok(())
}

/// The public ids, as JSON text, of the rows the transaction published, by
/// `(type, pk_<entity>)`.
fn written_ids(set: &Set) -> HashMap<(&str, i64), &str> {
    set.entries
        .iter()
        .map(|e| ((&*e.typename, e.pk), e.id.as_str()))
        .collect()
}

/// The id of parent `key` of `collection` among the rows the transaction wrote:
/// only a collection keyed by the parent's `pk_<entity>` can name one (any other
/// reference value is not a journal key).
fn own_id<'a>(
    written: &HashMap<(&str, i64), &'a str>,
    collection: &crate::catalog::Collection,
    key: &str,
) -> Option<&'a str> {
    if !collection.parent_keyed_by_pk {
        return None;
    }
    let pk = key.parse().ok()?;
    written.get(&(&*collection.parent_typename, pk)).copied()
}

/// The public ids, as JSON text, of the parents of `lookups` by
/// `(parent table, key)`: one statement per owner of parent TVIEWs, run as that
/// owner as every refresh read is, with a branch per parent TVIEW.
fn parent_ids(
    lookups: &[(Rc<crate::catalog::Collection>, Vec<String>)],
) -> crate::TViewResult<HashMap<(String, String), String>> {
    let mut by_owner: Vec<(pgrx::pg_sys::Oid, Vec<usize>)> = Vec::new();
    for (i, (c, _)) in lookups.iter().enumerate() {
        let owner = crate::owner::relation_owner(c.parent_table_oid)?;
        match by_owner.iter_mut().find(|(o, _)| *o == owner) {
            Some((_, at)) => at.push(i),
            None => by_owner.push((owner, vec![i])),
        }
    }
    let mut found = HashMap::new();
    for (owner, at) in by_owner {
        let _owner = crate::owner::AsOwner::of_role(owner);
        parent_ids_of(lookups, &at, &mut found)?;
    }
    Ok(found)
}

/// [`parent_ids`] of the lookups at `at`, into `found`.
fn parent_ids_of(
    lookups: &[(Rc<crate::catalog::Collection>, Vec<String>)],
    at: &[usize],
    found: &mut HashMap<(String, String), String>,
) -> crate::TViewResult<()> {
    use crate::utils::ident::quoted;
    // An integer key is bound as int8[]; any other as text, cast to its type.
    let integer = |c: &crate::catalog::Collection| {
        matches!(c.parent_type.as_str(), "smallint" | "integer" | "bigint")
    };
    let branches: Vec<String> = at
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let c = &lookups[i].0;
            let key = quoted(&c.parent_column);
            let keys = if integer(c) {
                format!("${}::pg_catalog.int8[]", n + 1)
            } else {
                format!("${}::pg_catalog.text[]::{}[]", n + 1, c.parent_type)
            };
            format!(
                "SELECT {i}, p.{key}::pg_catalog.text, p.\"id\"::pg_catalog.text FROM {} p \
                 WHERE p.{key} OPERATOR(pg_catalog.=) ANY ({keys})",
                c.parent_table
            )
        })
        .collect();
    let args: Vec<pgrx::datum::DatumWithOid> = at
        .iter()
        .map(|&i| {
            let (c, keys) = &lookups[i];
            if integer(c) {
                crate::utils::spi::int8_array(keys.iter().filter_map(|k| k.parse().ok()).collect())
            } else {
                crate::utils::spi::text_array(keys.clone())
            }
        })
        .collect();
    for row in crate::utils::spi::kept_rows(
        &branches.join(" UNION ALL "),
        &args,
        crate::utils::spi::Plan::Generic,
    )? {
        let [Some(i), Some(key), Some(id)] = &row[..] else {
            continue;
        };
        let Some((c, _)) = i.parse::<usize>().ok().and_then(|i| lookups.get(i)) else {
            continue;
        };
        let id = crate::catalog::RowId {
            text: id.clone(),
            kind: c.parent_id.unwrap_or(crate::catalog::PublicId::String),
        };
        found.insert((c.parent_table.clone(), key.clone()), id.json());
    }
    Ok(())
}

/// Resolve every unresolved parent key: from the rows the transaction wrote, else
/// with `found`, `(collection, key)` → its public id as JSON text, `None` when no
/// parent row holds the key.
fn settle(found: &dyn Fn(&crate::catalog::Collection, &str) -> Option<String>) {
    SET.with_borrow_mut(|set| {
        let resolved: Vec<(usize, Option<String>)> = {
            let written = written_ids(set);
            set.members[set.resolved..]
                .iter()
                .enumerate()
                .filter(|(_, m)| m.parent == Parent::Key)
                .map(|(i, m)| {
                    let id = own_id(&written, &m.collection, &m.key)
                        .map(str::to_string)
                        .or_else(|| found(&m.collection, &m.key));
                    (set.resolved + i, id)
                })
                .collect()
        };
        for (at, id) in resolved {
            let member = &mut set.members[at];
            member.parent = match id {
                Some(id) if set.bytes + id.len() <= KEPT_BYTES => {
                    set.bytes += id.len();
                    Parent::Id(id)
                }
                Some(_) => {
                    set.types.insert(Rc::clone(&member.collection.child_type));
                    Parent::Gone
                }
                None => Parent::Gone,
            };
        }
        set.resolved = set.members.len();
    });
}

/// How the change feed names a parent a written row named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    /// By its public id, as JSON text.
    Id(String),
    /// Not at all: no row of the parent TVIEW holds the key.
    Gone,
    /// By the child type: the parent has no id column, did not fit the byte
    /// budget, or was never resolved.
    Unlisted,
}

/// How the feed names each of `parents`, the parents written rows named, as far
/// as the last flush resolved them ([`resolve`]): the same capture, read before
/// commit by `pg_tviews_flush_and_report()` (#244).
pub fn listed(
    parents: &[(
        Rc<crate::catalog::Collection>,
        crate::catalog::collections::ParentRef,
    )],
) -> Vec<Listed> {
    SET.with_borrow(|set| {
        let members: HashMap<(&str, &str), &Parent> = set
            .members
            .iter()
            .map(|m| ((&*m.collection.key, m.key.as_str()), &m.parent))
            .collect();
        parents
            .iter()
            .map(|(collection, parent)| {
                if set.types.contains(&collection.child_type) {
                    return Listed::Unlisted;
                }
                match members.get(&(&*collection.key, parent.key.as_str())) {
                    Some(Parent::Id(id)) => Listed::Id(id.clone()),
                    Some(Parent::Gone) => Listed::Gone,
                    Some(Parent::Key) | None => Listed::Unlisted,
                }
            })
            .collect()
    })
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
        members: set.members.len(),
    })
}

/// The subtransaction of `mark` rolled back: forget what it added.
pub fn rollback(mark: Mark) {
    SET.with_borrow_mut(|set| {
        set.entries.truncate(mark.position);
        set.members.truncate(mark.members);
        set.resolved = set.resolved.min(mark.members);
        set.seen = set
            .members
            .iter()
            .map(|m| (Rc::clone(&m.collection.key), m.key.clone()))
            .collect();
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
    SET.with_borrow(|set| set.entries.is_empty() && set.types.is_empty() && set.members.is_empty())
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
    /// The collections changed, as `(collection key, child type, parent ids)`,
    /// keys in name order, ids in the order first named.
    pub collections: Vec<(&'a str, &'a str, Vec<&'a str>)>,
}

/// `f` of the set, folded into one net change per row.
pub fn with_folded<T>(f: impl FnOnce(&Folded) -> T) -> T {
    SET.with_borrow(|set| f(&fold(set)))
}

fn fold(set: &Set) -> Folded<'_> {
    let mut types: BTreeSet<&str> = set.types.iter().map(|t| &**t).collect();
    // A parent never resolved cannot be named: its child type is.
    types.extend(
        set.members
            .iter()
            .filter(|m| m.parent == Parent::Key)
            .map(|m| &*m.collection.child_type),
    );
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
    for member in &set.members {
        let (Parent::Id(id), c) = (&member.parent, &member.collection) else {
            continue;
        };
        if folded.types.contains(&*c.child_type) {
            continue;
        }
        match folded.collections.iter_mut().find(|(k, ..)| *k == &*c.key) {
            Some((.., ids)) if ids.contains(&id.as_str()) => {}
            Some((.., ids)) => ids.push(id),
            None => folded
                .collections
                .push((&*c.key, &*c.child_type, vec![id.as_str()])),
        }
    }
    folded.collections.sort_by_key(|(k, ..)| *k);
    folded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(typename: &str, pk: i64, id: Option<&str>, change: Change) -> Recorded {
        Recorded {
            entity: typename.to_lowercase(),
            typename: typename.into(),
            pk,
            // Already JSON text: kept as is.
            id: id.map(|text| crate::catalog::RowId {
                text: text.to_string(),
                kind: crate::catalog::PublicId::Number,
            }),
            change,
            parents: Vec::new(),
            captured: true,
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
            &r("User", 1, Some("\"u1\""), Change::Updated),
        );
        add(
            Publish::Rows,
            &r("Post", 1, Some("\"p1\""), Change::Inserted),
        );
        add(
            Publish::Rows,
            &r("Post", 1, Some("\"p1\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", 2, Some("\"p2\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", 3, Some("\"p3\""), Change::Deleted),
        );
        add(
            Publish::Rows,
            &r("Post", 3, Some("\"p3\""), Change::Inserted),
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
        add(Publish::Rows, &r("Tag", 1, None, Change::Updated));
        add(
            Publish::Rows,
            &r("User", 1, Some("\"u1\""), Change::Updated),
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
            &r("User", 1, Some("\"u1\""), Change::Updated),
        );
        add(
            Publish::Types,
            &r("Post", 1, Some("\"p1\""), Change::Deleted),
        );
        SET.with_borrow(|set| assert!(set.entries.is_empty()));
        assert_eq!(snapshot().2, ["Post", "User"]);
        clear();
    }

    #[test]
    fn past_the_byte_budget_a_type_is_kept_by_name() {
        clear();
        let id = format!("\"{}\"", "x".repeat(36));
        for pk in 0..=i64::try_from(KEPT_BYTES / id.len()).expect("a small count") {
            add(Publish::Rows, &r("User", pk, Some(&id), Change::Updated));
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
            &r("User", 1, Some("\"u1\""), Change::Updated),
        );
        let outer = mark();
        add(Publish::Rows, &r("Tag", 1, None, Change::Updated));
        let inner = mark();
        add(
            Publish::Rows,
            &r("Post", 1, Some("\"p1\""), Change::Inserted),
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

    fn collection(
        child: &str,
        column: &str,
        parent_id: bool,
    ) -> std::rc::Rc<crate::catalog::Collection> {
        std::rc::Rc::new(crate::catalog::Collection {
            key: format!("{child}.{column}").into(),
            child_type: child.into(),
            column: column.to_string(),
            parent_table: "public.tv_post".to_string(),
            parent_table_oid: pgrx::pg_sys::Oid::from(1),
            parent_column: "pk_post".to_string(),
            parent_type: "integer".to_string(),
            parent_id: parent_id.then_some(crate::catalog::PublicId::String),
            parent_typename: "Post".into(),
            parent_keyed_by_pk: true,
        })
    }

    /// A written child row naming `parents`, `(collection, key, id read by the
    /// write)`.
    fn child(
        pk: i64,
        change: Change,
        parents: &[(&std::rc::Rc<crate::catalog::Collection>, &str, Option<&str>)],
    ) -> Recorded {
        let mut recorded = r("Comment", pk, Some(&format!("\"c{pk}\"")), change);
        recorded.parents = parents
            .iter()
            .map(|(c, key, id)| {
                (
                    std::rc::Rc::clone(c),
                    crate::catalog::collections::ParentRef {
                        key: (*key).to_string(),
                        id: id.map(str::to_string),
                    },
                )
            })
            .collect();
        recorded
    }

    /// Post `key` has public id `p<key>`; post 9 is not in `tv_post`.
    fn settle_posts() {
        settle(&|_, key| (key != "9").then(|| format!("\"p{key}\"")));
    }

    /// Settle with nothing found by query.
    fn settle_none() {
        settle(&|_, _| None);
    }

    fn collections() -> Vec<(String, String, Vec<String>)> {
        with_folded(|f| {
            f.collections
                .iter()
                .map(|(k, t, ids)| {
                    (
                        (*k).to_string(),
                        (*t).to_string(),
                        ids.iter().map(|i| (*i).to_string()).collect(),
                    )
                })
                .collect()
        })
    }

    #[test]
    fn collection_keys_name_each_parent_once() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Rows,
            &child(1, Change::Inserted, &[(&post, "1", None)]),
        );
        // Moved from post 1 to post 2: both.
        add(
            Publish::Rows,
            &child(
                2,
                Change::Updated,
                &[(&post, "2", None), (&post, "1", None)],
            ),
        );
        settle_posts();
        assert_eq!(
            collections(),
            [(
                "Comment.fk_post".to_string(),
                "Comment".to_string(),
                vec!["\"p1\"".to_string(), "\"p2\"".to_string()]
            )]
        );
        clear();
    }

    #[test]
    fn a_parent_not_found_names_nothing_and_one_unresolved_names_the_type() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Rows,
            &child(1, Change::Deleted, &[(&post, "9", None)]),
        );
        settle_posts();
        assert!(collections().is_empty());
        assert!(snapshot().2.is_empty(), "a parent that is gone is no type");
        // Written outside a flush, never resolved: the child type by name.
        add(
            Publish::Rows,
            &child(2, Change::Updated, &[(&post, "2", None)]),
        );
        assert!(collections().is_empty());
        assert_eq!(snapshot().2, ["Comment"]);
        clear();
    }

    #[test]
    fn a_parent_without_id_names_the_child_type() {
        clear();
        let tag = collection("Comment", "fk_tag", false);
        add(
            Publish::Rows,
            &child(1, Change::Updated, &[(&tag, "1", None)]),
        );
        settle_posts();
        assert!(collections().is_empty());
        assert_eq!(snapshot().2, ["Comment"]);
        clear();
    }

    #[test]
    fn types_mode_keeps_no_collection() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Types,
            &child(1, Change::Updated, &[(&post, "1", None)]),
        );
        settle_posts();
        assert!(collections().is_empty());
        clear();
    }

    #[test]
    fn a_parent_named_by_many_rows_is_kept_once() {
        clear();
        let post = collection("Comment", "fk_post", true);
        for pk in 0..100 {
            add(
                Publish::Rows,
                &child(pk, Change::Inserted, &[(&post, "1", None)]),
            );
        }
        settle_posts();
        add(
            Publish::Rows,
            &child(100, Change::Inserted, &[(&post, "1", None)]),
        );
        settle_posts();
        SET.with_borrow(|set| {
            let ids = set
                .members
                .iter()
                .filter(|m| matches!(m.parent, Parent::Id(_)))
                .count();
            assert_eq!(ids, 1, "one member per parent");
        });
        assert_eq!(collections()[0].2, ["\"p1\""]);
        clear();
    }

    #[test]
    fn an_id_the_write_read_needs_no_resolution() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Rows,
            &child(1, Change::Inserted, &[(&post, "1", Some("p1"))]),
        );
        settle_none();
        assert_eq!(collections()[0].2, ["\"p1\""]);
        clear();
    }

    #[test]
    fn a_parent_the_transaction_wrote_is_resolved_from_its_row() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Rows,
            &r("Post", 4, Some("\"p4\""), Change::Updated),
        );
        add(
            Publish::Rows,
            &child(1, Change::Updated, &[(&post, "4", None)]),
        );
        settle_none();
        assert_eq!(collections()[0].2, ["\"p4\""]);
        clear();
    }

    #[test]
    fn a_rollback_forgets_collection_keys() {
        clear();
        let post = collection("Comment", "fk_post", true);
        add(
            Publish::Rows,
            &child(1, Change::Updated, &[(&post, "1", None)]),
        );
        settle_posts();
        let m = mark();
        add(
            Publish::Rows,
            &child(2, Change::Updated, &[(&post, "2", None)]),
        );
        settle_posts();
        rollback(m);
        assert_eq!(collections()[0].2, ["\"p1\""]);
        clear();
    }

    #[test]
    fn the_listing_names_parents_as_the_feed_does() {
        clear();
        let post = collection("Comment", "fk_post", true);
        let tag = collection("Note", "fk_tag", false);
        add(
            Publish::Rows,
            &child(
                1,
                Change::Updated,
                &[(&post, "1", None), (&post, "9", None), (&tag, "1", None)],
            ),
        );
        let named = |pairs: &[(&std::rc::Rc<crate::catalog::Collection>, &str)]| {
            let parents: Vec<_> = pairs
                .iter()
                .map(|(c, key)| {
                    (
                        std::rc::Rc::clone(c),
                        crate::catalog::collections::ParentRef {
                            key: (*key).to_string(),
                            id: None,
                        },
                    )
                })
                .collect();
            listed(&parents)
        };
        // Not resolved yet: the child type stands for the parent.
        assert_eq!(named(&[(&post, "1")]), [Listed::Unlisted]);
        settle_posts();
        assert_eq!(
            named(&[(&post, "1"), (&post, "9"), (&post, "2"), (&tag, "1")]),
            [
                Listed::Id("\"p1\"".to_string()),
                Listed::Gone,
                Listed::Unlisted, // never named
                Listed::Unlisted, // no id column
            ]
        );
        clear();
    }
}
