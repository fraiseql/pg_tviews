use crate::queue::cache::CachedEntityInfo;
use crate::queue::key::KeyValue;
use crate::queue::{enqueue_refresh, enqueue_refresh_patched};
use crate::utils::{IntExtraction, tuple_get_i64};
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
    let typid = att.atttypid;
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
    // Extract table OID
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };
    // The TVIEW this trigger serves (its argument; none for a trigger an older
    // release installed, which serves every TVIEW reading the table). A table read
    // by several TVIEWs has one trigger each.
    let served = crate::delta::trigger_entity(trigger);
    let serves = |entity: &str| served.as_deref().is_none_or(|e| e == entity);
    // A row of a partition: what pg_tviews knows is its partitioned table.
    let table_oid = match crate::delta::partition_root(table_oid) {
        Ok(root) => root,
        Err(e) => {
            warning!(
                "Failed to resolve the partition root of {:?}: {}",
                table_oid,
                e
            );
            table_oid
        }
    };

    let paths: Vec<crate::cascade_path::CascadePath> =
        match crate::queue::cache::cascade_cache::cascade_paths_for_table(table_oid) {
            Ok(p) => p.into_iter().filter(|p| serves(&p.entity_name)).collect(),
            Err(e) => {
                warning!(
                    "Failed to load cascade paths for table {:?}: {:?}",
                    table_oid,
                    e
                );
                vec![]
            }
        };
    // The TVIEW over tb_<entity>, when this is its table: direct patches (#56), and
    // a TVIEW registered before its root table had a cascade path of its own.
    let own = match crate::queue::cache::table_cache::entity_info_cached(table_oid) {
        Ok(info) => info.filter(|i| serves(&i.name)),
        Err(e) => {
            warning!(
                "Failed to resolve entity for table OID {:?}: {:?}",
                table_oid,
                e
            );
            None
        }
    };

    // If triggers are suspended, record the change instead of enqueuing
    if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
        if let Some(entity) = &served {
            crate::suspend::record_change(entity);
            return Ok(None);
        }
        if let Some(info) = &own {
            crate::suspend::record_change(&info.name);
        }
        for path in &paths {
            crate::suspend::record_change(&path.entity_name);
        }
        return Ok(None);
    }

    if let Some((info, legacy)) = own.as_ref().and_then(|i| i.legacy_root.map(|l| (i, l))) {
        enqueue_legacy_root(trigger, info, legacy);
    }

    // Every TVIEW key this row holds, its TVIEW's own rows included (ADR 0169).
    enqueue_cascade_parents(trigger, own.as_ref(), &paths);

    // A partitioned table whose writes map through a query (ADR 0157): the
    //    row trigger is the one copied onto every partition, so map this row.
    if let Some(entity) = &served
        && let Err(e) = crate::delta::map_row(trigger, entity, table_oid)
    {
        error!("pg_tviews: could not map the changed row to tv_{entity} keys: {e}");
    }

    Ok(None)
}

/// The key of a TVIEW registered before its root table had a cascade path of its
/// own (ADR 0169): `pk_<entity>` read by name off the old and new rows of
/// `tb_<entity>`, until `pg_tviews_reregister()` re-derives it. A DISTINCT ON one
/// is refreshed in full.
fn enqueue_legacy_root(
    trigger: &PgTrigger,
    info: &CachedEntityInfo,
    legacy: crate::queue::cache::LegacyRoot,
) {
    let entity = &info.name;
    let reregister = format!(
        "tv_{entity} was registered by an older release: SELECT * FROM \
         tviews.pg_tviews_reregister_all() re-registers it"
    );
    if legacy == crate::queue::cache::LegacyRoot::DistinctOn {
        crate::utils::log_once(
            &format!("legacy_distinct_on:{entity}"),
            &format!("{reregister}; until then it is refreshed in full on writes"),
        );
        crate::queue::enqueue_refresh_all(entity);
        return;
    }
    let pk_col = format!("pk_{entity}");
    let Some(tupdesc) = trigger_tupdesc(trigger) else {
        return;
    };
    if let Some(fields) = try_capture_direct_patch(trigger, info, &pk_col)
        && let Some(new) = new_image(trigger)
        // SAFETY: the new image of the trigger's row.
        && let KeyExtraction::Value(KeyValue::Int(pk)) =
            unsafe { tuple_key(new, &tupdesc, &pk_col, None) }
    {
        enqueue_refresh_patched(entity, pk, fields);
        return;
    }
    for image in row_images(trigger) {
        // SAFETY: an image of the trigger's row, of its relation's descriptor.
        match unsafe { tuple_key(image, &tupdesc, &pk_col, None) } {
            KeyExtraction::Value(key) => enqueue_refresh(entity, key),
            KeyExtraction::Null => {}
            KeyExtraction::Missing => warning!("{pk_col} not found on the changed row"),
        }
    }
}

/// The new image of an UPDATE's row.
fn new_image(trigger: &PgTrigger<'_>) -> Option<*mut pg_sys::HeapTupleData> {
    let new = trigger.trigger_data().tg_newtuple;
    (!new.is_null()).then_some(new)
}

/// Enqueue the TVIEW keys a changed row holds, one per cascade path of its
/// table: the TVIEW's own rows (a root path), a table joined on a key column, and
/// paths registered before ADR 0157.
///
/// Each path is followed from the old and the new row: an UPDATE that moves the
/// row to another key (a changed FK, a changed DISTINCT ON key) refreshes both.
fn enqueue_cascade_parents(
    trigger: &PgTrigger,
    own: Option<&CachedEntityInfo>,
    paths: &[crate::cascade_path::CascadePath],
) {
    if paths.is_empty() {
        return;
    }
    let Some(tupdesc) = trigger_tupdesc(trigger) else {
        warning!("No relation in trigger context");
        return;
    };
    let images = row_images(trigger);
    if images.is_empty() {
        warning!("No tuple available in trigger context");
        return;
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
        // Issue #56: an UPDATE of the TVIEW's own row that only changes columns
        // copied into its data is patched in place.
        if path.root
            && let Some(info) = own.filter(|i| i.name == path.entity_name)
            && let Some(fields) = try_capture_direct_patch(trigger, info, &path.initial_col)
            && let Some(new) = new_image(trigger)
            // SAFETY: the new image of the trigger's row.
            && let KeyExtraction::Value(KeyValue::Int(pk)) =
                unsafe { tuple_key(new, &tupdesc, &path.initial_col, path.initial_attnum) }
        {
            enqueue_refresh_patched(&path.entity_name, pk, fields);
            continue;
        }
        // Issue #120: write the change into all target rows at flush time, in one
        // statement, instead of recomputing each.
        if let Some(changed) = &changed
            && let Some((fanout, key, fields)) = try_capture_fanout(trigger, path, changed)
        {
            crate::queue::patch::record_fanout(
                (path.entity_name.clone(), fanout.lookup_col.clone(), key),
                fields,
            );
            continue;
        }
        for &image in &images {
            follow_cascade_path(path, image, &tupdesc);
        }
    }
}

/// Capture a fan-out patch (issue #120) for an UPDATE of a cascade path's source
/// row: the path's key and, for every changed column the target reads, the value
/// to write into its `data` key. `None` (recompute every target row) unless the
/// path has a fan-out patch, the join key is unchanged, and every changed column
/// the target reads is copied unchanged and has a whitelisted type.
fn try_capture_fanout<'p>(
    trigger: &PgTrigger,
    path: &'p crate::cascade_path::CascadePath,
    changed: &[String],
) -> Option<(
    &'p crate::cascade_path::FanoutPatch,
    i64,
    serde_json::Map<String, serde_json::Value>,
)> {
    let fanout = path.fanout.as_ref()?;
    if !crate::config::direct_patch_enabled()
        || !crate::lifecycle::check_jsonb_delta_available()
        || changed.contains(&path.initial_col)
    {
        return None;
    }
    let new_tuple = trigger.new()?;
    let IntExtraction::Value(key) = tuple_get_i64(&new_tuple, &path.initial_col) else {
        return None;
    };
    let mut fields = serde_json::Map::new();
    for col in changed.iter().filter(|c| path.source_columns.contains(c)) {
        let (_, data_key) = fanout.fields.iter().find(|(c, _)| c == col)?;
        fields.insert(data_key.clone(), capture_value(&new_tuple, col)?);
    }
    (!fields.is_empty()).then_some((fanout, key, fields))
}

/// Enqueue the key a cascade path reads off one image of the changed row (its
/// `initial_col`).
///
/// A path with hops comes from metadata registered before ADR 0157, which mapped
/// such tables hop by hop; until `pg_tviews_reregister()` gives the table a
/// mapping query, its writes refresh the whole TVIEW.
fn follow_cascade_path(
    path: &crate::cascade_path::CascadePath,
    image: *mut pg_sys::HeapTupleData,
    tupdesc: &PgTupleDesc<'_>,
) {
    // A path whose table is gone maps nothing; the table is reported as uncascaded
    // when the TVIEW is re-registered.
    if path.unresolvable {
        return;
    }
    if !path.hops.is_empty() {
        crate::utils::log_once(
            &format!("legacy_hops:{}", path.entity_name),
            &format!(
                "tv_{0} was registered by an older release: writes to {1} refresh it in full \
                 until SELECT * FROM tviews.pg_tviews_reregister_all() re-registers it",
                path.entity_name, path.source_table
            ),
        );
        crate::queue::enqueue_refresh_all(&path.entity_name);
        return;
    }

    // SAFETY: an image of the trigger's row, of its relation's descriptor.
    match unsafe { tuple_key(image, tupdesc, &path.initial_col, path.initial_attnum) } {
        KeyExtraction::Value(key) => enqueue_refresh(&path.entity_name, key),
        KeyExtraction::Null => {} // FK is NULL, cascade stops
        KeyExtraction::Missing => {
            crate::utils::log_once(
                &format!("initial_col:{}:{}", path.source_table, path.initial_col),
                &format!(
                    "column '{}' of {} is gone: its writes no longer cascade to tv_{}; \
                     re-register it with pg_tviews_reregister('{}')",
                    path.initial_col, path.source_table, path.entity_name, path.entity_name
                ),
            );
        }
    }
}

// ── Issue #56: direct-patch capture ──────────────────────────────────────────

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

/// Try to capture a direct patch for an eligible row-level UPDATE (issue #56).
///
/// Returns `Some(fields)` — a `key → value` JSONB map ready to merge into the
/// entity's own `data` — only when **every** eligibility condition holds; `None`
/// (fall back to recompute) otherwise. Pure in-memory: cached `EntityInfo`, a raw
/// datum diff, and typed value extraction — no SPI.
fn try_capture_direct_patch(
    trigger: &PgTrigger,
    entity_info: &CachedEntityInfo,
    key_col: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    use std::collections::HashSet;

    // Entity-level gates (cheapest first).
    if !crate::config::direct_patch_enabled()
        || entity_info.direct_map.is_empty()
        || entity_info.distinct_on
        || entity_info.is_union
    {
        return None;
    }
    if !crate::lifecycle::check_jsonb_delta_available() {
        return None;
    }

    // Full-tuple diff — `None` unless this is a row-level UPDATE (OLD and NEW both
    // present). No changed columns ⇒ nothing to patch (let the caller plain-enqueue).
    let changed = changed_columns(trigger)?;
    if changed.is_empty() {
        return None;
    }

    // Eligibility: every changed column must feed the entity's own `data` via the
    // direct map, and none may be a membership/identity/projected column that a
    // data-only patch would leave stale.
    let fk_set: HashSet<&str> = entity_info.fk_columns.iter().map(String::as_str).collect();
    let uuid_fk_set: HashSet<&str> = entity_info
        .uuid_fk_columns
        .iter()
        .map(String::as_str)
        .collect();
    let output_set: HashSet<&str> = entity_info
        .output_columns
        .iter()
        .map(String::as_str)
        .collect();

    for col in &changed {
        if col == key_col
            || fk_set.contains(col.as_str())
            || uuid_fk_set.contains(col.as_str())
            || output_set.contains(col.as_str())
            || !entity_info.direct_map.contains_key(col.as_str())
        {
            return None;
        }
    }

    // Capture NEW's value for each changed column with the type whitelist.
    let new_tuple = trigger.new()?;
    let mut fields = serde_json::Map::with_capacity(changed.len());
    for col in &changed {
        let key = entity_info.direct_map.get(col.as_str())?;
        let value = capture_value(&new_tuple, col)?;
        fields.insert(key.clone(), value);
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

/// Extract NEW's value for `col` as a `serde_json::Value` using the type whitelist
/// (issue #56). The whitelist matches `PostgreSQL`'s own `to_jsonb` output
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
/// If triggers are suspended, this trigger skips the flush.
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_flush_trigger<'a>(
    _trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Skip flush if triggers are suspended
    if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
        return Ok(None);
    }

    if let Err(e) = crate::queue::flush_refresh_queue() {
        warning!("TVIEW refresh failed in statement trigger: {:?}", e);
    }
    if let Err(e) = crate::audit::flush_audit_buffer() {
        warning!("Audit flush failed in statement trigger: {:?}", e);
    }
    Ok(None)
}
