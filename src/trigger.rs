use crate::queue::key::KeyValue;
use crate::queue::{enqueue_refresh, enqueue_refresh_patched};
use pgrx::PgTupleDesc;
use pgrx::prelude::*;
/// Trigger Handler: Change Detection and Queue Management
///
/// This module implements `PostgreSQL` triggers for TVIEW change tracking:
/// - **Row-level Triggers**: Detects INSERT/UPDATE/DELETE on base tables
/// - **Key Extraction**: reads the TVIEW key a changed row maps to
/// - **Queue Enqueueing**: Adds refresh requests to transaction queue
///
/// ## Trigger Lifecycle
///
/// 1. `PostgreSQL` calls trigger for each changed row
/// 2. For each cascade path of the table (one per TVIEW key column it holds,
///    the TVIEW's own rows included), read the key off the old and new row
/// 3. Enqueue `(entity, key)` for refresh
/// 4. The statement's flush (or COMMIT) processes the queue
///
/// ## Performance Considerations
///
/// - Triggers run in critical path - must be fast
/// - Minimal database queries during trigger execution
/// - Queue processing deferred to the end of the statement
use pgrx::spi;

/// A key read off a changed row.
enum KeyExtraction {
    Value(KeyValue),
    /// The column is NULL: the row maps to no TVIEW row.
    Null,
    /// The column is gone.
    Missing,
}

/// The attribute number of `name` in `tupdesc`: `hint` when that attribute still
/// has this name (a dropped column or a partition laid out differently moves it).
fn attnum_of(tupdesc: &PgTupleDesc<'_>, name: &str, hint: Option<i16>) -> Option<usize> {
    let named = |i: usize| {
        tupdesc
            .get(i)
            .is_some_and(|att| !att.attisdropped && att.name() == name)
    };
    hint.and_then(|n| usize::try_from(n).ok())
        .and_then(|n| n.checked_sub(1))
        .filter(|&i| named(i))
        .or_else(|| (0..tupdesc.len()).find(|&i| named(i)))
        .map(|i| i + 1)
}

/// The value of column `name` of `tuple` as a TVIEW key: `Int` for an integer
/// column, the type's output text for any other.
///
/// SAFETY: `tuple` is a valid heap tuple of `tupdesc`.
unsafe fn tuple_key(
    tuple: *mut pg_sys::HeapTupleData,
    tupdesc: &PgTupleDesc<'_>,
    name: &str,
    hint: Option<i16>,
) -> KeyExtraction {
    let Some(attnum) = attnum_of(tupdesc, name, hint) else {
        return KeyExtraction::Missing;
    };
    let Some(att) = tupdesc.get(attnum - 1) else {
        return KeyExtraction::Missing;
    };
    // SAFETY: a catalog lookup by type OID; a domain is read as its base type.
    let typid = unsafe { pg_sys::getBaseType(att.atttypid) };
    // SAFETY: the caller's tuple and its descriptor; `attnum` is in range.
    unsafe {
        let mut isnull = false;
        let datum = pg_sys::heap_getattr(
            tuple,
            i32::try_from(attnum).unwrap_or(i32::MAX),
            tupdesc.as_ptr(),
            &mut isnull,
        );
        if isnull {
            return KeyExtraction::Null;
        }
        let int = match typid {
            pg_sys::INT2OID => i16::from_datum(datum, false).map(i64::from),
            pg_sys::INT4OID => i32::from_datum(datum, false).map(i64::from),
            pg_sys::INT8OID => i64::from_datum(datum, false),
            _ => {
                let mut output = pg_sys::InvalidOid;
                let mut varlena = false;
                pg_sys::getTypeOutputInfo(typid, &raw mut output, &raw mut varlena);
                let text = pg_sys::OidOutputFunctionCall(output, datum);
                let value = std::ffi::CStr::from_ptr(text)
                    .to_string_lossy()
                    .into_owned();
                pg_sys::pfree(text.cast());
                return KeyExtraction::Value(KeyValue::Text(value));
            }
        };
        int.map_or(KeyExtraction::Null, |v| {
            KeyExtraction::Value(KeyValue::Int(v))
        })
    }
}

/// The old and new images of the changed row (an INSERT has only the new one in
/// `tg_trigtuple`, a DELETE only the old one).
fn row_images(trigger: &PgTrigger<'_>) -> Vec<*mut pg_sys::HeapTupleData> {
    let td = trigger.trigger_data();
    [td.tg_trigtuple, td.tg_newtuple]
        .into_iter()
        .filter(|t| !t.is_null())
        .collect()
}

/// The descriptor of the trigger's relation.
fn trigger_tupdesc<'a>(trigger: &'a PgTrigger<'a>) -> Option<PgTupleDesc<'a>> {
    let rel = trigger.trigger_data().tg_relation;
    // SAFETY: inside a row-level trigger the relation and its descriptor are valid
    // for the call.
    unsafe {
        (!rel.is_null() && !(*rel).rd_att.is_null())
            .then(|| PgTupleDesc::from_pg_unchecked((*rel).rd_att))
    }
}

/// Trigger handler function for TVIEW cascades
/// This is called by triggers installed on base tables when rows change
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    crate::revision::check();
    // A lookup that fails fails the write: committing it with nothing queued would
    // leave the TVIEWs over the table stale.
    let table_oid = trigger
        .relation()
        .unwrap_or_else(|e| error!("pg_tviews: the row trigger has no relation: {e}"))
        .oid();
    // The TVIEW this trigger serves (its argument). A table read by several
    // TVIEWs has one trigger each.
    let served = crate::delta::trigger_entity(trigger);
    // A row of a partition: what pg_tviews knows is its partitioned table.
    let table_oid = crate::delta::partition_root(table_oid)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not resolve the partition root"));
    let paths: Vec<crate::catalog::plan::LocalPath> = crate::cache::cascade_paths(table_oid)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plans"))
        .into_iter()
        .filter(|p| p.entity_name == served)
        .collect();
    // If triggers are suspended, record the change instead of enqueuing
    if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
        crate::suspend::record_change(&served);
        return Ok(None);
    }

    // Every TVIEW key this row holds, its TVIEW's own rows included (ADR 0169).
    enqueue_local_keys(trigger, &paths);

    // A partitioned table whose writes map through a query (ADR 0157): the
    //    row trigger is the one copied onto every partition, so map this row.
    if let Err(e) = crate::delta::map_row(trigger, &served, table_oid) {
        e.raise_in(&format!(
            "pg_tviews: could not map the changed row to tv_{served} keys"
        ));
    }

    Ok(None)
}

/// The new image of an UPDATE's row.
fn new_image(trigger: &PgTrigger<'_>) -> Option<*mut pg_sys::HeapTupleData> {
    let new = trigger.trigger_data().tg_newtuple;
    (!new.is_null()).then_some(new)
}

/// Enqueue the TVIEW keys a changed row holds, one per local path of its table:
/// the TVIEW's own rows (a root path) and a table joined on a key column.
///
/// Each path is followed from the old and the new row: an UPDATE that moves the
/// row to another key (a changed join key, a changed DISTINCT ON key) refreshes
/// both.
fn enqueue_local_keys(trigger: &PgTrigger, paths: &[crate::catalog::plan::LocalPath]) {
    if paths.is_empty() {
        return;
    }
    let Some(tupdesc) = trigger_tupdesc(trigger) else {
        error!("pg_tviews: the row trigger has no relation");
    };
    let images = row_images(trigger);
    if images.is_empty() {
        error!("pg_tviews: the row trigger has no row");
    }

    // Column-aware refresh: on a row-level UPDATE, a cascade whose target tview
    // depends on NONE of the changed columns cannot alter any target row, so the
    // refresh is skipped. `changed_columns` returns None for INSERT/DELETE
    // (membership changes) — those always cascade. A path with an empty
    // `source_columns` (multi-hop, unknown, or whole-row/.data reference) also
    // always cascades — the safe default.
    let changed = changed_columns(trigger);

    for path in paths {
        // A root path always refreshes: the TVIEW's own row is recomputed (and the
        // no-op guard skips the write) as before root paths existed.
        if let Some(changed) = &changed
            && !path.root
            && !path.source_columns.is_empty()
            && !path.source_columns.iter().any(|c| changed.contains(c))
        {
            continue;
        }
        // An UPDATE of the TVIEW's own row that only changes columns
        // copied into its data is patched in place.
        if path.root
            && let Some(fields) = try_capture_direct_patch(trigger, path, changed.as_deref())
            && let Some(new) = new_image(trigger)
            // SAFETY: the new image of the trigger's row.
            && let KeyExtraction::Value(KeyValue::Int(pk)) =
                unsafe { tuple_key(new, &tupdesc, &path.initial_col, path.initial_attnum) }
        {
            enqueue_refresh_patched(&path.entity_name, pk, fields);
            continue;
        }
        for &image in &images {
            follow_local_path(path, image, &tupdesc);
        }
    }
}

/// Enqueue the key a local path reads off one image of the changed row (its
/// `initial_col`).
fn follow_local_path(
    path: &crate::catalog::plan::LocalPath,
    image: *mut pg_sys::HeapTupleData,
    tupdesc: &PgTupleDesc<'_>,
) {
    // SAFETY: an image of the trigger's row, of its relation's descriptor.
    match unsafe { tuple_key(image, tupdesc, &path.initial_col, path.initial_attnum) } {
        KeyExtraction::Value(key) => enqueue_refresh(&path.entity_name, key),
        KeyExtraction::Null => {} // FK is NULL, cascade stops
        KeyExtraction::Missing => crate::TViewError::CatalogError {
            operation: format!(
                "Read the key of tv_{} off a row of {}",
                path.entity_name, path.source_table
            ),
            pg_error: format!(
                "the table has no column \"{}\" for tv_{} to read its key from",
                path.initial_col, path.entity_name
            ),
        }
        .raise(),
    }
}

// ── Direct-patch capture ─────────────────────────────────────────────────────

unsafe extern "C" {
    /// `PostgreSQL`'s `datumIsEqual` (`src/backend/utils/adt/datum.c`) — exported
    /// by the backend but absent from the pgrx bindings; resolved at `.so` load
    /// time like every other `pg_sys` symbol. Compares datums for physical equality.
    #[link_name = "datumIsEqual"]
    fn datum_is_equal(
        value1: pg_sys::Datum,
        value2: pg_sys::Datum,
        typ_by_val: bool,
        typ_len: i32,
    ) -> bool;
}

/// Try to capture a direct patch for an eligible row-level UPDATE.
///
/// Returns `Some(fields)` — a `key → value` JSONB map ready to merge into the
/// entity's own `data` — only when every changed column is in the plan's
/// direct-patch map (columns of the table holding the identity that `data`
/// copies and the definition reads nowhere else, ADR 0203) and the key is
/// unchanged; `None` (fall back to recompute) otherwise. Pure in-memory: the
/// cached plan, a raw datum diff, and typed value extraction — no SPI.
fn try_capture_direct_patch(
    trigger: &PgTrigger,
    path: &crate::catalog::plan::LocalPath,
    changed: Option<&[String]>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    if !crate::config::direct_patch_enabled() {
        return None;
    }
    // Full-tuple diff — `None` unless this is a row-level UPDATE (OLD and NEW both
    // present). No changed columns ⇒ nothing to patch (let the caller plain-enqueue).
    let changed = changed.filter(|c| !c.is_empty())?;
    if changed.contains(&path.initial_col) {
        return None;
    }
    let meta = crate::catalog::TviewMeta::load_by_entity(&path.entity_name)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plan"))?;
    let direct = &meta.plan.direct;
    let data_key = |col: &str| direct.iter().find(|(c, _)| c == col).map(|(_, k)| k);
    if direct.is_empty()
        || !changed.iter().all(|c| data_key(c).is_some())
        || !crate::jsonb_delta::check_jsonb_delta_available()
    {
        return None;
    }
    let new_tuple = trigger.new()?;
    let mut fields = serde_json::Map::with_capacity(changed.len());
    for col in changed {
        fields.insert(data_key(col)?.clone(), capture_value(&new_tuple, col)?);
    }
    Some(fields)
}

/// Diff OLD vs NEW over **all** columns of the changed row. Returns the names of
/// columns whose stored value changed, or `None` when this is not a row-level
/// UPDATE (either OLD or NEW absent).
///
/// Type-agnostic: compares raw datums via `datumIsEqual` over the relation's tuple
/// descriptor, so it detects changes to *unmapped* columns (filter columns,
/// computed-field inputs) without knowing their types. A false "changed" (e.g. an
/// equal-but-differently-TOASTed varlena) is safe — it only forces a recompute.
fn changed_columns(trigger: &PgTrigger) -> Option<Vec<String>> {
    // SAFETY: inside a row-level AFTER trigger the `TriggerData` and its relation
    // are valid for the call; `tg_trigtuple`/`tg_newtuple` are the OLD/NEW heap
    // tuples and `rd_att` the matching tuple descriptor. `heap_getattr` reads a
    // single attribute; `datum_is_equal` performs a physical (non-detoasting)
    // comparison — never dereferencing beyond the datum itself.
    unsafe {
        let td: &pg_sys::TriggerData = trigger.trigger_data();
        let old_tuple = td.tg_trigtuple;
        let new_tuple = td.tg_newtuple;
        if old_tuple.is_null() || new_tuple.is_null() {
            return None; // INSERT or DELETE — a membership change, not a patch
        }
        let rel = td.tg_relation;
        if rel.is_null() {
            return None;
        }
        let tupdesc_ptr = (*rel).rd_att;
        if tupdesc_ptr.is_null() {
            return None;
        }

        let tupdesc = PgTupleDesc::from_pg_unchecked(tupdesc_ptr);
        let mut changed = Vec::new();
        for i in 0..tupdesc.len() {
            let Some(att) = tupdesc.get(i) else { continue };
            if att.attisdropped {
                continue;
            }
            let attnum = i32::try_from(i + 1).ok()?;
            let mut old_isnull = false;
            let mut new_isnull = false;
            let old_datum = pg_sys::heap_getattr(old_tuple, attnum, tupdesc_ptr, &mut old_isnull);
            let new_datum = pg_sys::heap_getattr(new_tuple, attnum, tupdesc_ptr, &mut new_isnull);

            let differs = if old_isnull != new_isnull {
                true
            } else if old_isnull {
                false // both NULL
            } else {
                !datum_is_equal(old_datum, new_datum, att.attbyval, i32::from(att.attlen))
            };
            if differs {
                changed.push(att.name().to_string());
            }
        }
        Some(changed)
    }
}

/// Extract NEW's value for `col` as a `serde_json::Value` using the type whitelist.
/// The whitelist matches `PostgreSQL`'s own `to_jsonb` output
/// byte-for-byte; any other type (float, numeric, timestamp, array, …) yields
/// `None`, forcing a recompute. SQL NULL becomes `Value::Null`.
fn capture_value(
    tuple: &PgHeapTuple<'_, AllocatedByPostgres>,
    col: &str,
) -> Option<serde_json::Value> {
    use serde_json::Value;

    // Each arm: Ok(Some) → value, Ok(None) → SQL NULL, Err → wrong type, try next.
    match tuple.get_by_name::<String>(col) {
        Ok(Some(v)) => return Some(Value::String(v)),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    match tuple.get_by_name::<bool>(col) {
        Ok(Some(v)) => return Some(Value::Bool(v)),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    match tuple.get_by_name::<i64>(col) {
        Ok(Some(v)) => return Some(Value::Number(v.into())),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    match tuple.get_by_name::<i32>(col) {
        Ok(Some(v)) => return Some(Value::Number(v.into())),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    match tuple.get_by_name::<i16>(col) {
        Ok(Some(v)) => return Some(Value::Number(v.into())),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    // UUID → canonical lowercase string, matching uuid::text / to_jsonb(uuid).
    match tuple.get_by_name::<pgrx::Uuid>(col) {
        Ok(Some(v)) => return Some(Value::String(v.to_string())),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    // JSONB → passthrough of the parsed value.
    match tuple.get_by_name::<pgrx::JsonB>(col) {
        Ok(Some(v)) => return Some(v.0),
        Ok(None) => return Some(Value::Null),
        Err(_) => {}
    }
    None
}

/// Statement-level AFTER trigger that flushes the refresh queue.
///
/// This fires once per statement (not per row) and processes all queued
/// refresh requests. It ensures auto-commit (implicit) transactions get
/// their TVIEWs refreshed, since the `ProcessUtility` hook only intercepts
/// explicit COMMIT statements.
///
/// For explicit transactions (BEGIN...COMMIT), both this trigger and the
/// `ProcessUtility` hook may run. The flush is idempotent — the second call
/// finds an empty queue and returns immediately.
///
/// A refresh error fails the statement. If triggers are suspended, this trigger
/// skips the flush. Inside an enclosing statement that writes a TVIEW's base
/// table (a trigger writing its own table), it leaves the queue to that
/// statement's flush.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_flush_trigger<'a>(
    _trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    if !crate::executor::flush_deferred() {
        flush_after_statement();
    }
    Ok(None)
}

/// Refresh what the statement queued, unless triggers are suspended. A refresh
/// that fails fails the write, as an error PostgreSQL raises does: committing it
/// would leave the TVIEWs stale.
pub fn flush_after_statement() {
    if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
        return;
    }
    if let Err(e) = crate::flush::flush_refresh_queue() {
        e.raise_in("TVIEW refresh failed");
    }
    if let Err(e) = crate::audit::flush_audit_buffer() {
        e.raise_in("Audit flush failed");
    }
}
