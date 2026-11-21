//! Every decision to refresh less than a full recompute, as a pure function of
//! plain values (skips.md, theorems.md).
//!
//! A decision returns *skip* (or *patch*, or *prune*) only when its safety
//! condition is established by a positive fact among its inputs; a missing or
//! unknown input means refresh. Nothing here reads a datum, the catalog, a
//! setting or the clock, and nothing names `pgrx`: binary comparison of datums
//! stays with the trigger, and every fact about the database comes in
//! [`Env`], computed once per statement by named functions outside this module
//! (`scripts/check-module-layers.sh` enforces it). The same files build as a
//! standalone crate without `pgrx` (`decide/Cargo.toml`), whose `cargo test`
//! runs the exhaustive tests below, as a model checker of the code as written
//! will (D11).
//!
//! The row-security rule (a TVIEW is refused while row security applies on a
//! table it reads to a role it reads it as) is a precondition of every decision,
//! checked before any of them runs: no input here describes row security.

mod attnums;
#[cfg(test)]
mod tests;

pub use attnums::Attnums;

/// What a TVIEW's definition reads of one table.
///
/// Every column it reads, or unknown (a whole-row read, a system column, a declared function reading it,
/// a plan stored without it). `Unknown` rules out every column skip and patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadSet {
    /// Every column the definition reads of the table, by attnum.
    Complete(Attnums),
    /// Not known column by column: every column counts as read.
    Unknown,
}

impl ReadSet {
    /// The columns, when they are known.
    #[must_use]
    pub const fn complete(&self) -> Option<&Attnums> {
        match self {
            Self::Complete(read) => Some(read),
            Self::Unknown => None,
        }
    }
}

/// The write a decision is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Insert,
    Update,
    Delete,
}

/// Facts about the database a decision depends on, each computed outside this
/// module by a named function, once per statement (or per TVIEW and command).
/// A fact that could not be established is false.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // Reason: each is one independent fact, named.
pub struct Env {
    /// The TVIEW's stored plan is this release's (not `needs_reregister`, not
    /// decoded from an earlier release's plan).
    pub plan_current: bool,
    /// The backing view's chain, as it stands now, reads the table through the
    /// columns it names only (no whole-row or system-column read appeared since
    /// the plan was derived, through a view of the chain replaced since).
    pub chain_reads_named: bool,
    /// The TVIEW's owner holds every privilege its refresh needs to read what
    /// it reads of the table (USAGE on the schema, SELECT on those columns).
    pub owner_reads_all: bool,
    /// `pg_tviews.direct_patch` is on.
    pub direct_patch_enabled: bool,
}

impl Env {
    /// Whether the stored read set may be trusted for the table at all.
    const fn reads_hold(self) -> bool {
        self.plan_current && self.chain_reads_named && self.owner_reads_all
    }
}

/// The columns the TVIEW reads of the table, when the stored read set holds in
/// `env`; `None` otherwise.
///
/// Theorem: T1 (docs/proofs/theorems.md).
#[must_use]
pub fn trusted_read_set(read: &ReadSet, env: Env) -> Option<&Attnums> {
    read.complete().filter(|_| env.reads_hold())
}

/// S03 (row trigger): whether a write to one row of a table may skip the
/// TVIEW. Only an UPDATE that changed none of the columns it reads (`changed`,
/// by binary comparison of the two images) may.
///
/// Theorem: T1 (docs/proofs/theorems.md).
#[must_use]
pub fn column_skip(event: Event, read: &ReadSet, changed: &Attnums, env: Env) -> bool {
    event == Event::Update
        && trusted_read_set(read, env).is_some_and(|read| read.is_disjoint(changed))
}

/// S11 (statement trigger): whether a statement's rows may skip the TVIEW.
///
/// Only an UPDATE whose rows changed none of the columns it reads (`changed`: the union
/// over the rows, by binary comparison), on a table read without a virtual
/// generated column (`virtual_read`: such a column is NULL in the rows a
/// trigger sees).
///
/// Theorem: T2 (docs/proofs/theorems.md).
#[must_use]
pub fn statement_skip(
    event: Event,
    read: &ReadSet,
    changed: &Attnums,
    virtual_read: bool,
    env: Env,
) -> bool {
    !virtual_read && column_skip(event, read, changed, env)
}

/// S12 (pair filter): the columns an UPDATE's row pairs are compared on, a
/// pair equal on all of them dropped from the delta; `None`: every row is kept.
///
/// Theorem: T3 (docs/proofs/theorems.md).
#[must_use]
pub fn pair_filter(event: Event, read: &ReadSet, env: Env) -> Option<&Attnums> {
    if event != Event::Update {
        return None;
    }
    trusted_read_set(read, env).filter(|read| !read.is_empty())
}

/// A column a patch copies into `data`, and whether the copy is exact.
///
/// Exact: the value the patch writes renders as `to_jsonb()` of the column renders it
/// (false for `numeric`, `jsonb` that may hold numbers, floating types: G2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Copied {
    pub attnum: i16,
    pub exact: bool,
}

/// S05/S06 (direct patch of the TVIEW's own row): the columns to copy.
///
/// The columns to copy into its `data` in place of a recompute, or `None` to
/// recompute. Only an UPDATE that
/// leaves the key alone (`key_changed`), changed some column the TVIEW reads,
/// and every one of those is a column `data` copies (`copied`: the plan's
/// direct fields) exactly.
///
/// Theorem: T4 (docs/proofs/theorems.md).
#[must_use]
pub fn direct_patch(
    event: Event,
    read: &ReadSet,
    changed: &Attnums,
    key_changed: bool,
    copied: &[Copied],
    env: Env,
) -> Option<Attnums> {
    if event != Event::Update || key_changed || !env.direct_patch_enabled {
        return None;
    }
    let read = trusted_read_set(read, env)?;
    let fields = changed.intersection(read);
    let exact = |attnum: i16| copied.iter().any(|c| c.attnum == attnum && c.exact);
    (!fields.is_empty() && fields.iter().all(exact)).then_some(fields)
}

/// S16 (fan-out patch): whether an UPDATE's rows may patch the rows they reach.
///
/// They are written into every TVIEW row they reach by the fields `data`
/// copies (`copied`), instead of a recompute of those rows, when the key the rows are looked up by is unchanged in
/// every pair (`key_changed`), every row was paired (`paired`), and every
/// changed column the TVIEW reads is a copied field, copied exactly.
///
/// Theorem: T5 (docs/proofs/theorems.md).
#[must_use]
pub fn fanout_patch(
    event: Event,
    read: &ReadSet,
    changed: &Attnums,
    key_changed: bool,
    paired: bool,
    copied: &[Copied],
    env: Env,
) -> bool {
    if event != Event::Update || key_changed || !paired || !env.direct_patch_enabled {
        return false;
    }
    let Some(read) = trusted_read_set(read, env) else {
        return false;
    };
    let exact = |attnum: i16| copied.iter().any(|c| c.attnum == attnum && c.exact);
    changed.intersection(read).iter().all(exact)
}

/// How a no-op guard compares a stored row with a recomputed or patched one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Equality {
    /// Byte for byte (`*=` on the row records): `1.5` and `1.50` differ.
    Binary,
}

/// S31/S35 (no-op guard): the comparison by which a recomputed or patched row
/// equal to the stored one is left unwritten (D10). Made in SQL; its only
/// input is the two rows.
///
/// Theorem: T6 (docs/proofs/theorems.md).
#[must_use]
pub const fn noop_guard() -> Equality {
    Equality::Binary
}

/// S27 (edge prune): whether a parent reached along a document edge is left alone.
///
/// The parent reads the child only through the child's stored row
/// (`document_edge`), and the child row did not change in this flush.
///
/// Theorem: T7 (docs/proofs/theorems.md).
#[must_use]
pub const fn edge_prune(document_edge: bool, child_changed_in_flush: bool) -> bool {
    document_edge && !child_changed_in_flush
}

/// S14 (lookup filter): whether an `all_keys` write refreshes the whole TVIEW.
///
/// Only under `full_refresh`. A guarded table (`guard`: the
/// conditions on constants of the reads that leave it unmapped) does only when
/// a changed row meets them (`guard_met`, evaluated over both images); an
/// unguarded one always does.
///
/// Theorem: T10 (docs/proofs/theorems.md).
#[must_use]
pub const fn lookup_filter(full_refresh: bool, guarded: bool, guard_met: bool) -> bool {
    full_refresh && (!guarded || guard_met)
}
