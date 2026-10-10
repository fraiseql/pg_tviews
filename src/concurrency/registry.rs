//! What the current transaction holds: the value, intent and escalated locks it
//! took, so a lock is requested once, escalation is decided per relation, and a
//! rolled-back subtransaction forgets what the lock manager released.

use super::{Side, Space};
use std::cell::RefCell;
use std::collections::HashSet;

/// One lock, as recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Held {
    /// A value of a relation: the tag's hash.
    Value {
        relid: u32,
        space: Space,
        side: Side,
        hash: u32,
    },
    /// The intent lock on a relation that its value locks need.
    Intent {
        relid: u32,
        space: Space,
        side: Side,
    },
    /// The relation itself, in place of its values.
    Escalated {
        relid: u32,
        space: Space,
        side: Side,
    },
}

/// The locks of one transaction, in the order they were taken.
#[derive(Debug, Default)]
pub(super) struct Registry {
    log: Vec<Held>,
    index: HashSet<Held>,
}

impl Registry {
    pub(super) fn holds(&self, lock: &Held) -> bool {
        self.index.contains(lock)
    }

    /// Record a lock taken; false when it was already held.
    pub(super) fn record(&mut self, lock: Held) -> bool {
        let new = self.index.insert(lock);
        if new {
            self.log.push(lock);
        }
        new
    }

    /// The values of `relid` in `space` locked by `side`.
    pub(super) fn values(&self, relid: u32, space: Space, side: Side) -> usize {
        self.log
            .iter()
            .filter(|h| {
                matches!(h, Held::Value { relid: r, space: p, side: s, .. }
                         if *r == relid && *p == space && *s == side)
            })
            .count()
    }

    pub(super) fn escalated(&self, relid: u32, space: Space, side: Side) -> bool {
        self.holds(&Held::Escalated { relid, space, side })
    }

    /// Whether locking `new` more values of `relid` must lock the relation
    /// instead: past `threshold` values per relation and side, 0 always, a
    /// negative threshold never.
    pub(super) fn should_escalate(
        &self,
        relid: u32,
        space: Space,
        side: Side,
        new: usize,
        threshold: i32,
    ) -> bool {
        match usize::try_from(threshold) {
            Err(_) => false,
            Ok(0) => true,
            Ok(limit) => self.values(relid, space, side) + new > limit,
        }
    }

    pub(super) const fn mark(&self) -> usize {
        self.log.len()
    }

    /// Forget what was recorded after `mark`: a rolled-back subtransaction's
    /// locks are released by the lock manager.
    pub(super) fn rollback(&mut self, mark: usize) {
        for lock in self.log.drain(mark.min(self.log.len())..) {
            self.index.remove(&lock);
        }
    }

    pub(super) fn clear(&mut self) {
        self.log.clear();
        self.index.clear();
    }
}

thread_local! {
    pub(super) static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    const USERS: u32 = 16_384;

    const V: Space = Space::Values;

    fn value(hash: u32) -> Held {
        Held::Value {
            relid: USERS,
            space: V,
            side: Side::Writer,
            hash,
        }
    }

    #[test]
    fn a_value_is_recorded_once() {
        let mut r = Registry::default();
        assert!(r.record(value(1)));
        assert!(!r.record(value(1)));
        assert_eq!(r.values(USERS, V, Side::Writer), 1);
    }

    #[test]
    fn values_are_counted_per_relation_and_side() {
        let mut r = Registry::default();
        r.record(value(1));
        r.record(value(2));
        r.record(Held::Value {
            relid: USERS,
            space: V,
            side: Side::Refresh,
            hash: 3,
        });
        r.record(Held::Value {
            relid: USERS + 1,
            space: V,
            side: Side::Writer,
            hash: 4,
        });
        r.record(Held::Value {
            relid: USERS,
            space: Space::Keys,
            side: Side::Writer,
            hash: 5,
        });
        assert_eq!(r.values(USERS, V, Side::Writer), 2);
        assert_eq!(r.values(USERS, V, Side::Refresh), 1);
        assert_eq!(r.values(USERS, Space::Keys, Side::Writer), 1);
    }

    #[test]
    fn escalation_follows_the_threshold() {
        let mut r = Registry::default();
        for hash in 0..3 {
            r.record(value(hash));
        }
        assert!(!r.should_escalate(USERS, V, Side::Writer, 1, 4));
        assert!(r.should_escalate(USERS, V, Side::Writer, 2, 4));
        assert!(r.should_escalate(USERS, V, Side::Writer, 1, 0));
        assert!(!r.should_escalate(USERS, V, Side::Writer, 1_000_000, -1));
        assert!(!r.should_escalate(USERS, V, Side::Refresh, 4, 4));
        assert!(!r.should_escalate(USERS, Space::Keys, Side::Writer, 4, 4));
    }

    #[test]
    fn rollback_forgets_what_came_after_the_mark() {
        let mut r = Registry::default();
        r.record(value(1));
        let mark = r.mark();
        r.record(value(2));
        r.record(Held::Escalated {
            relid: USERS,
            space: V,
            side: Side::Writer,
        });
        r.rollback(mark);
        assert!(r.holds(&value(1)));
        assert!(!r.holds(&value(2)));
        assert!(!r.escalated(USERS, V, Side::Writer));
        // Taken again after the rollback: recorded again.
        assert!(r.record(value(2)));
    }

    #[test]
    fn a_lock_held_before_the_mark_survives_its_rollback() {
        let mut r = Registry::default();
        r.record(value(1));
        let mark = r.mark();
        assert!(!r.record(value(1)));
        r.rollback(mark);
        assert!(r.holds(&value(1)));
    }
}
