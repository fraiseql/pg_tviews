//! The statement's transition tables: read pairwise, and bound as named
//! relations for the queries that read them.

use super::render::key_of;
use crate::catalog::KeyType;
use crate::error::{TViewError, TViewResult};
use crate::lineage::DELTA;
use crate::queue::key::KeyValue;
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// Whether every row an UPDATE statement changed holds the same values in the
/// columns `attnums` before and after: the transition tables are read pairwise,
/// each row's old image next to its new one, and compared datum by datum.
/// False when they can't be paired (different row counts) or `attnums` is empty
/// (the columns read are unknown). Virtual generated columns are NULL in
/// transition tables: the caller checks the table has none.
///
/// SAFETY: `td` is the `TriggerData` of an AFTER statement trigger, whose
/// transition tables and relation are valid for the call.
pub(super) unsafe fn reads_unchanged(td: &pg_sys::TriggerData, attnums: &[i16]) -> bool {
    if attnums.is_empty() || td.tg_oldtable.is_null() || td.tg_newtable.is_null() {
        return false;
    }
    // SAFETY: the trigger's relation and its tuple descriptor.
    let natts = unsafe { (*(*td.tg_relation).rd_att).natts };
    if attnums.iter().any(|&a| a < 1 || i32::from(a) > natts) {
        return false;
    }
    // SAFETY: per the contract; each tuplestore is read through a read pointer of
    // its own (as a scan of it in a query would be), and the slots are dropped.
    unsafe {
        let tupdesc = (*td.tg_relation).rd_att;
        let desc = pgrx::PgTupleDesc::from_pg_unchecked(tupdesc);
        let open = |store: *mut pg_sys::Tuplestorestate| {
            let pointer = pg_sys::tuplestore_alloc_read_pointer(
                store,
                pg_sys::EXEC_FLAG_REWIND.cast_signed(),
            );
            pg_sys::tuplestore_select_read_pointer(store, pointer);
            pg_sys::tuplestore_rescan(store);
            pg_sys::MakeSingleTupleTableSlot(tupdesc, &raw const pg_sys::TTSOpsMinimalTuple)
        };
        let (old, new) = (td.tg_oldtable, td.tg_newtable);
        let (old_slot, new_slot) = (open(old), open(new));
        let unchanged = loop {
            let got_old = pg_sys::tuplestore_gettupleslot(old, true, false, old_slot);
            let got_new = pg_sys::tuplestore_gettupleslot(new, true, false, new_slot);
            match (got_old, got_new) {
                (false, false) => break true,
                (true, true) => {}
                _ => break false,
            }
            let same = attnums.iter().all(|&attnum| {
                let mut old_null = false;
                let mut new_null = false;
                let old_value = pg_sys::slot_getattr(old_slot, attnum.into(), &raw mut old_null);
                let new_value = pg_sys::slot_getattr(new_slot, attnum.into(), &raw mut new_null);
                let Some(att) = desc.get(usize::try_from(attnum - 1).unwrap_or(usize::MAX)) else {
                    return false;
                };
                old_null == new_null
                    && (old_null
                        || crate::utils::datum_is_equal(
                            old_value,
                            new_value,
                            att.attbyval,
                            i32::from(att.attlen),
                        ))
            });
            if !same {
                break false;
            }
        };
        pg_sys::ExecDropSingleTupleTableSlot(old_slot);
        pg_sys::ExecDropSingleTupleTableSlot(new_slot);
        // Back to the stores' first read pointer, as they were.
        pg_sys::tuplestore_select_read_pointer(old, 0);
        pg_sys::tuplestore_select_read_pointer(new, 0);
        unchanged
    }
}

/// Run `sql`, which reads the statement's transition tables, as `entity`'s owner
/// and return the key in each row's first column.
pub(super) fn run_with_transition_tables(
    trigger: &PgTrigger<'_>,
    entity: &str,
    key_type: &KeyType,
    sql: &str,
) -> TViewResult<Vec<KeyValue>> {
    with_transition_tables(trigger, entity, |client, table| {
        let mut keys = Vec::new();
        for row in crate::utils::spi::kept_update_over(client, table, sql, &[])? {
            keys.extend(key_of(&row, key_type)?);
        }
        crosscheck_keys(sql, &[], &keys)?;
        Ok(keys)
    })
}

/// REPEATABLE READ: the keys `sql` finds under the latest snapshot are among
/// `keys`, which it found under the transaction's. Inside the connection that ran
/// it (it may read transition tables).
pub(super) fn crosscheck_keys(
    sql: &str,
    args: &[DatumWithOid<'_>],
    keys: &[KeyValue],
) -> TViewResult<()> {
    if !crate::concurrency::crosscheck::enabled() {
        return Ok(());
    }
    let latest = crate::utils::spi::latest_rows_connected(sql, args, None)?;
    let found: Vec<crate::concurrency::crosscheck::TextRow> =
        keys.iter().map(|k| vec![Some(k.to_string())]).collect();
    crate::concurrency::crosscheck::discovered(&found, &latest);
    Ok(())
}

/// `SELECT DISTINCT <value>` of the changed rows `delta`, `value` a lock value
/// over their columns, NULLs left out.
pub(super) fn changed_values(delta: &str, value: &str) -> String {
    format!(
        "WITH {DELTA} AS ({delta}) \
         SELECT DISTINCT v FROM (SELECT {value} AS v FROM {DELTA}) s WHERE v IS NOT NULL"
    )
}

/// The text in the first column of each row of `sql`, a query over the
/// transition tables of table `over` when given.
pub(super) fn texts(
    client: &mut spi::SpiClient<'_>,
    over: Option<Oid>,
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> TViewResult<Vec<String>> {
    let rows = match over {
        Some(table) => crate::utils::spi::kept_update_over(client, table, sql, args)?,
        None => crate::utils::spi::kept_update(client, sql, args)?,
    };
    let mut out = Vec::new();
    for row in rows {
        out.extend(row.get::<String>(1)?);
    }
    Ok(out)
}

/// Run `f` on an SPI connection that sees the statement's transition tables, as
/// `entity`'s owner, with the trigger's table (what the tables are of). Read-write: each query takes a fresh snapshot (READ
/// COMMITTED), so one run after a lock wait sees what it waited for.
pub(super) fn with_transition_tables<T>(
    trigger: &PgTrigger<'_>,
    entity: &str,
    f: impl FnOnce(&mut spi::SpiClient<'_>, Oid) -> crate::TViewResult<T>,
) -> TViewResult<T> {
    let table = trigger
        .relation()
        .map_err(|e| crate::utils::spi::error("the trigger's table", &e))?
        .oid();
    let _owner = crate::owner::AsOwner::of_entity(entity)?;
    Spi::connect_mut(|client| {
        // SAFETY: inside an AFTER statement trigger the TriggerData is valid; this
        // registers its transition tables with the SPI connection just opened.
        let registered = unsafe {
            pg_sys::SPI_register_trigger_data(std::ptr::from_ref(trigger.trigger_data()).cast_mut())
        };
        if registered != pg_sys::SPI_OK_TD_REGISTER.cast_signed() {
            return Err(TViewError::SpiError {
                query: "SPI_register_trigger_data".to_string(),
                error: format!("returned {registered}"),
            });
        }
        f(client, table)
    })
    .map_err(|e| crate::utils::spi::error("a query over the transition tables", &e))
}
