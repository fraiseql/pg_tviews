//! Value locks: how concurrent transactions maintaining the same TVIEW rows wait
//! for each other (ADR 0207).
//!
//! A writer locks, exclusively, the join values of the rows it changed before it
//! looks TVIEW rows up by them; a refresh locks, shared, the join values its rows
//! read before it computes them. Past a threshold, a transaction locks the
//! relation instead of its values. Locks live in PostgreSQL's lock manager under a
//! private advisory tag and are held to the end of the transaction. REPEATABLE
//! READ fails instead of waiting; SERIALIZABLE takes none.

pub mod reads;
mod registry;

use pgrx::pg_sys;
use pgrx::prelude::*;
use registry::{Held, REGISTRY};

/// `field4` of a value lock's tag. `pg_advisory_lock()` uses 1 and 2.
pub const VALUE_TAG: u16 = 0x5476;
/// `field4` of a relation's intent and escalated locks.
pub const RELATION_TAG: u16 = 0x5477;

/// Which end of the protocol takes a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// A refresh, about to compute rows from the values it reads.
    Refresh,
    /// A writer, about to look rows up by the values it changed.
    Writer,
}

/// Where in a relation a lock lives: the values of its columns, or (a TVIEW's
/// table) the keys of rows being created. Each has its own intent and
/// escalated locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Space {
    Values,
    Keys,
}

impl Space {
    /// `field3` of the space's relation tag.
    const fn field3(self) -> u32 {
        match self {
            Self::Values => 0,
            Self::Keys => 1,
        }
    }
}

/// The columns of a relation whose values are locked: one value per row, the
/// columns' text forms combined when there are several.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LockTarget {
    /// The relation (a partition's root, a TVIEW's table for an embed).
    pub relid: u32,
    /// Its columns, in a fixed order.
    pub attnums: Vec<i16>,
}

/// What a transaction does when a lock isn't granted at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Wait for it (READ COMMITTED): what follows sees the other's commit.
    Wait,
    /// Fail with 40001 (REPEATABLE READ): the snapshot can't see what it waited for.
    FailFast,
    /// Take no lock (SERIALIZABLE): SSI detects the conflict.
    Skip,
}

impl Policy {
    /// The policy of an isolation level (`XactIsoLevel`).
    #[must_use]
    pub const fn of_level(level: u32) -> Self {
        match level {
            pg_sys::XACT_SERIALIZABLE => Self::Skip,
            pg_sys::XACT_REPEATABLE_READ => Self::FailFast,
            _ => Self::Wait,
        }
    }

    /// The policy of the current transaction.
    #[must_use]
    pub fn current() -> Self {
        // SAFETY: XactIsoLevel is a plain backend global, set for the transaction.
        let level = unsafe { pg_sys::XactIsoLevel };
        Self::of_level(u32::try_from(level).unwrap_or(0))
    }
}

const fn value_mode(side: Side) -> pg_sys::LOCKMODE {
    match side {
        Side::Refresh => pg_sys::ShareLock.cast_signed(),
        Side::Writer => pg_sys::ExclusiveLock.cast_signed(),
    }
}

const fn intent_mode(side: Side) -> pg_sys::LOCKMODE {
    match side {
        Side::Refresh => pg_sys::RowShareLock.cast_signed(),
        Side::Writer => pg_sys::RowExclusiveLock.cast_signed(),
    }
}

const fn escalated_mode(side: Side) -> pg_sys::LOCKMODE {
    match side {
        Side::Refresh => pg_sys::ShareLock.cast_signed(),
        Side::Writer => pg_sys::ExclusiveLock.cast_signed(),
    }
}

/// FNV-1a (32 bits) of the columns' attnums, a zero byte and the value's text:
/// computable without a server, stable across releases. A collision only makes
/// two values share a lock.
#[must_use]
pub fn value_hash(attnums: &[i16], value: &str) -> u32 {
    const OFFSET: u32 = 0x811c_9dc5;
    const PRIME: u32 = 0x0100_0193;
    attnums
        .iter()
        .flat_map(|a| a.to_le_bytes())
        .chain(std::iter::once(0))
        .chain(value.bytes())
        .fold(OFFSET, |h, b| (h ^ u32::from(b)).wrapping_mul(PRIME))
}

#[allow(clippy::cast_possible_truncation)] // Reason: lock tag types and lock method ids fit a uint8 (lock.h)
const fn tag(database: u32, relid: u32, field3: u32, field4: u16) -> pg_sys::LOCKTAG {
    pg_sys::LOCKTAG {
        locktag_field1: database,
        locktag_field2: relid,
        locktag_field3: field3,
        locktag_field4: field4,
        locktag_type: pg_sys::LockTagType::LOCKTAG_ADVISORY as u8,
        locktag_lockmethodid: pg_sys::USER_LOCKMETHOD as u8,
    }
}

/// The tag of a value lock.
#[must_use]
pub const fn value_tag(database: u32, relid: u32, hash: u32) -> pg_sys::LOCKTAG {
    tag(database, relid, hash, VALUE_TAG)
}

/// The tag of a relation's intent and escalated locks in `space`.
#[must_use]
pub const fn relation_tag(database: u32, relid: u32, space: Space) -> pg_sys::LOCKTAG {
    tag(database, relid, space.field3(), RELATION_TAG)
}

fn database() -> u32 {
    // SAFETY: MyDatabaseId is set once the backend is connected to a database.
    unsafe { pg_sys::MyDatabaseId }.to_u32()
}

/// Lock `values` of `target` for `side`, to the end of the transaction: values
/// already held are skipped, the relation's intent lock is taken with the first
/// one, and past `pg_tviews.lock_escalation_threshold` values the relation is
/// locked instead.
pub fn lock_values(target: &LockTarget, side: Side, values: &[String]) {
    lock_in(Space::Values, target, side, values);
}

/// A refresh about to create TVIEW rows (`keys`, rows of `tview` it found
/// missing) locks them exclusively, in the TVIEW's key space: two transactions
/// creating the same row, which no join value links (a child carrying the key
/// in its row, a new group of an aggregate), wait for each other. Escalation
/// locks the key space alone, so it only stops other creations of rows.
pub fn lock_new_keys(tview: pg_sys::Oid, keys: &[String]) {
    let target = LockTarget {
        relid: tview.to_u32(),
        attnums: vec![-1],
    };
    lock_in(Space::Keys, &target, Side::Writer, keys);
}

fn lock_in(space: Space, target: &LockTarget, side: Side, values: &[String]) {
    let policy = Policy::current();
    if policy == Policy::Skip || values.is_empty() {
        return;
    }
    let relid = target.relid;
    let mut hashes: Vec<u32> = values
        .iter()
        .map(|v| value_hash(&target.attnums, v))
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    let held = |hash| Held::Value {
        relid,
        space,
        side,
        hash,
    };
    let (escalate, intent, new) = REGISTRY.with_borrow(|r| {
        if r.escalated(relid, space, side) {
            return (false, false, Vec::new());
        }
        let new: Vec<u32> = hashes
            .into_iter()
            .filter(|&hash| !r.holds(&held(hash)))
            .collect();
        let threshold = crate::config::lock_escalation_threshold();
        let escalate =
            !new.is_empty() && r.should_escalate(relid, space, side, new.len(), threshold);
        (
            escalate,
            !r.holds(&Held::Intent { relid, space, side }),
            new,
        )
    });
    if escalate {
        relation_lock(relid, space, side, escalated_mode(side));
        return;
    }
    if new.is_empty() {
        return;
    }
    if intent {
        relation_lock(relid, space, side, intent_mode(side));
    }
    let database = database();
    for hash in new {
        acquire(
            &value_tag(database, relid, hash),
            value_mode(side),
            side,
            policy,
        );
        REGISTRY.with_borrow_mut(|r| r.record(held(hash)));
    }
}

/// Lock relation `relid` as a whole for `side`, to the end of the transaction:
/// a writer that changes every value (a full refresh), or a transaction past the
/// escalation threshold.
pub fn lock_relation(relid: u32, side: Side) {
    relation_lock(relid, Space::Values, side, escalated_mode(side));
}

/// Take the intent lock of relation `relid` for `side` alone: a refresh of a
/// TVIEW's rows conflicts with a writer that locked the whole TVIEW.
pub fn lock_intent(relid: u32, side: Side) {
    relation_lock(relid, Space::Values, side, intent_mode(side));
}

/// Take `relid`'s relation lock in `space` in `mode`, the intent or the
/// escalated mode of `side`, unless held (an escalated lock covers the intent).
fn relation_lock(relid: u32, space: Space, side: Side, mode: pg_sys::LOCKMODE) {
    let policy = Policy::current();
    let escalated = mode == escalated_mode(side);
    let lock = if escalated {
        Held::Escalated { relid, space, side }
    } else {
        Held::Intent { relid, space, side }
    };
    let held = REGISTRY.with_borrow(|r| r.holds(&lock) || r.escalated(relid, space, side));
    if policy == Policy::Skip || held {
        return;
    }
    acquire(&relation_tag(database(), relid, space), mode, side, policy);
    REGISTRY.with_borrow_mut(|r| r.record(lock));
}

/// Writer side: before a query finds TVIEW rows by the values a statement wrote
/// to `mapping`'s table, lock those values in each read-set column. `values`
/// returns the distinct text values of a column (its quoted name) among the
/// changed rows, both images of an update. A read set with no column (the
/// table is locked as a whole by refreshes) takes the intent lock alone.
///
/// # Errors
/// What `values` returns.
pub fn lock_changed_rows(
    mapping: &crate::lineage::KeyMapping,
    mut values: impl FnMut(&str) -> crate::TViewResult<Vec<String>>,
) -> crate::TViewResult<()> {
    if Policy::current() == Policy::Skip {
        return Ok(());
    }
    for set in &mapping.reads {
        if set.attnum == 0 {
            lock_intent(mapping.relid, Side::Writer);
            continue;
        }
        let Some(column) = column_name(mapping.relid, set.attnum)? else {
            continue;
        };
        let target = LockTarget {
            relid: mapping.relid,
            attnums: vec![set.attnum],
        };
        lock_values(&target, Side::Writer, &values(&column)?);
    }
    Ok(())
}

/// The keys of an embedded TVIEW's rows (`pk_<child>`), locked by `side` on its
/// table: a writer before it looks up the parents holding them, a refresh before
/// it computes parents from them.
pub fn lock_embedded_keys(child_table: pg_sys::Oid, side: Side, keys: &[String]) {
    let target = LockTarget {
        relid: child_table.to_u32(),
        attnums: Vec::new(),
    };
    lock_values(&target, side, keys);
}

/// The quoted name of column `attnum` of `relid` (cached); `None` when it is gone.
fn column_name(relid: u32, attnum: i16) -> crate::TViewResult<Option<String>> {
    let key = (relid, attnum);
    if let Some(name) = crate::cache::LOCK_COLUMNS.with(|m| m.get(&key)) {
        return Ok(name);
    }
    let template = format!("{{c:{relid}:{attnum}}}");
    let name = crate::lineage::render_template(&template)
        .map_err(|e| crate::utils::spi::error(&template, &e))?;
    crate::cache::watch(&[pg_sys::Oid::from(relid)]);
    crate::cache::LOCK_COLUMNS.with(|m| m.insert(key, name.clone()));
    Ok(name)
}

/// Acquire `tag` in `mode`: at once, or as `policy` says when another
/// transaction holds a conflicting lock.
fn acquire(tag: &pg_sys::LOCKTAG, mode: pg_sys::LOCKMODE, side: Side, policy: Policy) {
    // SAFETY: a transaction is in progress (a trigger or a flush runs inside one)
    // and the tag is a well-formed advisory tag; the lock belongs to the current
    // resource owner and ends with the (sub)transaction.
    let got = unsafe { pg_sys::LockAcquire(tag, mode, false, true) };
    if got != pg_sys::LockAcquireResult::LOCKACQUIRE_NOT_AVAIL {
        return;
    }
    match policy {
        Policy::FailFast => serialization_failure(match side {
            Side::Refresh => "a concurrent transaction changes rows this TVIEW refresh reads",
            Side::Writer => {
                "a concurrent transaction refreshes TVIEW rows from rows this write changes"
            }
        }),
        // SAFETY: as above; waiting honours lock_timeout and deadlock detection,
        // which raise an ERROR.
        Policy::Wait | Policy::Skip => unsafe {
            pg_sys::LockAcquire(tag, mode, false, false);
        },
    }
}

/// Raise 40001 with `why`. Does not return.
pub fn serialization_failure(why: &str) -> ! {
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_T_R_SERIALIZATION_FAILURE,
        format!("could not serialize access: {why}"),
        function_name!(),
    )
    .set_hint("The transaction might succeed if retried.")
    .report(PgLogLevel::ERROR);
    unreachable!("an ERROR report does not return")
}

/// Where the transaction's locks stand, for a subtransaction to roll back to.
#[must_use]
pub fn mark() -> usize {
    REGISTRY.with_borrow(registry::Registry::mark)
}

/// A subtransaction rolled back: the lock manager released what it took.
pub fn rollback(mark: usize) {
    REGISTRY.with_borrow_mut(|r| r.rollback(mark));
}

/// The transaction ended: every lock was released.
pub fn clear() {
    REGISTRY.with_borrow_mut(registry::Registry::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_stable() {
        // FNV-1a over [1, 0] (attnum 1), 0x00, "42".
        assert_eq!(value_hash(&[1], "42"), value_hash(&[1], "42"));
        assert_eq!(value_hash(&[1], "42"), 0x0538_46ce);
    }

    #[test]
    fn the_columns_are_part_of_the_value() {
        assert_ne!(value_hash(&[1], "42"), value_hash(&[2], "42"));
        assert_ne!(value_hash(&[1, 2], "42"), value_hash(&[1], "42"));
        assert_ne!(value_hash(&[1], "42"), value_hash(&[1], "43"));
    }

    #[test]
    fn tags_are_private_advisory_tags() {
        let value = value_tag(5, 16_384, 7);
        assert_eq!(value.locktag_field4, 0x5476);
        assert_eq!(
            u32::from(value.locktag_type),
            pg_sys::LockTagType::LOCKTAG_ADVISORY
        );
        assert_eq!(
            u32::from(value.locktag_lockmethodid),
            pg_sys::USER_LOCKMETHOD
        );
        assert_eq!(
            (
                value.locktag_field1,
                value.locktag_field2,
                value.locktag_field3
            ),
            (5, 16_384, 7)
        );
        let relation = relation_tag(5, 16_384, Space::Values);
        assert_eq!(relation.locktag_field4, 0x5477);
        assert_eq!(relation.locktag_field3, 0);
        assert_eq!(relation_tag(5, 16_384, Space::Keys).locktag_field3, 1);
    }

    #[test]
    fn modes_conflict_as_the_protocol_needs() {
        // PostgreSQL's conflict table (lock.c), restricted to the four modes used.
        const TABLE: [[bool; 4]; 4] = [
            [false, false, false, true],
            [false, false, true, true],
            [false, true, false, true],
            [true, true, true, true],
        ];
        let conflicts = |a: pg_sys::LOCKMODE, b: pg_sys::LOCKMODE| {
            let rank = |m| match m {
                2 => 0, // RowShare
                3 => 1, // RowExclusive
                5 => 2, // Share
                7 => 3, // Exclusive
                _ => unreachable!(),
            };
            TABLE[rank(a)][rank(b)]
        };
        let (r, w) = (Side::Refresh, Side::Writer);
        assert!(conflicts(value_mode(r), value_mode(w)));
        assert!(!conflicts(value_mode(r), value_mode(r)));
        assert!(!conflicts(intent_mode(r), intent_mode(w)));
        assert!(conflicts(escalated_mode(r), intent_mode(w)));
        assert!(conflicts(escalated_mode(w), intent_mode(r)));
        assert!(!conflicts(escalated_mode(r), intent_mode(r)));
    }

    #[test]
    fn each_level_has_its_policy() {
        assert_eq!(Policy::of_level(0), Policy::Wait);
        assert_eq!(Policy::of_level(pg_sys::XACT_READ_COMMITTED), Policy::Wait);
        assert_eq!(
            Policy::of_level(pg_sys::XACT_REPEATABLE_READ),
            Policy::FailFast
        );
        assert_eq!(Policy::of_level(pg_sys::XACT_SERIALIZABLE), Policy::Skip);
    }
}
