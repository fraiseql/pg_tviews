//! The per-backend memo every cache is made of, and the one path that
//! invalidates them all.
//!
//! A memo only remembers catalog reads: clearing it costs a re-read, never
//! correctness. Each owner declares its memos next to the reads they remember
//! (the catalog its rows and plans, the delta its queries, the SPI layer its kept
//! statements); this module knows none of them. DDL on a watched relation (a
//! TVIEW's table or backing view, `pg_tview_meta`, a base table a mapping query
//! reads) invalidates its relcache entry in every backend; the relcache callback
//! bumps this backend's generation, and the next lookup in any memo sees the bump
//! and clears it. The callback only touches a `Cell`: it can run in the middle of
//! a catalog access, so it must neither take a lock nor call SPI.

use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Entries a memo holds before it is cleared and refilled lazily: bounds a
/// backend's memory on high-cardinality workloads.
const MAX_ENTRIES: usize = 10_000;

/// A memo of catalog reads, keyed by `K`, bounded by [`MAX_ENTRIES`].
/// Cleared at the first lookup after an invalidation ([`invalidate_all`]), except
/// the entries its `keep` keeps.
pub struct Memo<K, V> {
    map: RefCell<HashMap<K, V>>,
    /// The epoch the entries were read in.
    epoch: Cell<u64>,
    keep: fn(&K) -> bool,
}

impl<K: Eq + Hash, V: Clone> Memo<K, V> {
    /// A memo cleared whole at an invalidation.
    #[must_use]
    pub fn new() -> Self {
        Self::keeping(|_| false)
    }

    /// A memo whose entries `keep` keeps outlive an invalidation.
    #[must_use]
    pub fn keeping(keep: fn(&K) -> bool) -> Self {
        Self {
            map: RefCell::new(HashMap::new()),
            epoch: Cell::new(EPOCH.with(Cell::get)),
            keep,
        }
    }

    /// The value for `key`, after any pending invalidation.
    pub fn get(&self, key: &K) -> Option<V> {
        self.sync();
        self.map.borrow().get(key).cloned()
    }

    /// Remember `value` for `key`, read since the lookup that missed it: unless
    /// an invalidation arrived since that lookup, when what was read may predate
    /// it (but a key the memo keeps across invalidations).
    pub fn insert(&self, key: K, value: V) {
        self.insert_read_in(self.epoch.get(), key, value);
    }

    /// Remember `value` for `key`, read by a read that started in `read_in`
    /// ([`epoch`]): unless an invalidation arrived since, when what was read may
    /// predate it (but a key the memo keeps across invalidations). Every row of
    /// one read is remembered, or none.
    pub fn insert_read_in(&self, read_in: u64, key: K, value: V) {
        self.sync();
        if self.epoch.get() != read_in && !(self.keep)(&key) {
            return;
        }
        let mut map = self.map.borrow_mut();
        if map.len() >= MAX_ENTRIES {
            map.clear();
        }
        map.insert(key, value);
    }

    /// The value for `key`, loaded by `load` on a miss.
    ///
    /// # Errors
    /// What `load` returns.
    pub fn get_or_load(
        &self,
        key: K,
        load: impl FnOnce() -> crate::TViewResult<V>,
    ) -> crate::TViewResult<V> {
        if let Some(value) = self.get(&key) {
            return Ok(value);
        }
        let read_in = self.epoch.get();
        let value = load()?;
        self.insert_read_in(read_in, key, value.clone());
        Ok(value)
    }

    /// Forget what was read before the last invalidation.
    fn sync(&self) {
        sync_generation();
        let epoch = EPOCH.with(Cell::get);
        if self.epoch.get() != epoch {
            let keep = self.keep;
            self.map.borrow_mut().retain(|k, _| keep(k));
            self.epoch.set(epoch);
        }
    }
}

impl<K: Eq + Hash, V: Clone> Default for Memo<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

/// How the catalog's OID is found: `None` before `CREATE EXTENSION`.
type CatalogOid = fn() -> Option<Oid>;

/// The epoch a catalog read starting now runs in, for [`Memo::insert_read_in`].
pub fn epoch() -> u64 {
    sync_generation();
    EPOCH.with(Cell::get)
}

/// Forget every memoized value of this backend (but what a memo keeps, see
/// [`Memo::keeping`]).
pub fn invalidate_all() {
    CHANGED_IN_XACT.with(|c| c.set(true));
    EPOCH.with(|e| e.set(e.get().wrapping_add(1)));
    // The catalog may have been dropped and created again under another OID.
    CATALOG_WATCHED.with(|w| w.set(false));
}

// ── cross-backend invalidation ───────────────────────────────────────────────

thread_local! {
    /// The number of invalidations so far: what a memo read under one value
    /// holds while it is unchanged.
    static EPOCH: Cell<u64> = const { Cell::new(0) };
    /// How this backend finds `pg_tview_meta`'s OID ([`set_catalog`]).
    static CATALOG: Cell<Option<CatalogOid>> = const { Cell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static SEEN_GENERATION: Cell<u64> = const { Cell::new(0) };
    static WATCHED: RefCell<HashSet<Oid>> = RefCell::new(HashSet::new());
    static CATALOG_WATCHED: Cell<bool> = const { Cell::new(false) };
    /// A relation invalidation arrived, or the caches were cleared, in the open
    /// transaction.
    static CHANGED_IN_XACT: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "test-hooks")]
thread_local! {
    /// Full relcache invalidations received: a shared invalidation queue reset,
    /// or a change to every relation (`CREATE PUBLICATION … FOR ALL TABLES`).
    static RESETS: Cell<u64> = const { Cell::new(0) };
}

/// The full relcache invalidations this backend received: each drops every
/// memo, so the next write reads the catalog again. Tests counting catalog
/// lookups read it to tell a reset from a regression.
#[cfg(feature = "test-hooks")]
pub fn resets() -> u64 {
    RESETS.with(Cell::get)
}

/// At the abort of a transaction, or of a subtransaction: forget the caches if
/// the transaction may have memoized a catalog change being rolled back. A change
/// of its own came through the utility hook ([`mark_changed`]), as a relation
/// invalidation (processed locally at each command boundary) or a clear by the
/// DDL path that made it; without one, the
/// caches hold what the committed catalog says, and the next write needs no lookup.
pub fn forget_on_abort() {
    if changed_in_xact() {
        invalidate_all();
    }
}

/// The open transaction ran a statement that may change the catalog.
pub fn mark_changed() {
    CHANGED_IN_XACT.with(|c| c.set(true));
}

/// Whether the open transaction may have changed the catalog: what an abort
/// must forget (see [`forget_on_abort`]).
pub fn changed_in_xact() -> bool {
    CHANGED_IN_XACT.with(Cell::get)
}

/// At the end of a transaction, however it ends.
pub fn end_transaction() {
    CHANGED_IN_XACT.with(|c| c.set(false));
}

/// How this backend finds `pg_tview_meta`'s OID, to watch it. Called once from
/// `_PG_init`.
pub fn set_catalog(oid: CatalogOid) {
    CATALOG.with(|c| c.set(Some(oid)));
}

/// Watch `pg_tview_meta` itself: its statement trigger invalidates it on every
/// write, which is how other backends learn about a created, changed or dropped
/// TVIEW. Resolved again after every invalidation.
fn watch_catalog() {
    if CATALOG_WATCHED.with(Cell::get) {
        return;
    }
    if let Some(oid) = CATALOG.with(Cell::get).and_then(|catalog| catalog()) {
        watch(&[oid]);
        CATALOG_WATCHED.with(|w| w.set(true));
    }
}

/// Watch relations whose DDL must invalidate the caches.
pub fn watch(oids: &[Oid]) {
    WATCHED.with(|w| w.borrow_mut().extend(oids.iter().copied()));
}

/// Invalidate every memo if a relevant invalidation arrived since the last call.
pub fn sync_generation() {
    watch_catalog();
    let current = GENERATION.with(Cell::get);
    if SEEN_GENERATION.with(Cell::get) != current {
        SEEN_GENERATION.with(|s| s.set(current));
        invalidate_all();
        watch_catalog();
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn relcache_callback(_arg: pg_sys::Datum, relid: Oid) {
    // Any relation's: what the caches memoized of one they don't watch may be rolled
    // back too. Plain writes send none.
    CHANGED_IN_XACT.with(|c| c.set(true));
    #[cfg(feature = "test-hooks")]
    if relid == pg_sys::InvalidOid {
        RESETS.with(|r| r.set(r.get() + 1));
    }
    let relevant = relid == pg_sys::InvalidOid
        || WATCHED.with(|w| w.try_borrow().map_or(true, |w| w.contains(&relid)));
    if relevant {
        GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    }
}

/// Register the relcache callback. Called once from `_PG_init`.
pub fn register_relcache_callback() {
    // SAFETY: registers a static callback with no argument; valid in `_PG_init`.
    unsafe {
        pg_sys::CacheRegisterRelcacheCallback(Some(relcache_callback), pg_sys::Datum::from(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidate_all_clears_memos_but_what_they_keep_and_the_catalog_watch() {
        let plain: Memo<u32, u32> = Memo::new();
        let kept: Memo<u32, u32> = Memo::keeping(|k| *k > 1);
        // Filled directly: `insert` reads a GUC, which a test thread may not.
        plain.map.borrow_mut().insert(1, 1);
        kept.map.borrow_mut().extend([(1, 1), (2, 2)]);
        CATALOG_WATCHED.with(|w| w.set(true));
        invalidate_all();
        CATALOG_WATCHED.with(|w| w.set(true));
        assert_eq!(plain.get(&1), None);
        assert_eq!((kept.get(&1), kept.get(&2)), (None, Some(2)));
        invalidate_all();
        assert!(!CATALOG_WATCHED.with(Cell::get));
    }

    #[test]
    fn a_value_loaded_across_an_invalidation_is_not_remembered() {
        let memo: Memo<u32, u32> = Memo::new();
        let outer: Memo<u32, u32> = Memo::new();
        let invalidating = |value| {
            invalidate_all();
            Ok(value)
        };
        assert_eq!(memo.get_or_load(1, || invalidating(10)).unwrap(), 10);
        assert_eq!(memo.get(&1), None);
        // A load nested in another's: neither value is remembered.
        let value = outer
            .get_or_load(1, || memo.get_or_load(2, || invalidating(20)))
            .unwrap();
        assert_eq!(value, 20);
        assert_eq!((outer.get(&1), memo.get(&2)), (None, None));
        // A lookup, then a separate insert after an invalidation.
        assert_eq!(memo.get(&3), None);
        invalidate_all();
        memo.insert(3, 30);
        assert_eq!(memo.get(&3), None);
        // A load no invalidation crossed is remembered.
        assert_eq!(memo.get_or_load(4, || Ok(40)).unwrap(), 40);
        assert_eq!(memo.get(&4), Some(40));
    }

    #[test]
    fn rows_of_one_read_across_an_invalidation_are_not_remembered() {
        let memo: Memo<u32, u32> = Memo::new();
        assert_eq!(memo.get(&1), None);
        let read_in = epoch();
        // The read that returns both rows sees the catalog before an
        // invalidation absorbed after it.
        invalidate_all();
        memo.insert_read_in(read_in, 1, 10);
        memo.insert_read_in(read_in, 2, 20);
        assert_eq!((memo.get(&1), memo.get(&2)), (None, None));
    }

    #[test]
    fn a_load_looking_up_its_own_memo_across_an_invalidation_is_not_remembered() {
        let memo: Memo<u32, u32> = Memo::new();
        let value = memo
            .get_or_load(1, || {
                invalidate_all();
                Ok(memo.get(&2).unwrap_or(10))
            })
            .unwrap();
        assert_eq!(value, 10);
        assert_eq!(memo.get(&1), None);
    }

    #[test]
    fn a_kept_entry_loaded_across_an_invalidation_is_remembered() {
        let kept: Memo<u32, u32> = Memo::keeping(|k| *k > 1);
        let loaded = kept.get_or_load(2, || {
            invalidate_all();
            Ok(2)
        });
        assert_eq!(loaded.unwrap(), 2);
        assert_eq!(kept.get(&2), Some(2));
    }
}
