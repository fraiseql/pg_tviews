//! The row-level trigger of base tables: it reads the TVIEW keys a changed row
//! holds (one local path per key column, the TVIEW's own rows included, read off
//! the old and the new row), captures a direct patch when the UPDATE allows one,
//! and enqueues them; and the statement-level trigger that flushes the queue at
//! the end of the statement. A lookup that fails fails the write.

use crate::queue::key::KeyValue;
use crate::queue::{enqueue_refresh, enqueue_refresh_patched};
use pgrx::PgTupleDesc;
use pgrx::prelude::*;
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
/// column, the type's output text under [`crate::owner::RENDER_SETTINGS`] for any other.
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
                // Read as the refresh parses it back: under the pinned settings,
                // not the writer's. Integer keys need none.
                let _pin = crate::owner::RenderPin::new();
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

/// The row trigger of a table: one per table (and the copy `PostgreSQL` makes
/// on each partition), serving every TVIEW that reads the table's rows by key.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let _path = crate::owner::CatalogPath::pin();
    crate::revision::check();
    crate::concurrency::require_read_committed();
    crate::delta::require_level(trigger, true);
    crate::delta::require_table_trigger(trigger);
    // A lookup that fails fails the write: committing it with nothing queued would
    // leave the TVIEWs over the table stale.
    let table_oid = trigger
        .relation()
        .unwrap_or_else(|e| {
            crate::TViewError::TriggerMisused {
                reason: format!("pg_tviews: the row trigger has no relation: {e}"),
            }
            .raise()
        })
        .oid();
    // A row of a partition: what pg_tviews knows is its partitioned table.
    let table_oid = crate::catalog::partition_root(table_oid)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not resolve the partition root"));
    let readers = crate::catalog::table_readers(table_oid)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plans"));
    let all_paths = crate::catalog::cascade_paths(table_oid)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plans"));
    // The changed columns, compared datum by datum, once for every TVIEW: only
    // when a local path or a link check reads them.
    let changed = (!all_paths.is_empty() || !crate::concurrency::links_changed())
        .then(|| changed_columns(trigger, table_oid))
        .flatten();
    for served in readers.row() {
        note_row_write(trigger, served, table_oid, changed.as_ref());
    }
    // If triggers are suspended, record the change instead of enqueuing
    if crate::suspend::is_suspended() {
        for served in readers.row() {
            crate::suspend::record_change(served);
        }
        return Ok(None);
    }

    for served in readers.row() {
        // Every TVIEW key this row holds, its TVIEW's own rows included (ADR 0169).
        let paths: Vec<&crate::lineage::plan::LocalPath> = all_paths
            .iter()
            .filter(|p| p.entity_name == served)
            .collect();
        enqueue_local_keys(trigger, &paths, changed.as_ref());

        // A partitioned table whose writes map through a query (ADR 0157): the
        // row trigger is the one copied onto every partition, so map this row.
        if let Err(e) = crate::delta::map_row(trigger, served, table_oid) {
            e.raise_in(&format!(
                "pg_tviews: could not map the changed row to tv_{served} keys"
            ));
        }
    }

    Ok(None)
}

/// Note whether the changed row changed a link of `served`'s rows (#263): an
/// insert or a delete does; an update does when one of the columns of `table`
/// (the partition tree's root) its rows are linked by is among `changed`.
fn note_row_write(
    trigger: &PgTrigger<'_>,
    served: &str,
    table: pg_sys::Oid,
    changed: Option<&Changed>,
) {
    let meta = crate::catalog::TviewMeta::load_reaching(served, table)
        .ok()
        .flatten();
    // A refresh reads every row of the tables the TVIEW reads.
    if let Some(meta) = meta.as_deref()
        && let Err(e) = crate::catalog::stopgap::refuse_row_security(meta)
    {
        e.raise();
    }
    if crate::concurrency::links_changed() {
        return;
    }
    let event = match trigger.op() {
        Ok(PgTriggerOperation::Insert) => crate::queue::Event::Insert,
        Ok(PgTriggerOperation::Update) => crate::queue::Event::Update,
        _ => crate::queue::Event::Delete,
    };
    // Columns the plan does not list may link its rows: every write changes a
    // link.
    if let Some(meta) = meta.as_deref() {
        let relid = table.to_u32();
        let read = crate::catalog::facts::read_set(meta, relid);
        let env = crate::catalog::facts::env(meta, relid);
        if crate::decide::trusted_read_set(&read, env).is_none() {
            crate::concurrency::note_link_change();
            return;
        }
    }
    let mapping = meta.as_deref().and_then(|m| m.key_mapping(table, None));
    crate::concurrency::note_write(mapping, event, |links| {
        let Some(changed) = changed else {
            return true;
        };
        links.iter().any(|&attnum| {
            // A link column gone (dropped) cannot be told apart: count it.
            changed.attnums.contains(attnum)
                // SAFETY: a catalog lookup by OID and attribute number.
                || unsafe { pg_sys::get_attname(table, attnum, true) }.is_null()
        })
    });
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
fn enqueue_local_keys(
    trigger: &PgTrigger,
    paths: &[&crate::lineage::plan::LocalPath],
    changed: Option<&Changed>,
) {
    if paths.is_empty() {
        return;
    }
    let Some(tupdesc) = trigger_tupdesc(trigger) else {
        crate::TViewError::TriggerMisused {
            reason: "pg_tviews: the row trigger has no relation".to_string(),
        }
        .raise();
    };
    let images = row_images(trigger);
    if images.is_empty() {
        crate::TViewError::TriggerMisused {
            reason: "pg_tviews: the row trigger has no row".to_string(),
        }
        .raise();
    }

    // Column-aware refresh: on a row-level UPDATE, a cascade whose target tview
    // depends on NONE of the changed columns cannot alter any target row, so the
    // refresh is skipped (`crate::decide::column_skip`). `changed` is None for
    // INSERT/DELETE (membership changes): those always cascade, and so does a
    // path whose read set is unknown (read whole, through a system column or a
    // declared function, or by a plan stored without it).
    for path in paths {
        // An UPDATE changing none of the columns the TVIEW reads changes none of
        // its rows, its own included.
        if let Some(changed) = changed
            && column_skip(path, changed)
        {
            continue;
        }
        // An UPDATE of the TVIEW's own row that only changes columns
        // copied into its data is patched in place.
        if path.root
            && let Some(fields) = try_capture_direct_patch(trigger, path, changed)
            && let Some(new) = new_image(trigger)
            && let KeyExtraction::Value(KeyValue::Int(pk)) =
                // SAFETY: the new image of the trigger's row, of its relation's
                // descriptor.
                unsafe { tuple_key(new, &tupdesc, &path.initial_col, path.initial_attnum) }
        {
            enqueue_refresh_patched(&path.entity_name, pk, fields);
            continue;
        }
        for (i, &image) in images.iter().enumerate() {
            // Mutant S09: the old image of an UPDATE keys no row.
            if i == 0 && images.len() == 2 && crate::mutant::on("S09") {
                continue;
            }
            follow_local_path(path, image, &tupdesc);
        }
    }
}

/// Whether an UPDATE of a row of `path`'s table that changed `changed` may skip
/// the path's TVIEW ([`crate::decide::column_skip`]).
fn column_skip(path: &crate::lineage::plan::LocalPath, changed: &Changed) -> bool {
    let Some(meta) = crate::catalog::TviewMeta::load_by_entity(&path.entity_name)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plan"))
    else {
        return false;
    };
    let relid = path.source_oid.to_u32();
    let stored = path
        .read_set
        .as_deref()
        .map(|read| crate::mutant::drop_last("S03", read));
    let read = crate::catalog::facts::stored(stored);
    let env = crate::catalog::facts::env(&meta, relid);
    let skip =
        crate::decide::column_skip(crate::decide::Event::Update, &read, &changed.attnums, env);
    // Test builds: a skip the columns the definition names would have allowed.
    #[cfg(feature = "test-hooks")]
    if !skip
        && !path.source_columns.is_empty()
        && !path
            .source_columns
            .iter()
            .any(|c| changed.names().any(|n| n == c))
    {
        crate::stats::count_refusal(&meta, relid);
    }
    skip
}

/// Enqueue the key a local path reads off one image of the changed row (its
/// `initial_col`).
fn follow_local_path(
    path: &crate::lineage::plan::LocalPath,
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

/// Try to capture a direct patch for an eligible row-level UPDATE.
///
/// Returns `Some(fields)` — a `key → value` JSONB map ready to merge into the
/// entity's own `data` — when [`crate::decide::direct_patch`] allows it: every
/// changed column the TVIEW reads is a column `data` copies (the plan's direct
/// fields, ADR 0203) exactly, and the key is unchanged; `None` (fall back to
/// recompute) otherwise, or when a value cannot be captured. The cached plan,
/// a raw datum diff, typed value extraction and syscache lookups: no SPI.
fn try_capture_direct_patch(
    trigger: &PgTrigger,
    path: &crate::lineage::plan::LocalPath,
    changed: Option<&Changed>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    // `None` unless this is a row-level UPDATE (OLD and NEW both present).
    let changed = changed.filter(|c| !c.attnums.is_empty())?;
    let meta = crate::catalog::TviewMeta::load_by_entity(&path.entity_name)
        .unwrap_or_else(|e| e.raise_in("pg_tviews: could not read the propagation plan"))?;
    let relid = path.source_oid.to_u32();
    let direct = &meta.plan.direct;
    if direct.is_empty() {
        return None;
    }
    let data_key = |col: &str| direct.iter().find(|(c, _)| c == col).map(|(_, k)| k);
    // The changed columns `data` copies, each with whether the copy is exact.
    let copied: Vec<crate::decide::Copied> = changed
        .columns
        .iter()
        .filter(|(name, _)| data_key(name).is_some())
        .map(|&(_, attnum)| crate::decide::Copied {
            attnum,
            exact: crate::catalog::facts::copy_is_exact(relid, attnum),
        })
        .collect();
    let key_changed = changed.names().any(|n| n == path.initial_col);
    let read = crate::catalog::facts::path_read_set(path);
    let env = crate::catalog::facts::patch_env(&meta, relid);
    #[cfg(feature = "test-hooks")]
    if crate::decide::trusted_read_set(&read, env).is_none() {
        crate::stats::count_refusal(&meta, relid);
    }
    let fields = crate::decide::direct_patch(
        crate::decide::Event::Update,
        &read,
        &changed.attnums,
        key_changed,
        &copied,
        env,
    )?;
    let new_tuple = trigger.new()?;
    let mut out = serde_json::Map::new();
    for attnum in fields.iter() {
        let (col, _) = changed.columns.iter().find(|(_, a)| *a == attnum)?;
        out.insert(data_key(col)?.clone(), capture_value(&new_tuple, col)?);
    }
    Some(out)
}

/// The columns an UPDATE changed, by binary comparison of its two images.
pub(crate) struct Changed {
    /// Each by name and attnum in the table the plans name (a partition's
    /// partitioned table).
    columns: Vec<(String, i16)>,
    /// Their attnums.
    attnums: crate::decide::Attnums,
}

impl Changed {
    fn names(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().map(|(name, _)| name.as_str())
    }
}

/// Diff OLD vs NEW over **all** columns of the changed row: the columns whose
/// stored value changed, numbered as in `table` (the partition tree's root), or
/// `None` when this is not a row-level UPDATE (either OLD or NEW absent).
///
/// Type-agnostic: compares raw datums via `datumIsEqual` over the relation's tuple
/// descriptor, so it detects changes to *unmapped* columns (filter columns,
/// computed-field inputs) without knowing their types. A false "changed" (e.g. an
/// equal-but-differently-TOASTed varlena) is safe — it only forces a recompute.
fn changed_columns(trigger: &PgTrigger, table: pg_sys::Oid) -> Option<Changed> {
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
                !crate::utils::datum_is_equal(
                    old_datum,
                    new_datum,
                    att.attbyval,
                    i32::from(att.attlen),
                )
            };
            if differs {
                changed.push((att.name().to_string(), i16::try_from(i + 1).ok()?));
            }
        }
        // A partition numbers its columns its own way.
        if (*rel).rd_id != table {
            for (name, attnum) in &mut changed {
                let c_name = std::ffi::CString::new(name.as_str()).ok()?;
                *attnum = pg_sys::get_attnum(table, c_name.as_ptr());
            }
        }
        let attnums = changed.iter().map(|(_, attnum)| *attnum).collect();
        Some(Changed {
            columns: changed,
            attnums,
        })
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
/// skips the flush. Inside an enclosing statement (a mutation function's
/// statement, a trigger writing its own table) it leaves the queue to that
/// statement's end, or to a read of a TVIEW before it ([`crate::executor`]).
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_flush_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let _path = crate::owner::CatalogPath::pin();
    crate::delta::require_table_trigger(trigger);
    if !crate::executor::flush_deferred() {
        flush_after_statement();
    }
    Ok(None)
}

/// Refresh what the statement queued, unless triggers are suspended. A refresh
/// that fails fails the write, as an error PostgreSQL raises does: committing it
/// would leave the TVIEWs stale.
pub fn flush_after_statement() {
    let _path = crate::owner::CatalogPath::pin();
    if crate::suspend::is_suspended() {
        return;
    }
    if let Err(e) = crate::flush::flush_refresh_queue() {
        e.raise_in("TVIEW refresh failed");
    }
}
