//! The lifecycle of an UNLOGGED TVIEW's rows: trusted until PostgreSQL resets the
//! table, then refilled once.

/// Whether an UNLOGGED TVIEW's rows can be trusted, and its fill when they can't.
///
/// PostgreSQL empties every UNLOGGED table on a crash restart and on promotion,
/// and leaves no other trace of it. `pg_tview_valid` is UNLOGGED too and holds
/// a row per UNLOGGED TVIEW whose rows are trusted, by entity, so the reset
/// empties both together. A missing row makes the next write fill that TVIEW
/// from its view; inserting the row is the claim, so concurrent writers wait for
/// the claimer instead of filling twice (#214). The rows are extension
/// configuration: `pg_dump` writes them exactly when it writes UNLOGGED tables'
/// rows, so a restore brings back both or neither (#236).
pub mod validity {
    use crate::TViewResult;
    use crate::catalog::TviewMeta;
    use pgrx::pg_sys::{self, Oid};
    use pgrx::prelude::*;

    thread_local! {
        /// The TVIEW tables this backend found trusted (or filled).
        static CHECKED: std::cell::RefCell<std::collections::HashSet<u32>> =
            std::cell::RefCell::new(std::collections::HashSet::new());

        /// This transaction filled a TVIEW: an abort undoes it.
        static FILLED_IN_XACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

        /// The TVIEW tables whose refill this transaction claimed, with the
        /// (sub)transaction nesting level that claimed each.
        static CLAIMS: std::cell::RefCell<Vec<(Oid, i32)>> =
            const { std::cell::RefCell::new(Vec::new()) };

        /// The entities this transaction's writes found empty at their first
        /// check in the backend ([`refill_if_restored_empty`]).
        static FOUND_EMPTY: std::cell::RefCell<std::collections::HashSet<String>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
    }

    fn table() -> String {
        format!("{}.pg_tview_valid", crate::utils::ext_schema())
    }

    fn unlogged(table_oid: Oid) -> bool {
        // SAFETY: a syscache lookup; '\0' for a relation that no longer exists.
        let persistence = unsafe { pg_sys::get_rel_persistence(table_oid) };
        u8::try_from(persistence).ok() == Some(pg_sys::RELPERSISTENCE_UNLOGGED)
    }

    /// Whether `entity`'s table, `table_oid`, is UNLOGGED and its rows can't be
    /// trusted.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be read.
    pub fn needs_fill(entity: &str, table_oid: Oid) -> TViewResult<bool> {
        if !unlogged(table_oid) {
            return Ok(false);
        }
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE entity = $1)",
            table()
        );
        Ok(!probe(&sql, &[crate::utils::spi::text(entity)])?)
    }

    /// Record that the rows of `entity`'s table, `table_oid`, filled in this
    /// transaction, can be trusted. Nothing for a LOGGED table.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be written.
    pub fn mark(entity: &str, table_oid: Oid) -> TViewResult<bool> {
        if !unlogged(table_oid) {
            return Ok(false);
        }
        let sql = format!(
            "INSERT INTO {} VALUES ($1) ON CONFLICT DO NOTHING RETURNING entity",
            table()
        );
        let _owner = crate::owner::AsOwner::of_extension()?;
        // No row when it was there already (`first()` of an empty result is an error).
        let inserted = Spi::connect_mut(|client| {
            Ok::<_, pgrx::spi::Error>(
                !client
                    .update(&sql, None, &[crate::utils::spi::text(entity)])?
                    .is_empty(),
            )
        })
        .map_err(|e| crate::utils::spi::error(&sql, &e))?;
        Ok(inserted)
    }

    /// Forget `entity`: it is dropped, or no longer UNLOGGED.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be written.
    pub fn forget(entity: &str) -> TViewResult<()> {
        let sql = format!("DELETE FROM {} WHERE entity = $1", table());
        let _owner = crate::owner::AsOwner::of_extension()?;
        crate::utils::spi::run(&sql, &[crate::utils::spi::text(entity)])
    }

    /// Make `entity`'s rows trustworthy before a write refreshes them: once per
    /// entity and backend, since a reset ends every backend. See [`fill_if_reset`].
    /// A TVIEW found empty is checked again once the write refreshed it
    /// ([`refill_if_restored_empty`]). Runs as the TVIEW's owner.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or a fill fails.
    pub fn ensure(entity: &str) -> TViewResult<bool> {
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(false);
        };
        let table = meta.tview_oid.to_u32();
        if CHECKED.with_borrow(|checked| checked.contains(&table)) {
            return Ok(false);
        }
        let filled = fill_if_reset(entity)?.is_some();
        // The TVIEWs it reads first: one restored empty would make its rows empty.
        let graph = crate::cache::graph()?;
        for dependency in graph.children.get(entity).into_iter().flatten() {
            fill_if_emptied(dependency)?;
        }
        if !filled && is_empty(&meta)? {
            FOUND_EMPTY.with_borrow_mut(|found| found.insert(entity.to_string()));
        }
        CHECKED.with_borrow_mut(|checked| checked.insert(table));
        Ok(filled)
    }

    /// After a write refreshed `entity`, which [`ensure`] found empty: refill it
    /// if its view computes a row the TVIEW still lacks. The TVIEW was restored
    /// without its rows (a dump that left them out by name keeps an UNLOGGED
    /// TVIEW's validity row, and a LOGGED one has none), and only the write's own
    /// rows are in it. A TVIEW empty because its view computed nothing holds every
    /// row of its view now: it is left as the write made it. Runs as the TVIEW's
    /// owner. Returns whether it refilled.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or the refill fails.
    pub fn refill_if_restored_empty(entity: &str) -> TViewResult<bool> {
        if !FOUND_EMPTY.with_borrow_mut(|found| found.remove(entity)) {
            return Ok(false);
        }
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(false);
        };
        let key = crate::utils::ident::quoted(&meta.identity.column);
        let (tv, view) = relations(&meta)?;
        let missing = probe(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM {view} v \
                                WHERE NOT EXISTS (SELECT 1 FROM {tv} t WHERE t.{key} = v.{key}))"
            ),
            &[],
        )?;
        if !missing {
            return Ok(false);
        }
        refill(entity)?;
        mark(entity, meta.tview_oid)?;
        Ok(true)
    }

    /// A write refreshed `entity` whole ([`ensure`] found it empty): it holds
    /// every row of its view, nothing to refill.
    pub fn refreshed_whole(entity: &str) {
        FOUND_EMPTY.with_borrow_mut(|found| found.remove(entity));
    }

    /// Fill `entity`'s TVIEW, a TVIEW a write's TVIEW reads, when it is empty
    /// while its view computes rows (restored without them), the TVIEWs it reads
    /// first. No write reaches it in this statement, so its view's rows are all
    /// missing ones.
    fn fill_if_emptied(entity: &str) -> TViewResult<()> {
        let graph = crate::cache::graph()?;
        for dependency in graph.children.get(entity).into_iter().flatten() {
            fill_if_emptied(dependency)?;
        }
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(());
        };
        let owner = crate::owner::AsOwner::of_entity(entity)?;
        let emptied = is_empty(&meta)? && {
            let (_, view) = relations(&meta)?;
            probe(&format!("SELECT EXISTS (SELECT 1 FROM {view})"), &[])?
        };
        drop(owner);
        if emptied {
            refill(entity)?;
            mark(entity, meta.tview_oid)?;
        }
        Ok(())
    }

    /// Whether `meta`'s TVIEW has no row: no page at all, else no entry in the
    /// index of its identity (one index probe, never a scan of the table). Read
    /// by its owner.
    fn is_empty(meta: &TviewMeta) -> TViewResult<bool> {
        let args = [crate::utils::spi::oid(meta.tview_oid)];
        if probe("SELECT pg_catalog.pg_relation_size($1) = 0", &args)? {
            return Ok(true);
        }
        let (tv, _) = relations(meta)?;
        let key = crate::utils::ident::quoted(&meta.identity.column);
        probe(
            &format!(
                "SELECT pg_catalog.count(*) = 0 \
                 FROM (SELECT 1 FROM {tv} t ORDER BY t.{key} LIMIT 1) first"
            ),
            &[],
        )
    }

    /// The TVIEW's table and its backing view, schema-qualified.
    fn relations(meta: &TviewMeta) -> TViewResult<(String, String)> {
        Ok((
            crate::utils::qualified_relname_from_oid(meta.tview_oid)?,
            crate::utils::qualified_relname_from_oid(meta.view_oid)?,
        ))
    }

    /// Whether `sql` returns true, read-only: a status call must not assign a
    /// transaction id.
    fn probe(sql: &str, args: &[pgrx::datum::DatumWithOid<'_>]) -> TViewResult<bool> {
        Spi::connect(|client| client.select(sql, Some(1), args)?.first().get_one::<bool>())
            .map(|found| found == Some(true))
            .map_err(|e| crate::utils::spi::error(sql, &e))
    }

    /// Check the tables again when a rolled-back (sub)transaction may have undone
    /// what was found: it filled a TVIEW, or changed the catalog (a TVIEW created
    /// in it is gone); and after a prepared one, which may not commit. A check
    /// that found a TVIEW trusted, and filled nothing, holds after the rollback.
    pub fn forget_checks() {
        if FILLED_IN_XACT.get() || crate::cache::changed_in_xact() {
            forget_all_checks();
        }
    }

    /// Check every table again.
    pub fn forget_all_checks() {
        CHECKED.with_borrow_mut(std::collections::HashSet::clear);
    }

    /// Fill `entity`'s TVIEW from its view, noting it for an abort.
    fn refill(entity: &str) -> TViewResult<u64> {
        FILLED_IN_XACT.set(true);
        crate::refresh::full::refill(entity)
    }

    /// The transaction ended: its claims, the TVIEWs it found empty, and its
    /// fills ended with it.
    pub fn forget_claims() {
        CLAIMS.with_borrow_mut(Vec::clear);
        FOUND_EMPTY.with_borrow_mut(std::collections::HashSet::clear);
        FILLED_IN_XACT.set(false);
    }

    /// A subtransaction rolled back: the claims it made are undone.
    pub fn forget_subtransaction_claims() {
        // SAFETY: reads the backend's transaction state.
        let level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
        CLAIMS.with_borrow_mut(|claims| claims.retain(|&(_, at)| at < level));
    }

    /// Refuse to prepare a transaction that claimed a refill: the claim, a row of
    /// `pg_tview_valid`, would block every writer of that TVIEW until
    /// `COMMIT PREPARED`. Runs in the `PRE_PREPARE` callback: no SPI.
    pub fn refuse_prepare_with_claims() {
        let Some(table) = CLAIMS.with_borrow(|claims| claims.first().map(|&(t, _)| t)) else {
            return;
        };
        crate::TViewError::PrepareRefused {
            reason: crate::error::PrepareRefusal::Refill {
                table: crate::utils::qualified_relname_from_oid(table)
                    .unwrap_or_else(|_| format!("with OID {}", table.to_u32())),
            },
        }
        .raise();
    }

    /// If `entity`'s table is UNLOGGED and was reset, fill the TVIEWs it reads,
    /// then claim and fill it. Returns the rows inserted when this transaction
    /// filled it.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or a fill fails.
    pub fn fill_if_reset(entity: &str) -> TViewResult<Option<u64>> {
        let Some(meta) = TviewMeta::load_by_entity(entity)? else {
            return Ok(None);
        };
        if !needs_fill(entity, meta.tview_oid)? {
            return Ok(None);
        }
        let graph = crate::cache::graph()?;
        for dependency in graph.children.get(entity).into_iter().flatten() {
            fill_if_reset(dependency)?;
        }
        // Another transaction may have claimed it since: the claim then waits
        // for it and finds the row.
        if !mark(entity, meta.tview_oid)? {
            return Ok(None);
        }
        // SAFETY: reads the backend's transaction state.
        let level = unsafe { pg_sys::GetCurrentTransactionNestLevel() };
        CLAIMS.with_borrow_mut(|claims| claims.push((meta.tview_oid, level)));
        refill(entity).map(Some)
    }

    /// Before `entity`'s table, `table_oid`, becomes LOGGED (`logged`) or UNLOGGED:
    /// a reset TVIEW is filled first, since a LOGGED one is never checked again.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or the fill fails.
    pub fn before_persistence_change(entity: &str, logged: bool) -> TViewResult<()> {
        if logged {
            fill_if_reset(entity)?;
        }
        Ok(())
    }

    /// After `entity`'s table, `table_oid`, became LOGGED (`logged`) or UNLOGGED: a
    /// LOGGED table has no row; the rows of one just made UNLOGGED were trusted
    /// until now.
    ///
    /// # Errors
    /// Returns an error if `pg_tview_valid` cannot be written.
    pub fn after_persistence_change(entity: &str, table_oid: Oid, logged: bool) -> TViewResult<()> {
        if logged {
            forget(entity)
        } else {
            mark(entity, table_oid).map(|_| ())
        }
    }
}
