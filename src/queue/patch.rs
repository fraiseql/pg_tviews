//! Transaction-local patch payloads for the issue #56 direct-patch fast path.
//!
//! [`TX_REFRESH_QUEUE`](super::state::TX_REFRESH_QUEUE) (a `HashSet<RefreshKey>`)
//! stays the dedup/ordering key set; the JSONB payloads ride alongside in
//! [`TX_PATCH_MAP`], keyed by the same [`RefreshKey`]. A key is either carrying a
//! usable [`PatchState::Direct`] chain or is [`PatchState::Poisoned`] — an
//! ineligible change touched it this transaction, so it must recompute. Poison is
//! sticky.
//!
//! The map participates in savepoint snapshot/restore and abort clearing in
//! lockstep with the queue (see [`super::xact`]) — a patch that survived a
//! rollback would be a correctness bug, not a missing feature.

use super::key::RefreshKey;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;

/// One link of a patch chain: a JSONB path prefix and the fields to merge there.
///
/// An empty prefix targets the entity's own `data` object; a non-empty prefix
/// (e.g. `["author"]`) targets a nested embedded object — used by parent patch
/// derivation (issue #56).
pub type PatchEntry = (Vec<String>, Map<String, Value>);

/// The patch payload carried for a queued [`RefreshKey`].
#[derive(Debug, Clone, PartialEq)]
pub enum PatchState {
    /// A usable chain of patches to apply directly. Entries with distinct
    /// prefixes accumulate; a same-prefix merge keeps the later value per key.
    Direct(Vec<PatchEntry>),
    /// An ineligible change touched this key this transaction ⇒ recompute.
    /// Sticky: once poisoned, always poisoned (until abort / savepoint restore).
    Poisoned,
}

thread_local! {
    /// Transaction-local patch payloads keyed by `RefreshKey`. Lives beside
    /// `TX_REFRESH_QUEUE`; see module docs.
    pub static TX_PATCH_MAP: RefCell<HashMap<RefreshKey, PatchState>> =
        RefCell::new(HashMap::new());
}

/// Record a direct patch for `key`: merge `fields` at `prefix` into any existing
/// `Direct` chain, or start a new one. A `Poisoned` key stays poisoned (sticky).
///
/// Returns `true` iff this inserted a **fresh** `Direct` entry for a previously
/// unseen key — used to count captures exactly once even when a base table feeds
/// several tviews (and so has several row triggers that each fire and re-record
/// the same key within one statement).
pub fn record(key: RefreshKey, prefix: Vec<String>, fields: Map<String, Value>) -> bool {
    TX_PATCH_MAP.with(|m| {
        let mut map = m.borrow_mut();
        match map.get_mut(&key) {
            // Sticky poison — a prior ineligible change wins.
            Some(PatchState::Poisoned) => false,
            Some(PatchState::Direct(chain)) => {
                merge_into_chain(chain, prefix, fields);
                false
            }
            None => {
                map.insert(key, PatchState::Direct(vec![(prefix, fields)]));
                true
            }
        }
    })
}

/// Merge `fields` at `prefix` into an existing chain: same prefix ⇒ merge maps
/// (later value wins per key); a new prefix ⇒ push a new chain entry.
fn merge_into_chain(chain: &mut Vec<PatchEntry>, prefix: Vec<String>, fields: Map<String, Value>) {
    if let Some(entry) = chain.iter_mut().find(|(p, _)| *p == prefix) {
        for (k, v) in fields {
            entry.1.insert(k, v); // later value wins
        }
    } else {
        chain.push((prefix, fields));
    }
}

/// Poison `key`: any existing (or later) direct patch is discarded and the key
/// recomputes. Idempotent and sticky.
pub fn poison(key: RefreshKey) {
    TX_PATCH_MAP.with(|m| {
        m.borrow_mut().insert(key, PatchState::Poisoned);
    });
}

/// Take (and clear) the entire patch map. Used both at flush time (the consumer)
/// and at savepoint start (to stash the pre-savepoint state).
pub fn take_patch_snapshot() -> HashMap<RefreshKey, PatchState> {
    TX_PATCH_MAP.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

/// Replace the patch map wholesale — savepoint rollback restore.
pub fn replace_patch_map(new_map: HashMap<RefreshKey, PatchState>) {
    TX_PATCH_MAP.with(|m| *m.borrow_mut() = new_map);
}

/// Clear the patch map — transaction abort.
pub fn clear_patch_map() {
    TX_PATCH_MAP.with(|m| m.borrow_mut().clear());
}

// ── Flush-local map operations (issue #56) ───────────────────────────
//
// Parent patch derivation happens against the flush's *local* snapshot map, not
// the thread-local `TX_PATCH_MAP` (already drained). These mirror `record`/`poison`
// but operate on a caller-owned map.

/// Merge a derived `chain` into `map` for `key` (same rules as [`record`]): a
/// `Poisoned` key stays poisoned; a `Direct` key merges each entry; an absent key
/// starts a new `Direct`.
pub fn merge_chain_into(
    map: &mut HashMap<RefreshKey, PatchState>,
    key: RefreshKey,
    chain: Vec<PatchEntry>,
) {
    match map.get_mut(&key) {
        Some(PatchState::Poisoned) => {}
        Some(PatchState::Direct(existing)) => {
            for (prefix, fields) in chain {
                merge_into_chain(existing, prefix, fields);
            }
        }
        None => {
            map.insert(key, PatchState::Direct(chain));
        }
    }
}

/// Poison `key` in `map` (sticky) — the parent must recompute. Used when a parent
/// is reached from a recomputed child, or from a child whose dependency can't take
/// a path patch (array/scalar/uuid-fk).
pub fn poison_into(map: &mut HashMap<RefreshKey, PatchState>, key: RefreshKey) {
    map.insert(key, PatchState::Poisoned);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(k: &str, v: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(k.to_string(), Value::String(v.to_string()));
        m
    }

    fn reset() {
        clear_patch_map();
    }

    fn state_of(key: &RefreshKey) -> Option<PatchState> {
        TX_PATCH_MAP.with(|m| m.borrow().get(key).cloned())
    }

    #[test]
    fn merge_same_prefix_later_value_wins() {
        reset();
        let key = RefreshKey::pk("user", 1);
        record(key.clone(), vec![], field("bio", "old"));
        record(key.clone(), vec![], field("bio", "new"));

        match state_of(&key).unwrap() {
            PatchState::Direct(chain) => {
                assert_eq!(chain.len(), 1, "same prefix must merge, not accumulate");
                assert_eq!(chain[0].1.get("bio").unwrap(), &Value::String("new".into()));
            }
            PatchState::Poisoned => panic!("expected Direct"),
        }
        reset();
    }

    #[test]
    fn merge_same_prefix_unions_distinct_keys() {
        reset();
        let key = RefreshKey::pk("user", 1);
        record(key.clone(), vec![], field("bio", "b"));
        record(key.clone(), vec![], field("name", "n"));

        match state_of(&key).unwrap() {
            PatchState::Direct(chain) => {
                assert_eq!(chain.len(), 1);
                assert_eq!(chain[0].1.len(), 2);
                assert!(chain[0].1.contains_key("bio"));
                assert!(chain[0].1.contains_key("name"));
            }
            PatchState::Poisoned => panic!("expected Direct"),
        }
        reset();
    }

    #[test]
    fn distinct_prefixes_accumulate_entries() {
        reset();
        let key = RefreshKey::pk("post", 1);
        record(key.clone(), vec![], field("title", "t"));
        record(key.clone(), vec!["author".into()], field("bio", "b"));

        match state_of(&key).unwrap() {
            PatchState::Direct(chain) => {
                assert_eq!(chain.len(), 2, "distinct prefixes must accumulate");
            }
            PatchState::Poisoned => panic!("expected Direct"),
        }
        reset();
    }

    #[test]
    fn poison_is_sticky_over_later_record() {
        reset();
        let key = RefreshKey::pk("user", 1);
        poison(key.clone());
        record(key.clone(), vec![], field("bio", "new"));
        assert_eq!(state_of(&key), Some(PatchState::Poisoned));
        reset();
    }

    #[test]
    fn record_then_poison_overwrites_to_poison() {
        reset();
        let key = RefreshKey::pk("user", 1);
        record(key.clone(), vec![], field("bio", "new"));
        poison(key.clone());
        assert_eq!(state_of(&key), Some(PatchState::Poisoned));
        reset();
    }

    #[test]
    fn snapshot_take_clears_and_restore_round_trips() {
        reset();
        let key = RefreshKey::pk("user", 1);
        record(key.clone(), vec![], field("bio", "b"));

        let snap = take_patch_snapshot();
        assert!(state_of(&key).is_none(), "take must clear the live map");
        assert_eq!(snap.len(), 1);

        replace_patch_map(snap);
        assert!(matches!(state_of(&key), Some(PatchState::Direct(_))));
        reset();
    }

    #[test]
    fn clear_empties_the_map() {
        reset();
        record(RefreshKey::pk("user", 1), vec![], field("bio", "b"));
        clear_patch_map();
        assert!(take_patch_snapshot().is_empty());
    }
}
