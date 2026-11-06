//! Value locks: how concurrent transactions maintaining the same TVIEW rows wait
//! for each other (ADR 0207).
//!
//! A writer locks, exclusively, the join values of the rows it changed before it
//! looks TVIEW rows up by them; a refresh locks, shared, the join values its rows
//! read before it computes them. Past a threshold, a transaction locks the
//! relation instead of its values. Locks live in PostgreSQL's lock manager under a
//! private advisory tag and are held to the end of the transaction. Writers run
//! at READ COMMITTED: a transaction snapshot could not see what it waited for.

pub mod key;
mod pass;
pub mod reads;
mod registry;

pub use pass::Pass;
use pgrx::pg_sys;
use pgrx::prelude::*;
pub use registry::Mark;
use registry::{Held, REGISTRY};

thread_local! {
    /// `(relid, role)` → whether the role may write the relation, this transaction.
    static WRITABLE: std::cell::RefCell<std::collections::HashMap<(u32, pg_sys::Oid), bool>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// `(relid, attnum)` → how a value of a column writers lock values of is locked.
    static LOCK_VALUES: crate::cache::Memo<(u32, i16), Option<LockValue>> = crate::cache::Memo::new();
}

/// `field4` of a value lock's tag. `pg_advisory_lock()` uses 1 and 2.
pub const VALUE_TAG: u16 = 0x5476;
/// `field4` of a relation's intent and escalated locks.
pub const RELATION_TAG: u16 = 0x5477;
/// `field4` of a TVIEW's registration lock.
pub const REGISTRATION_TAG: u16 = 0x5478;

/// Which end of the protocol takes a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side {
    /// A refresh, about to compute rows from the values it reads.
    Refresh,
    /// A writer, about to look rows up by the values it changed.
    Writer,
}

/// Where in a relation a lock lives: the values of its columns, or (a TVIEW's
/// table) the keys of rows being created. Each has its own intent and
/// escalated locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Refuse a write path (a trigger capturing a write, a refresh, a rebuild) in a
/// transaction at REPEATABLE READ or SERIALIZABLE, with 0A000, before it writes
/// (ADR 0207). Its snapshot cannot see what it would wait for on the value
/// locks: a row it computes or a row it looks up could be one a concurrent
/// transaction already changed. Reads are not concerned.
pub fn require_read_committed() {
    // SAFETY: XactIsoLevel is a plain backend global, set for the transaction.
    let level = u32::try_from(unsafe { pg_sys::XactIsoLevel }).unwrap_or(0);
    let name = match level {
        pg_sys::XACT_REPEATABLE_READ => "REPEATABLE READ",
        pg_sys::XACT_SERIALIZABLE => "SERIALIZABLE",
        _ => return,
    };
    pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_FEATURE_NOT_SUPPORTED,
        format!(
            "pg_tviews: a {name} transaction cannot write to a table TVIEWs read, nor \
             refresh TVIEWs"
        ),
        function_name!(),
    )
    .set_hint(
        "Set READ COMMITTED for transactions that write to tables feeding TVIEWs. \
         Reads of TVIEWs work at any level.",
    )
    .report(PgLogLevel::ERROR);
}

const fn value_mode(side: Side) -> pg_sys::LOCKMODE {
    match side {
        Side::Refresh => pg_sys::ShareLock.cast_signed(),
        Side::Writer => pg_sys::ExclusiveLock.cast_signed(),
    }
}

/// SipHash-2-4, keyed with the server's `key`, of the columns' attnums, a zero
/// byte and the value's text, truncated to 32 bits: the same in every backend
/// of the server, and not computable without its key. A collision only makes
/// two values share a lock.
#[must_use]
pub fn value_hash(key: &key::Key, attnums: &[i16], value: &str) -> u32 {
    use std::hash::Hasher;
    let mut hasher = siphasher::sip::SipHasher24::new_with_key(key);
    for attnum in attnums {
        hasher.write(&attnum.to_le_bytes());
    }
    hasher.write(&[0]);
    hasher.write(value.as_bytes());
    #[allow(clippy::cast_possible_truncation)] // Reason: a lock tag field holds 32 bits
    let hash = hasher.finish() as u32;
    hash
}

/// The tag (`objid` in `pg_locks`) of the value lock on `value` of `column` of
/// `relid`, or of the key `value` of the TVIEW table `relid` being created when
/// `column` is `None`. Superusers only: the tag is keyed with the server's
/// secret, so that no other role can tell which value a lock in `pg_locks` is
/// on.
///
/// # Errors
/// `PermissionDenied` for a caller that is not a superuser; `InvalidInput` for
/// a column `relid` lacks; `WrongState` without the server's key; a failed
/// conversion of `value` to the column's type.
pub fn value_lock_tag(
    relid: pg_sys::Oid,
    column: Option<&str>,
    value: &str,
) -> crate::TViewResult<i64> {
    // SAFETY: a role check of the current user.
    if !unsafe { pg_sys::superuser() } {
        return Err(crate::TViewError::PermissionDenied {
            reason: "only a superuser may compute value-lock tags".to_string(),
        });
    }
    let key = key::current()?;
    let Some(column) = column else {
        return Ok(i64::from(value_hash(&key, &[-1], value)));
    };
    let relation = crate::utils::qualified_relname_from_oid(relid)?;
    let invalid = || crate::TViewError::InvalidInput {
        parameter: "column_name".to_string(),
        reason: format!("{relation} has no column {column}"),
    };
    let name = std::ffi::CString::new(column).map_err(|_| invalid())?;
    // SAFETY: a syscache lookup by OID and column name; InvalidAttrNumber when
    // there is no such column.
    let attnum = unsafe { pg_sys::get_attnum(relid, name.as_ptr()) };
    if attnum <= 0 {
        return Err(invalid());
    }
    let lock = lock_value(relid.to_u32(), attnum)?.ok_or_else(invalid)?;
    let _path = crate::owner::CatalogPath::pin();
    let text = crate::utils::spi::one::<String>(
        &format!("SELECT {}", lock.of("$1")),
        &[crate::utils::spi::text(value)],
    )?
    .unwrap_or_default();
    Ok(i64::from(value_hash(&key, &[attnum], &text)))
}

/// The server's value-lock key, or the error raised without it.
fn lock_key() -> key::Key {
    key::current().unwrap_or_else(|e| e.raise())
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

/// The tag of the registration lock of the entity whose name hashes to `hash`.
#[must_use]
pub const fn registration_tag(database: u32, hash: u32) -> pg_sys::LOCKTAG {
    tag(database, hash, 0, REGISTRATION_TAG)
}

/// The registration lock's hash of `entity`, keyed with the server's secret
/// `key` as value tags are: no role can find, offline, a name of its own whose
/// tag is another entity's.
#[must_use]
fn registration_hash(key: &key::Key, entity: &str) -> u32 {
    value_hash(key, &[], entity)
}

/// Hold the registration lock of `entity` until the transaction ends: calls
/// registering, changing or dropping one entity run one after the other. The
/// tag is private to `pg_tviews` (`pg_advisory_lock()` never takes it), so no
/// other role can hold it; two entities sharing a hash only take turns.
pub fn lock_registration(entity: &str) {
    let hash = registration_hash(&lock_key(), entity);
    // SAFETY: a transaction is in progress (DDL runs in one) and the tag is a
    // well-formed advisory tag; the lock belongs to the current resource owner
    // and ends with the transaction. Waiting honours lock_timeout and deadlock
    // detection, which raise an ERROR.
    unsafe {
        pg_sys::LockAcquire(
            &registration_tag(database(), hash),
            pg_sys::ExclusiveLock.cast_signed(),
            false,
            false,
        );
    }
}

fn database() -> u32 {
    // SAFETY: MyDatabaseId is set once the backend is connected to a database.
    unsafe { pg_sys::MyDatabaseId }.to_u32()
}

/// Take the locks of `pass` this transaction doesn't hold, to the end of the
/// transaction, in one fixed order (see [`Pass`]): values already held are
/// skipped, a relation's intent lock is taken with its first value, and past
/// `pg_tviews.lock_escalation_threshold` values of one relation, space and side
/// the relation is locked instead.
pub fn take(pass: &Pass) {
    if pass.is_empty() {
        return;
    }
    let threshold = crate::config::lock_escalation_threshold();
    let caller = crate::owner::caller();
    let may_lock_whole = |relid: u32| may_write(relid, caller);
    let key = lock_key();
    let planned = REGISTRY.with_borrow(|r| pass::plan(pass, r, threshold, &may_lock_whole, &key));
    let (ops, escalations) = match planned {
        Ok(planned) => planned,
        Err(pass::TooManyLinks { max }) => crate::TViewError::RefreshLimit {
            limit: crate::error::Limit::Links { max },
        }
        .raise(),
    };
    for _ in 0..escalations {
        crate::metrics::metrics_api::record_value_lock_escalation();
    }
    let database = database();
    for op in ops {
        match op {
            pass::Op::Relation { relid, space, mode } => {
                acquire(&relation_tag(database, relid, space), mode.lockmode());
                REGISTRY.with_borrow_mut(|r| {
                    for held in mode.held(relid, space) {
                        r.record(held);
                    }
                });
            }
            pass::Op::Value {
                relid,
                space,
                hash,
                side,
            } => {
                acquire(&value_tag(database, relid, hash), value_mode(side));
                REGISTRY.with_borrow_mut(|r| {
                    r.record(Held::Value {
                        relid,
                        space,
                        side,
                        hash,
                    })
                });
                crate::metrics::metrics_api::record_value_lock();
            }
        }
    }
}

/// Lock `values` of `target` for `side`, to the end of the transaction, as one
/// [`take`].
pub fn lock_values(target: &LockTarget, side: Side, values: &[String]) {
    let mut pass = Pass::default();
    pass.values(target, side, values);
    take(&pass);
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
    let mut pass = Pass::default();
    pass.values_in(Space::Keys, &target, Side::Writer, keys);
    take(&pass);
}

/// Lock `values` of `target` for `side`, as [`lock_values`] does, unless that
/// would lock the relation as a whole: past `pg_tviews.lock_escalation_threshold`
/// values, nothing is locked and `false` is returned, for the caller to find its
/// rows another way. An escalated lock blocks every writer of the relation, not
/// only the writers of these values. A relation already locked as a whole holds
/// the values.
pub fn lock_values_unescalated(target: &LockTarget, side: Side, values: &[String]) -> bool {
    let mut pass = Pass::default();
    pass.values(target, side, values);
    let threshold = crate::config::lock_escalation_threshold();
    // Asked as if the relation could be locked whole: whether it would be.
    let whole = REGISTRY.with_borrow(|r| {
        pass::plan(&pass, r, threshold, &|_| true, &lock_key())
            .map_or(true, |(_, escalations)| escalations > 0)
    });
    if whole {
        return false;
    }
    take(&pass);
    true
}

/// Whether this transaction holds the key lock of `key`, a row of `tview` it
/// may create ([`lock_new_keys`]), itself or through the key space's escalated
/// lock.
#[must_use]
pub fn holds_new_key(tview: pg_sys::Oid, key: &str) -> bool {
    let relid = tview.to_u32();
    let held = Held::Value {
        relid,
        space: Space::Keys,
        side: Side::Writer,
        hash: value_hash(&lock_key(), &[-1], key),
    };
    REGISTRY.with_borrow(|r| r.escalated(relid, Space::Keys, Side::Writer) || r.holds(&held))
}

/// Lock relation `relid` as a whole for `side`, to the end of the transaction:
/// a writer that changes every value (a full refresh), or a transaction past the
/// escalation threshold.
pub fn lock_relation(relid: u32, side: Side) {
    let mut pass = Pass::default();
    pass.relation(relid, side);
    take(&pass);
}

/// Writer side: the locks on the values a statement wrote to `mapping`'s table,
/// added to `pass`, which takes them before any query finds TVIEW rows by those
/// values. `values` runs, over the changed rows (both images of an update), the
/// lock value of a read-set column ([`lock_value`]: an expression of its quoted
/// name) and returns the distinct results. A read set with no column (the table
/// is locked as a whole by refreshes), or whose column is gone, takes the intent
/// lock alone.
///
/// # Errors
/// What `values` returns.
pub fn changed_rows(
    mapping: &crate::lineage::KeyMapping,
    pass: &mut Pass,
    mut values: impl FnMut(&str) -> crate::TViewResult<Vec<String>>,
) -> crate::TViewResult<()> {
    for set in &mapping.reads {
        let column = if set.attnum == 0 {
            None
        } else {
            lock_value(mapping.relid, set.attnum)?
        };
        let Some(column) = column else {
            pass.intent(mapping.relid, Side::Writer);
            continue;
        };
        let target = LockTarget {
            relid: mapping.relid,
            attnums: vec![set.attnum],
        };
        pass.values(&target, Side::Writer, &values(&column.of(&column.name))?);
    }
    Ok(())
}

/// The keys of an embedded TVIEW's rows (`pk_<child>`), locked by `side` on its
/// table: a writer before it looks up the parents holding them, a refresh before
/// it computes parents from them.
pub fn lock_embedded_keys(child_table: pg_sys::Oid, side: Side, keys: &[String]) {
    lock_values(&embedded_keys_target(child_table), side, keys);
}

/// Where the keys of an embedded TVIEW's rows are locked: its table, no column.
#[must_use]
pub const fn embedded_keys_target(child_table: pg_sys::Oid) -> LockTarget {
    LockTarget {
        relid: child_table.to_u32(),
        attnums: Vec::new(),
    }
}

/// How a value of a column is locked: the hash its type's equality uses (the
/// support function of the type's default hash operator class), in the
/// column's type and collation, as text. Equal values then lock the same tag
/// whatever their text forms (`5` and `5.00`, `Ann` and `ann` under a
/// case-insensitive collation), on the writer's side (the column itself) and
/// on the refresh's (the value it is compared with). A type with no hash
/// operator class is locked by its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockValue {
    /// The column's quoted name.
    pub name: String,
    prefix: String,
    suffix: String,
}

impl LockValue {
    /// The lock value of `expr`, a value compared with the column.
    #[must_use]
    pub fn of(&self, expr: &str) -> String {
        format!("{}({expr}){}", self.prefix, self.suffix)
    }
}

/// The lock value of column `attnum` of `relid` (cached); `None` when it is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn lock_value(relid: u32, attnum: i16) -> crate::TViewResult<Option<LockValue>> {
    const SQL: &str = "\
        SELECT pg_catalog.quote_ident(a.attname::pg_catalog.text), \
               pg_catalog.format_type(a.atttypid, a.atttypmod), \
               (SELECT p.amproc::pg_catalog.regproc::pg_catalog.text \
                  FROM pg_catalog.pg_opclass oc \
                  JOIN pg_catalog.pg_am am ON am.oid = oc.opcmethod AND am.amname = 'hash' \
                  JOIN pg_catalog.pg_amproc p ON p.amprocfamily = oc.opcfamily \
                   AND p.amprocnum = 1 AND p.amproclefttype = oc.opcintype \
                   AND p.amprocrighttype = oc.opcintype \
                 WHERE oc.opcdefault \
                   AND (oc.opcintype = t.base OR EXISTS ( \
                        SELECT 1 FROM pg_catalog.pg_cast c WHERE c.castsource = t.base \
                           AND c.casttarget = oc.opcintype AND c.castmethod = 'b')) \
                 ORDER BY oc.opcintype = t.base DESC LIMIT 1), \
               CASE WHEN a.attcollation NOT IN (0, 100) THEN \
                    (SELECT pg_catalog.quote_ident(n.nspname) || '.' \
                            || pg_catalog.quote_ident(co.collname) \
                       FROM pg_catalog.pg_collation co \
                       JOIN pg_catalog.pg_namespace n ON n.oid = co.collnamespace \
                      WHERE co.oid = a.attcollation) END \
          FROM pg_catalog.pg_attribute a \
          JOIN pg_catalog.pg_type ty ON ty.oid = a.atttypid, \
               LATERAL (SELECT CASE WHEN ty.typtype = 'd' THEN ty.typbasetype \
                                    ELSE a.atttypid END AS base) t \
         WHERE a.attrelid = $1 AND a.attnum = $2 AND NOT a.attisdropped";
    let key = (relid, attnum);
    if let Some(value) = LOCK_VALUES.with(|m| m.get(&key)) {
        return Ok(value);
    }
    let args = [
        crate::utils::spi::oid(pg_sys::Oid::from(relid)),
        crate::utils::spi::int2(attnum),
    ];
    let row = crate::utils::spi::catalog_text_rows(SQL, &args)?
        .into_iter()
        .next();
    let value = row.and_then(|row| {
        let mut cells = row.into_iter();
        let (name, ty, hash, collation) = (
            cells.next().flatten()?,
            cells.next().flatten()?,
            cells.next().flatten(),
            cells.next().flatten(),
        );
        let collate = collation
            .map(|c| format!(" COLLATE {c}"))
            .unwrap_or_default();
        let (prefix, suffix) = match hash {
            Some(hash) => (
                format!("{hash}(("),
                format!(")::{ty}{collate})::pg_catalog.text"),
            ),
            None => (
                "((".to_string(),
                format!(")::{ty}{collate})::pg_catalog.text"),
            ),
        };
        Some(LockValue {
            name,
            prefix,
            suffix,
        })
    });
    crate::cache::watch(&[pg_sys::Oid::from(relid)]);
    LOCK_VALUES.with(|m| m.insert(key, value.clone()));
    Ok(value)
}

/// Acquire `tag` in `mode`: at once, or after waiting for the transactions that
/// hold a conflicting lock. A wait is counted and timed.
fn acquire(tag: &pg_sys::LOCKTAG, mode: pg_sys::LOCKMODE) {
    // SAFETY: a transaction is in progress (a trigger or a flush runs inside one)
    // and the tag is a well-formed advisory tag; the lock belongs to the current
    // resource owner and ends with the (sub)transaction.
    let got = unsafe { pg_sys::LockAcquire(tag, mode, false, true) };
    if got != pg_sys::LockAcquireResult::LOCKACQUIRE_NOT_AVAIL {
        return;
    }
    let start = std::time::Instant::now();
    // SAFETY: as above; waiting honours lock_timeout and deadlock detection,
    // which raise an ERROR.
    unsafe {
        pg_sys::LockAcquire(tag, mode, false, false);
    }
    crate::metrics::metrics_api::record_value_lock_wait(start.elapsed().as_nanos());
}

/// Where the transaction's locks stand, for a subtransaction to roll back to.
#[must_use]
pub fn mark() -> Mark {
    REGISTRY.with_borrow(registry::Registry::mark)
}

/// A subtransaction rolled back: the lock manager released what it took.
pub fn rollback(mark: Mark) {
    REGISTRY.with_borrow_mut(|r| r.rollback(mark));
}

/// The transaction changed a link: it inserted or deleted rows of a table a
/// TVIEW reads, or changed a column of one that a TVIEW's rows are linked by
/// (#263). From now on its refreshes lock the values they read.
pub fn note_link_change() {
    REGISTRY.with_borrow_mut(|r| r.links_changed = true);
}

/// Enqueue a refresh of every row of `entity`'s TVIEW: rows rebuilt in full may
/// come and go, a link change ([`note_link_change`]).
pub fn enqueue_refresh_all(entity: &str) {
    note_link_change();
    crate::queue::enqueue_refresh_all(entity);
}

/// Whether the transaction changed a link ([`note_link_change`]).
#[must_use]
pub fn links_changed() -> bool {
    REGISTRY.with_borrow(|r| r.links_changed)
}

/// Note a write of `event` to the table `relid` reads as `mapping`: an insert or
/// a delete changes a link; an update does when `changed(links)` says one of the
/// columns `mapping` is linked by changed (unknown links: always).
pub fn note_write(
    mapping: Option<&crate::lineage::KeyMapping>,
    event: crate::queue::Event,
    changed: impl FnOnce(&[i16]) -> bool,
) {
    if links_changed() {
        return;
    }
    let link = match (event, mapping.and_then(|m| m.links.as_deref())) {
        (crate::queue::Event::Update, Some(links)) => !links.is_empty() && changed(links),
        _ => true,
    };
    if link {
        note_link_change();
    }
}

/// Whether `role` may write `relid` (`INSERT` or `UPDATE`): only then does a
/// refresh it caused lock the whole relation, which blocks every writer of it.
/// Answered once per relation and role in a transaction.
fn may_write(relid: u32, role: pg_sys::Oid) -> bool {
    if let Some(known) = WRITABLE.with_borrow(|w| w.get(&(relid, role)).copied()) {
        return known;
    }
    let mut missing = false;
    // SAFETY: an ACL check by OID; the `_ext` check reports a missing relation
    // instead of raising. Any of the two privileges passes.
    let may = unsafe {
        pg_sys::pg_class_aclcheck_ext(
            pg_sys::Oid::from(relid),
            role,
            pg_sys::AclMode::from(pg_sys::ACL_INSERT | pg_sys::ACL_UPDATE),
            &raw mut missing,
        )
    } == pg_sys::AclResult::ACLCHECK_OK
        && !missing;
    WRITABLE.with_borrow_mut(|w| w.insert((relid, role), may));
    may
}

/// The transaction ended: every lock was released.
pub fn clear() {
    REGISTRY.with_borrow_mut(registry::Registry::clear);
    WRITABLE.with_borrow_mut(std::collections::HashMap::clear);
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: key::Key = [7; 16];

    #[test]
    fn siphash_matches_its_reference_vector() {
        // SipHash-2-4 paper, appendix A: key 00..0f, message 00..0e.
        use std::hash::Hasher;
        let key: key::Key = std::array::from_fn(|i| u8::try_from(i).unwrap());
        let message: Vec<u8> = (0..15).collect();
        let mut hasher = siphasher::sip::SipHasher24::new_with_key(&key);
        hasher.write(&message);
        assert_eq!(hasher.finish(), 0xa129_ca61_49be_45e5);
    }

    #[test]
    fn the_hash_depends_on_the_key() {
        assert_eq!(value_hash(&KEY, &[1], "42"), value_hash(&KEY, &[1], "42"));
        assert_ne!(
            value_hash(&KEY, &[1], "42"),
            value_hash(&[8; 16], &[1], "42")
        );
        // The unkeyed FNV-1a of the same input.
        assert_ne!(value_hash(&KEY, &[1], "42"), 0x0538_46ce);
    }

    #[test]
    fn the_columns_are_part_of_the_value() {
        assert_ne!(value_hash(&KEY, &[1], "42"), value_hash(&KEY, &[2], "42"));
        assert_ne!(
            value_hash(&KEY, &[1, 2], "42"),
            value_hash(&KEY, &[1], "42")
        );
        assert_ne!(value_hash(&KEY, &[1], "42"), value_hash(&KEY, &[1], "43"));
    }

    #[test]
    fn a_registration_hash_depends_on_the_server_key() {
        let (one, other) = ([1; 16], [2; 16]);
        assert_eq!(
            super::registration_hash(&one, "post"),
            super::registration_hash(&one, "post")
        );
        assert_ne!(
            super::registration_hash(&one, "post"),
            super::registration_hash(&other, "post")
        );
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
        let registration = registration_tag(5, 99);
        assert_eq!(registration.locktag_field4, 0x5478);
        assert_eq!(
            u32::from(registration.locktag_type),
            pg_sys::LockTagType::LOCKTAG_ADVISORY
        );
        assert_eq!(
            u32::from(registration.locktag_lockmethodid),
            pg_sys::USER_LOCKMETHOD
        );
        assert_eq!(
            (registration.locktag_field1, registration.locktag_field2),
            (5, 99)
        );
    }

    #[test]
    fn modes_conflict_as_the_protocol_needs() {
        use pass::RelationMode as M;
        // PostgreSQL's conflict table (lock.c), restricted to the five modes used.
        const TABLE: [[bool; 5]; 5] = [
            [false, false, false, false, true],
            [false, false, true, true, true],
            [false, true, false, true, true],
            [false, true, true, true, true],
            [true, true, true, true, true],
        ];
        let conflicts = |a: pg_sys::LOCKMODE, b: pg_sys::LOCKMODE| {
            let rank = |m| match m {
                2 => 0, // RowShare
                3 => 1, // RowExclusive
                5 => 2, // Share
                6 => 3, // ShareRowExclusive
                7 => 4, // Exclusive
                _ => unreachable!(),
            };
            TABLE[rank(a)][rank(b)]
        };
        let (r, w) = (Side::Refresh, Side::Writer);
        assert!(conflicts(value_mode(r), value_mode(w)));
        assert!(!conflicts(value_mode(r), value_mode(r)));
        let mode = M::lockmode;
        assert!(!conflicts(mode(M::RefreshIntent), mode(M::WriterIntent)));
        assert!(conflicts(mode(M::RefreshEscalated), mode(M::WriterIntent)));
        assert!(conflicts(mode(M::WriterEscalated), mode(M::RefreshIntent)));
        assert!(!conflicts(
            mode(M::RefreshEscalated),
            mode(M::RefreshIntent)
        ));
        // Writing values while reading the whole relation: waits for writers
        // and for whole-relation reads, not for refreshes of values.
        let both = mode(M::WriterIntentRefreshEscalated);
        assert!(conflicts(both, mode(M::WriterIntent)));
        assert!(conflicts(both, mode(M::RefreshEscalated)));
        assert!(conflicts(both, both));
        assert!(!conflicts(both, mode(M::RefreshIntent)));
    }
}
