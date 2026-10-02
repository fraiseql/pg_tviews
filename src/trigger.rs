use crate::queue::cache::CachedEntityInfo;
use crate::queue::{enqueue_refresh, enqueue_refresh_dedup, enqueue_refresh_patched};
use crate::utils::{IntExtraction, tuple_get_i64};
use pgrx::PgTupleDesc;
use pgrx::prelude::*;
/// Trigger Handler: Change Detection and Queue Management
///
/// This module implements `PostgreSQL` triggers for TVIEW change tracking:
/// - **Row-level Triggers**: Detects INSERT/UPDATE/DELETE on base tables
/// - **Primary Key Extraction**: Identifies changed rows for selective refresh
/// - **Queue Enqueueing**: Adds refresh requests to transaction queue
/// - **Bulk Operations**: Handles multi-row changes efficiently
///
/// ## Trigger Lifecycle
///
/// 1. `PostgreSQL` calls trigger for each changed row
/// 2. Extract primary key of changed row
/// 3. Map table OID to entity name
/// 4. Enqueue `(entity, pk)` pair for refresh
/// 5. Transaction commit processes the queue
///
/// ## Performance Considerations
///
/// - Triggers run in critical path - must be fast
/// - Bulk enqueueing for multi-row operations
/// - Minimal database queries during trigger execution
/// - Queue processing deferred to commit time
use pgrx::spi;

/// Result of attempting to extract a DISTINCT ON key from a tuple
enum KeyExtraction {
    /// Successfully extracted and converted to String
    Value(String),
    /// Column exists but value is NULL
    Null,
    /// All typed extraction attempts failed (unsupported column type)
    TypeMismatch,
}

/// Extract DISTINCT ON key value from tuple, trying multiple types
/// Returns the extraction result: Value on success, Null if column is NULL, `TypeMismatch` if unsupported type
fn extract_distinct_on_key(
    tuple: &PgHeapTuple<'_, AllocatedByPostgres>,
    key_col: &str,
) -> KeyExtraction {
    // Try String (TEXT, VARCHAR)
    match tuple.get_by_name::<String>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {} // type mismatch, try next
    }
    // Try UUID — pgrx's Display yields the canonical lowercase hyphenated form,
    // matching PostgreSQL's `uuid::text` used by refresh_by_dedup_key's WHERE clause.
    match tuple.get_by_name::<pgrx::Uuid>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val.to_string()),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {}
    }
    // Try i64 (BIGINT)
    match tuple.get_by_name::<i64>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val.to_string()),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {}
    }
    // Try i32 (INTEGER)
    match tuple.get_by_name::<i32>(key_col) {
        Ok(Some(val)) => return KeyExtraction::Value(val.to_string()),
        Ok(None) => return KeyExtraction::Null,
        Err(_) => {}
    }
    KeyExtraction::TypeMismatch
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

    // If triggers are suspended, record the change instead of enqueuing
    if crate::config::suspend_triggers() || crate::suspend::is_suspended() {
        if let Some(entity) = &served {
            crate::suspend::record_change(entity);
            return Ok(None);
        }
        // Record direct entity if any
        if let Ok(Some(entity_info)) =
            crate::queue::cache::table_cache::entity_info_cached(table_oid)
        {
            crate::suspend::record_change(&entity_info.name);
        }

        // Record entities from cascade paths (indirect dependencies)
        let paths: Vec<crate::cascade_path::CascadePath> =
            match crate::queue::cache::cascade_cache::cascade_paths_for_table(table_oid) {
                Ok(p) => p,
                Err(e) => {
                    warning!(
                        "Failed to load cascade paths for suspended trigger: {:?}",
                        e
                    );
                    vec![]
                }
            };
        for path in paths {
            crate::suspend::record_change(&path.entity_name);
        }

        return Ok(None);
    }

    // 1. Direct entity: this table IS a TVIEW source (e.g. tb_user → entity "user")
    match crate::queue::cache::table_cache::entity_info_cached(table_oid) {
        Ok(Some(entity_info)) if serves(&entity_info.name) => {
            let entity = &entity_info.name;
            // Check if this is a DISTINCT ON TVIEW using cached distinct_on_key
            if let Some(key_col) = &entity_info.distinct_on_key {
                // DISTINCT ON TVIEW: enqueue dedup key value instead of base PK
                let tuple = if let Some(t) = trigger.new().or_else(|| trigger.old()) {
                    t
                } else {
                    warning!("No tuple in trigger context for DISTINCT ON TVIEW '{entity}'");
                    return Ok(None);
                };
                match extract_distinct_on_key(&tuple, key_col) {
                    KeyExtraction::Value(key_val) => {
                        enqueue_refresh_dedup(entity, &key_val);
                    }
                    KeyExtraction::Null => {
                        warning!(
                            "DISTINCT ON key '{key_col}' is NULL for entity '{entity}' — skipping refresh"
                        );
                    }
                    KeyExtraction::TypeMismatch => {
                        warning!(
                            "Cannot extract DISTINCT ON key '{key_col}' for '{entity}': \
                             unsupported column type — skipping refresh for this row"
                        );
                    }
                }
            } else {
                // Standard PK-based TVIEW: extract pk_<entity>
                let pk_value = match crate::utils::extract_pk(trigger, entity) {
                    Ok(pk) => pk,
                    Err(e) => {
                        warning!("Failed to extract primary key from trigger: {:?}", e);
                        return Ok(None);
                    }
                };
                // Issue #56: on an eligible row-level UPDATE, capture a direct patch
                // from NEW and enqueue it alongside the key (the counter is bumped
                // once per fresh key inside enqueue_refresh_patched). Anything
                // ineligible (or any capture miss) falls through to the plain
                // enqueue, which poisons the key so it recomputes — the universal,
                // always-correct path.
                if let Some(fields) = try_capture_direct_patch(trigger, &entity_info) {
                    enqueue_refresh_patched(entity, pk_value, fields);
                } else {
                    enqueue_refresh(entity, pk_value);
                }
            }
            // No early return: a direct TVIEW source can simultaneously be a
            // base-table dependency of other TVIEWs (tb_user feeds tv_user directly
            // AND tv_post/tv_comment, which embed the author inline via a JOIN on
            // tb_user). That embed is classified a `scalar` dependency, NOT the
            // nested_object/v_user.data form that commit-time entity propagation
            // (find_parents_batch) follows — so without falling through to
            // enqueue_cascade_parents (which walks the base-table cascade paths),
            // tb_user edits leave tv_post/tv_comment author fields stale.
        }
        Ok(_) => { /* fall through to indirect lookup */ }
        Err(e) => {
            warning!(
                "Failed to resolve entity for table OID {:?}: {:?}",
                table_oid,
                e
            );
            return Ok(None);
        }
    }

    // 2. Indirect: this table is a dependency of one or more TVIEWs
    //    Follow cascade paths to determine which TVIEW rows need refreshing
    enqueue_cascade_parents(trigger, table_oid, &serves);

    // 3. A partitioned table whose writes map through a query (ADR 0157): its
    //    partitions cannot have transition tables, so map this row.
    if let Some(entity) = &served
        && let Err(e) = crate::delta::map_row(trigger, entity, table_oid)
    {
        error!("pg_tviews: could not map the changed row to tv_{entity} keys: {e}");
    }

    Ok(None)
}

/// Follow cascade paths from a base table change to enqueue parent TVIEW refreshes.
///
/// When a base table (e.g. `tb_item`) changes, loads cascade paths from the
/// transaction-scoped cache and follows each path hop-by-hop via SPI to
/// discover which TVIEW entity rows need refreshing.
fn enqueue_cascade_parents(
    trigger: &PgTrigger,
    table_oid: pg_sys::Oid,
    serves: &dyn Fn(&str) -> bool,
) {
    let paths: Vec<crate::cascade_path::CascadePath> =
        match crate::queue::cache::cascade_cache::cascade_paths_for_table(table_oid) {
            Ok(p) => p.into_iter().filter(|p| serves(&p.entity_name)).collect(),
            Err(e) => {
                warning!(
                    "Failed to load cascade paths for table {:?}: {:?}",
                    table_oid,
                    e
                );
                return;
            }
        };

    if paths.is_empty() {
        return;
    }

    // An UPDATE can move the row to another parent (a changed FK): the old parent
    // must lose it and the new one gain it, so follow each path from both images.
    let tuples: Vec<_> = [trigger.old(), trigger.new()]
        .into_iter()
        .flatten()
        .collect();
    if tuples.is_empty() {
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

    for path in &paths {
        if let Some(changed) = &changed
            && !path.source_columns.is_empty()
            && !path.source_columns.iter().any(|c| changed.contains(c))
        {
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
        for tuple in &tuples {
            if let Err(e) = follow_cascade_path(path, tuple) {
                warning!(
                    "Cascade refresh failed for path {} → {}: {:?}",
                    path.source_table,
                    path.entity_name,
                    e
                );
            }
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

/// Enqueue the key a cascade path reads off the changed row (its `initial_col`).
///
/// A path with hops comes from metadata registered before ADR 0157, which mapped
/// such tables hop by hop; until `pg_tviews_reregister()` gives the table a
/// mapping query, its writes refresh the whole TVIEW.
fn follow_cascade_path(
    path: &crate::cascade_path::CascadePath,
    tuple: &PgHeapTuple<AllocatedByPostgres>,
) -> crate::TViewResult<()> {
    // A path whose table is gone maps nothing; the table is reported as uncascaded
    // when the TVIEW is re-registered.
    if path.unresolvable {
        return Ok(());
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
        return Ok(());
    }

    match tuple_get_i64(tuple, &path.initial_col) {
        IntExtraction::Value(pk) => enqueue_refresh(&path.entity_name, pk),
        IntExtraction::Null => {} // FK is NULL, cascade stops
        IntExtraction::Missing => {
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
    Ok(())
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
) -> Option<serde_json::Map<String, serde_json::Value>> {
    use std::collections::HashSet;

    // Entity-level gates (cheapest first).
    if !crate::config::direct_patch_enabled()
        || entity_info.direct_map.is_empty()
        || entity_info.distinct_on_key.is_some()
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
    let pk_col = format!("pk_{}", entity_info.name);
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
        if col == &pk_col
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
