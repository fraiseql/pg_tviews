//! One acquisition of value and relation locks, in a fixed order (ADR 0207,
//! #277): two transactions that each take their locks in one pass can't wait
//! for each other in a cycle.

use super::registry::{Held, Registry};
use super::{LockTarget, Side, Space, value_hash};
use pgrx::pg_sys;
use std::collections::{BTreeMap, BTreeSet};

/// The locks one acquisition takes: values of relations, and relations whole.
#[derive(Debug, Default)]
pub struct Pass {
    values: Vec<(Space, LockTarget, Side, Vec<String>)>,
    relations: Vec<(u32, Space, Side)>,
    intents: Vec<(u32, Space, Side)>,
}

impl Pass {
    /// Lock `values` of `target` for `side`.
    pub fn values(&mut self, target: &LockTarget, side: Side, values: &[String]) {
        self.values_in(Space::Values, target, side, values);
    }

    pub(super) fn values_in(
        &mut self,
        space: Space,
        target: &LockTarget,
        side: Side,
        values: &[String],
    ) {
        if !values.is_empty() {
            self.values
                .push((space, target.clone(), side, values.to_vec()));
        }
    }

    /// Lock relation `relid` as a whole for `side`.
    pub fn relation(&mut self, relid: u32, side: Side) {
        self.relations.push((relid, Space::Values, side));
    }

    /// Take relation `relid`'s intent lock for `side`, with or without values.
    pub fn intent(&mut self, relid: u32, side: Side) {
        self.intents.push((relid, Space::Values, side));
    }

    /// Whether the pass locks nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.values.is_empty() && self.relations.is_empty() && self.intents.is_empty()
    }
}

/// One lock of a planned pass, in acquisition order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Op {
    /// A relation's lock in `space`, in `mode`, recorded as `held`.
    Relation {
        relid: u32,
        space: Space,
        mode: RelationMode,
    },
    /// A value's lock.
    Value {
        relid: u32,
        space: Space,
        hash: u32,
        side: Side,
    },
}

impl Op {
    /// Where the lock comes in the pass: by relation, then space, the
    /// relation's own lock before its values, then by value.
    const fn order(&self) -> (u32, Space, u8, u32) {
        match *self {
            Self::Relation { relid, space, mode } => (relid, space, 0, mode as u32),
            Self::Value {
                relid, space, hash, ..
            } => (relid, space, 1, hash),
        }
    }
}

/// The relation locks a pass may take, each covering what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RelationMode {
    /// A writer's intent (`RowExclusiveLock`).
    WriterIntent,
    /// A refresh's intent (`RowShareLock`).
    RefreshIntent,
    /// A refresh's escalated lock (`ShareLock`).
    RefreshEscalated,
    /// A writer's intent and a refresh's escalated lock, taken as one
    /// (`ShareRowExclusiveLock`): two transactions each holding the intent
    /// and then asking for the shared lock would wait for each other.
    WriterIntentRefreshEscalated,
    /// A writer's escalated lock (`ExclusiveLock`), which covers every other.
    WriterEscalated,
}

impl RelationMode {
    pub(super) const fn lockmode(self) -> pg_sys::LOCKMODE {
        match self {
            Self::WriterIntent => pg_sys::RowExclusiveLock.cast_signed(),
            Self::RefreshIntent => pg_sys::RowShareLock.cast_signed(),
            Self::RefreshEscalated => pg_sys::ShareLock.cast_signed(),
            Self::WriterIntentRefreshEscalated => pg_sys::ShareRowExclusiveLock.cast_signed(),
            Self::WriterEscalated => pg_sys::ExclusiveLock.cast_signed(),
        }
    }

    /// What it is recorded as.
    pub(super) fn held(self, relid: u32, space: Space) -> Vec<Held> {
        let intent = |side| Held::Intent { relid, space, side };
        let escalated = |side| Held::Escalated { relid, space, side };
        match self {
            Self::WriterIntent => vec![intent(Side::Writer)],
            Self::RefreshIntent => vec![intent(Side::Refresh)],
            Self::RefreshEscalated => vec![escalated(Side::Refresh)],
            Self::WriterIntentRefreshEscalated => {
                vec![intent(Side::Writer), escalated(Side::Refresh)]
            }
            Self::WriterEscalated => vec![escalated(Side::Writer), escalated(Side::Refresh)],
        }
    }
}

/// What one side of a pass wants of a relation, the stronger covering the weaker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Want {
    #[default]
    Nothing,
    Intent,
    Escalated,
}

/// What one relation and space of a pass wants, per side.
#[derive(Debug, Default)]
struct Wants {
    writer: Want,
    refresh: Want,
}

impl Wants {
    /// Raise what `side` wants to at least `want`.
    fn raise(&mut self, side: Side, want: Want) {
        let w = match side {
            Side::Writer => &mut self.writer,
            Side::Refresh => &mut self.refresh,
        };
        *w = (*w).max(want);
    }
}

/// How many times `threshold` values of one relation a refresh locks one by one
/// when it may not lock the relation.
pub(super) const LINK_CAP_FACTOR: usize = 16;

/// A refresh would lock more than `max` values of a relation it may not lock
/// whole.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TooManyLinks {
    pub max: usize,
}

/// The locks `pass` takes that `registry` doesn't hold, in acquisition order,
/// and the relations it escalates. A writer's lock on a value or a relation
/// covers a refresh's. Past `threshold` values per relation, space and side
/// (counting those held), the relation is locked instead; 0 always does. A
/// refresh locks a relation whole only when `may_lock_whole(relid)`: it blocks
/// every writer of it. Otherwise it keeps locking values, up to
/// [`LINK_CAP_FACTOR`] times `threshold`.
///
/// # Errors
/// [`TooManyLinks`] past that cap.
pub(super) fn plan(
    pass: &Pass,
    registry: &Registry,
    threshold: usize,
    may_lock_whole: &dyn Fn(u32) -> bool,
) -> Result<(Vec<Op>, usize), TooManyLinks> {
    // The values asked for, per relation, space and side, writers first.
    let mut asked: BTreeMap<(u32, Space, Side), BTreeSet<u32>> = BTreeMap::new();
    for (space, target, side, values) in &pass.values {
        asked
            .entry((target.relid, *space, *side))
            .or_default()
            .extend(values.iter().map(|v| value_hash(&target.attnums, v)));
    }
    let covers = |relid, space, side, hash| {
        let held = |side| {
            registry.holds(&Held::Value {
                relid,
                space,
                side,
                hash,
            })
        };
        held(side)
            || (side == Side::Refresh
                && (held(Side::Writer)
                    || asked
                        .get(&(relid, space, Side::Writer))
                        .is_some_and(|w| w.contains(&hash))))
    };
    let escalated = |relid, space, side| {
        registry.escalated(relid, space, side)
            || (side == Side::Refresh && registry.escalated(relid, space, Side::Writer))
    };
    let mut wants: BTreeMap<(u32, Space), Wants> = BTreeMap::new();
    let mut ops = Vec::new();
    for &(relid, space, side) in &pass.relations {
        wants
            .entry((relid, space))
            .or_default()
            .raise(side, Want::Escalated);
    }
    for &(relid, space, side) in &pass.intents {
        wants
            .entry((relid, space))
            .or_default()
            .raise(side, Want::Intent);
    }
    let mut escalations = 0;
    // `Side` orders writers first, so a refresh's values are checked against
    // the writer's asked in the same pass.
    for (&(relid, space, side), hashes) in &asked {
        if escalated(relid, space, side) {
            continue;
        }
        let new: Vec<u32> = hashes
            .iter()
            .copied()
            .filter(|&hash| !covers(relid, space, side, hash))
            .collect();
        if new.is_empty() {
            continue;
        }
        if registry.should_escalate(relid, space, side, new.len(), threshold) {
            if side == Side::Writer || may_lock_whole(relid) {
                escalations += 1;
                wants
                    .entry((relid, space))
                    .or_default()
                    .raise(side, Want::Escalated);
                continue;
            }
            let max = threshold.max(1).saturating_mul(LINK_CAP_FACTOR);
            if registry.values(relid, space, side) + new.len() > max {
                return Err(TooManyLinks { max });
            }
        }
        wants
            .entry((relid, space))
            .or_default()
            .raise(side, Want::Intent);
        ops.extend(new.into_iter().map(|hash| Op::Value {
            relid,
            space,
            hash,
            side,
        }));
    }
    // A relation escalated for a side takes none of that side's values.
    ops.retain(|op| match *op {
        Op::Value {
            relid, space, side, ..
        } => wants.get(&(relid, space)).is_none_or(|w| match side {
            Side::Writer => w.writer < Want::Escalated,
            Side::Refresh => w.refresh < Want::Escalated && w.writer < Want::Escalated,
        }),
        Op::Relation { .. } => true,
    });
    ops.extend(relation_ops(&wants, registry));
    ops.sort_by_key(Op::order);
    Ok((ops, escalations))
}

/// The relation locks `wants` asks for that `registry` doesn't hold.
fn relation_ops(wants: &BTreeMap<(u32, Space), Wants>, registry: &Registry) -> Vec<Op> {
    let escalated = |relid, space, side| {
        registry.escalated(relid, space, side)
            || (side == Side::Refresh && registry.escalated(relid, space, Side::Writer))
    };
    let mut ops = Vec::new();
    for (&(relid, space), w) in wants {
        let holds = |lock| registry.holds(&lock);
        let intent_held = |side| holds(Held::Intent { relid, space, side });
        let mut modes = Vec::new();
        if w.writer == Want::Escalated {
            if !escalated(relid, space, Side::Writer) {
                modes.push(RelationMode::WriterEscalated);
            }
        } else {
            let writer_intent = w.writer == Want::Intent
                && !intent_held(Side::Writer)
                && !escalated(relid, space, Side::Writer);
            let refresh_escalated =
                w.refresh == Want::Escalated && !escalated(relid, space, Side::Refresh);
            match (writer_intent, refresh_escalated) {
                (true, true) => modes.push(RelationMode::WriterIntentRefreshEscalated),
                (true, false) => modes.push(RelationMode::WriterIntent),
                (false, true) => modes.push(RelationMode::RefreshEscalated),
                (false, false) => {}
            }
            if w.refresh == Want::Intent
                && !intent_held(Side::Refresh)
                && !escalated(relid, space, Side::Refresh)
            {
                modes.push(RelationMode::RefreshIntent);
            }
        }
        ops.extend(
            modes
                .into_iter()
                .map(|mode| Op::Relation { relid, space, mode }),
        );
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(relid: u32) -> LockTarget {
        LockTarget {
            relid,
            attnums: vec![1],
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn a_pass_is_sorted_by_relation_then_relation_lock_then_value() {
        let mut pass = Pass::default();
        pass.values(&target(20), Side::Refresh, &strings(&["b", "a"]));
        pass.values(&target(10), Side::Writer, &strings(&["c"]));
        let (ops, _) = plan(&pass, &Registry::default(), 64, &|_| true).unwrap();
        let order: Vec<_> = ops.iter().map(Op::order).collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(order, sorted);
        assert!(matches!(ops[0], Op::Relation { relid: 10, .. }));
        assert!(matches!(ops[2], Op::Relation { relid: 20, .. }));
        assert_eq!(ops.len(), 5);
    }

    #[test]
    fn a_refresh_locks_values_of_a_relation_it_may_not_lock_whole() {
        let mut pass = Pass::default();
        pass.values(&target(20), Side::Refresh, &strings(&["a", "b", "c"]));
        let (ops, escalations) = plan(&pass, &Registry::default(), 1, &|_| false).unwrap();
        assert_eq!(escalations, 0);
        let values = ops
            .iter()
            .filter(|op| matches!(op, Op::Value { .. }))
            .count();
        assert_eq!(values, 3);
        assert!(!ops.iter().any(|op| matches!(
            op,
            Op::Relation {
                mode: RelationMode::RefreshEscalated,
                ..
            }
        )));
    }

    #[test]
    fn a_refresh_past_the_cap_on_such_a_relation_is_refused() {
        let mut pass = Pass::default();
        let values: Vec<String> = (0..=LINK_CAP_FACTOR).map(|i| i.to_string()).collect();
        pass.values(&target(20), Side::Refresh, &values);
        assert_eq!(
            plan(&pass, &Registry::default(), 1, &|_| false),
            Err(TooManyLinks {
                max: LINK_CAP_FACTOR
            })
        );
        let mut within = Pass::default();
        within.values(&target(20), Side::Refresh, &values[1..]);
        assert!(plan(&within, &Registry::default(), 1, &|_| false).is_ok());
    }

    #[test]
    fn a_writer_escalates_whatever_it_may_lock_whole() {
        let mut pass = Pass::default();
        pass.values(&target(10), Side::Writer, &strings(&["a", "b"]));
        let (_, escalations) = plan(&pass, &Registry::default(), 1, &|_| false).unwrap();
        assert_eq!(escalations, 1);
    }

    #[test]
    fn a_writer_value_covers_the_same_refresh_value() {
        let mut pass = Pass::default();
        pass.values(&target(10), Side::Refresh, &strings(&["a", "b"]));
        pass.values(&target(10), Side::Writer, &strings(&["a"]));
        let (ops, _) = plan(&pass, &Registry::default(), 64, &|_| true).unwrap();
        let values: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Value { side, hash, .. } => Some((*side, *hash)),
                Op::Relation { .. } => None,
            })
            .collect();
        assert_eq!(values.iter().filter(|(s, _)| *s == Side::Writer).count(), 1);
        assert_eq!(
            values.iter().filter(|(s, _)| *s == Side::Refresh).count(),
            1
        );
        assert!(!values.contains(&(Side::Refresh, value_hash(&[1], "a"))));
    }

    #[test]
    fn a_writer_intent_and_a_refresh_escalation_are_one_lock() {
        let mut pass = Pass::default();
        pass.values(&target(10), Side::Writer, &strings(&["a"]));
        pass.values(&target(10), Side::Refresh, &strings(&["b", "c"]));
        let (ops, escalations) = plan(&pass, &Registry::default(), 1, &|_| true).unwrap();
        assert_eq!(escalations, 1);
        assert_eq!(
            ops.first(),
            Some(&Op::Relation {
                relid: 10,
                space: Space::Values,
                mode: RelationMode::WriterIntentRefreshEscalated,
            })
        );
        assert!(ops.iter().all(|op| !matches!(
            op,
            Op::Value {
                side: Side::Refresh,
                ..
            }
        )));
    }

    #[test]
    fn a_writer_escalation_takes_no_values() {
        let mut pass = Pass::default();
        pass.values(&target(10), Side::Writer, &strings(&["a", "b"]));
        pass.values(&target(10), Side::Refresh, &strings(&["c"]));
        let (ops, _) = plan(&pass, &Registry::default(), 1, &|_| true).unwrap();
        assert_eq!(
            ops,
            vec![Op::Relation {
                relid: 10,
                space: Space::Values,
                mode: RelationMode::WriterEscalated,
            }]
        );
    }

    #[test]
    fn held_locks_are_not_taken_again() {
        let mut registry = Registry::default();
        registry.record(Held::Intent {
            relid: 10,
            space: Space::Values,
            side: Side::Writer,
        });
        registry.record(Held::Value {
            relid: 10,
            space: Space::Values,
            side: Side::Writer,
            hash: value_hash(&[1], "a"),
        });
        let mut pass = Pass::default();
        pass.values(&target(10), Side::Writer, &strings(&["a"]));
        pass.values(&target(10), Side::Refresh, &strings(&["a"]));
        assert!(plan(&pass, &registry, 64, &|_| true).unwrap().0.is_empty());
    }
}
