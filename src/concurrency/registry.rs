//! What the current transaction holds: the value, intent and escalated locks it
//! took, so a lock is requested once, escalation is decided per relation, and a
//! rolled-back subtransaction forgets what the lock manager released.

use super::Side;
use std::cell::RefCell;
use std::collections::HashSet;

/// One lock, as recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Held {
    /// A value of a relation: the tag's hash.
    Value { relid: u32, side: Side, hash: u32 },
    /// The intent lock on a relation that its value locks need.
    Intent { relid: u32, side: Side },
    /// The relation itself, in place of its values.
    Escalated { relid: u32, side: Side },
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

    /// The values of `relid` locked by `side`.
    pub(super) fn values(&self, relid: u32, side: Side) -> usize {
        self.log
            .iter()
            .filter(
                |h| matches!(h, Held::Value { relid: r, side: s, .. } if *r == relid && *s == side),
            )
            .count()
    }

    pub(super) fn escalated(&self, relid: u32, side: Side) -> bool {
        self.holds(&Held::Escalated { relid, side })
    }

    /// Whether locking `new` more values of `relid` must lock the relation
    /// instead: past `threshold` values per relation and side, 0 always, a
    /// negative threshold never.
    pub(super) fn should_escalate(
        &self,
        relid: u32,
        side: Side,
        new: usize,
        threshold: i32,
    ) -> bool {
        match usize::try_from(threshold) {
            Err(_) => false,
            Ok(0) => true,
            Ok(limit) => self.values(relid, side) + new > limit,
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

    fn value(hash: u32) -> Held {
        Held::Value {
            relid: USERS,
            side: Side::Writer,
            hash,
        }
    }

    #[test]
    fn a_value_is_recorded_once() {
        let mut r = Registry::default();
        assert!(r.record(value(1)));
        assert!(!r.record(value(1)));
        assert_eq!(r.values(USERS, Side::Writer), 1);
    }

    #[test]
    fn values_are_counted_per_relation_and_side() {
        let mut r = Registry::default();
        r.record(value(1));
        r.record(value(2));
        r.record(Held::Value {
            relid: USERS,
            side: Side::Refresh,
            hash: 3,
        });
        r.record(Held::Value {
            relid: USERS + 1,
            side: Side::Writer,
            hash: 4,
        });
        assert_eq!(r.values(USERS, Side::Writer), 2);
        assert_eq!(r.values(USERS, Side::Refresh), 1);
    }

    #[test]
    fn escalation_follows_the_threshold() {
        let mut r = Registry::default();
        for hash in 0..3 {
            r.record(value(hash));
        }
        assert!(!r.should_escalate(USERS, Side::Writer, 1, 4));
        assert!(r.should_escalate(USERS, Side::Writer, 2, 4));
        assert!(r.should_escalate(USERS, Side::Writer, 1, 0));
        assert!(!r.should_escalate(USERS, Side::Writer, 1_000_000, -1));
        assert!(!r.should_escalate(USERS, Side::Refresh, 4, 4));
    }

    #[test]
    fn rollback_forgets_what_came_after_the_mark() {
        let mut r = Registry::default();
        r.record(value(1));
        let mark = r.mark();
        r.record(value(2));
        r.record(Held::Escalated {
            relid: USERS,
            side: Side::Writer,
        });
        r.rollback(mark);
        assert!(r.holds(&value(1)));
        assert!(!r.holds(&value(2)));
        assert!(!r.escalated(USERS, Side::Writer));
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
