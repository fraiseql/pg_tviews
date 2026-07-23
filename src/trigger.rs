use crate::catalog::entity_for_table;
use crate::queue::cache::CachedEntityInfo;
use crate::queue::{
    enqueue_refresh, enqueue_refresh_bulk, enqueue_refresh_dedup, enqueue_refresh_patched,
};
use crate::utils::{IntExtraction, quote_identifier, tuple_get_i64};
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
    // Extract table OID
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };

    // If triggers are suspended, record the change instead of enqueuing
    if crate::config::suspend_triggers() {
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
        Ok(Some(entity_info)) => {
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
                let pk_value = match crate::utils::extract_pk(trigger) {
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
        Ok(None) => { /* fall through to indirect lookup */ }
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
    enqueue_cascade_parents(trigger, table_oid);

    Ok(None)
}

/// Follow cascade paths from a base table change to enqueue parent TVIEW refreshes.
///
/// When a base table (e.g. `tb_item`) changes, loads cascade paths from the
/// transaction-scoped cache and follows each path hop-by-hop via SPI to
/// discover which TVIEW entity rows need refreshing.
fn enqueue_cascade_parents(trigger: &PgTrigger, table_oid: pg_sys::Oid) {
    let paths: Vec<crate::cascade_path::CascadePath> =
        match crate::queue::cache::cascade_cache::cascade_paths_for_table(table_oid) {
            Ok(p) => p,
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

    let Some(tuple) = trigger.new().or_else(|| trigger.old()) else {
        warning!("No tuple available in trigger context");
        return;
    };

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
        if let Err(e) = follow_cascade_path(path, &tuple) {
            warning!(
                "Cascade refresh failed for path {} → {}: {:?}",
                path.source_table,
                path.entity_name,
                e
            );
        }
    }
}

/// Follow a single cascade path to enqueue refresh(es) for the target entity.
fn follow_cascade_path(
    path: &crate::cascade_path::CascadePath,
    tuple: &PgHeapTuple<AllocatedByPostgres>,
) -> crate::TViewResult<()> {
    if path.unresolvable {
        // Full refresh fallback — enqueue with pk=0 sentinel
        // (the flush engine will treat this as "refresh all rows")
        warning!(
            "Unresolvable cascade path for entity '{}' — full refresh needed",
            path.entity_name
        );
        return Ok(());
    }

    // Step 1: Read initial_col from the changed tuple
    let mut current_ids = match tuple_get_i64(tuple, &path.initial_col) {
        IntExtraction::Value(pk) => vec![pk],
        IntExtraction::Null => return Ok(()), // FK is NULL, cascade stops
        IntExtraction::Missing => {
            warning!(
                "Initial column '{}' not found on tuple for cascade to '{}'",
                path.initial_col,
                path.entity_name
            );
            return Ok(());
        }
    };

    // Step 2: Follow each intermediate hop via SPI
    for hop in &path.hops {
        if current_ids.is_empty() {
            return Ok(());
        }
        current_ids = crate::queue::spi_batch_lookup(
            hop.table_oid,
            &hop.lookup_col,
            &hop.carry_col,
            &current_ids,
        )?;
    }

    // Step 3: Enqueue refresh for each resulting PK
    for pk in current_ids {
        enqueue_refresh(&path.entity_name, pk);
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
    if crate::config::suspend_triggers() {
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

/// Statement-level trigger handler for bulk operations
/// This is called once per statement instead of once per row
#[pg_trigger]
#[allow(clippy::unnecessary_wraps)] // Reason: pgrx #[pg_trigger] requires Result return type
fn pg_tview_stmt_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract table OID
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => {
            warning!("Failed to get trigger relation: {:?}", e);
            return Ok(None);
        }
    };

    // Map table OID → entity name
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => {
            // Table not in pg_tview_meta, skip
            return Ok(None);
        }
        Err(e) => {
            warning!(
                "Failed to resolve entity for table OID {:?}: {:?}",
                table_oid,
                e
            );
            return Ok(None);
        }
    };

    // Extract all changed PKs from transition table
    let changed_pks = match extract_pks_from_transition_table(trigger) {
        Ok(pks) => pks,
        Err(e) => {
            warning!("Failed to extract PKs from transition table: {:?}", e);
            return Ok(None);
        }
    };

    if changed_pks.is_empty() {
        // No rows changed, nothing to do
        return Ok(None);
    }

    // Bulk enqueue all changed PKs
    enqueue_refresh_bulk(&entity, changed_pks);

    Ok(None)
}

/// Extract primary keys from `PostgreSQL` transition tables
/// Transition tables are special references visible only in trigger context
fn extract_pks_from_transition_table(trigger: &PgTrigger) -> spi::Result<Vec<i64>> {
    // Determine which transition table to use based on operation type
    // Check which transition table is available (INSERT has NEW, DELETE has OLD, UPDATE has both)
    let transition_table_name = if trigger.new().is_some() && trigger.old().is_none() {
        "new_table" // INSERT
    } else if trigger.new().is_none() && trigger.old().is_some() {
        "old_table" // DELETE
    } else if trigger.new().is_some() && trigger.old().is_some() {
        "new_table" // UPDATE (use NEW for consistency)
    } else {
        return Ok(Vec::new()); // Unsupported event
    };

    // Get PK column name (convention: pk_<entity>)
    let pk_column = get_pk_column_name(
        trigger
            .relation()
            .map_err(|_| crate::TViewError::SpiError {
                query: "get relation".to_string(),
                error: "Failed to get trigger relation".to_string(),
            })?
            .oid(),
    )?;

    // Query transition table for all PKs
    // IMPORTANT: Transition table references don't need quote_ident()
    // They are special PostgreSQL identifiers visible only in trigger context
    let query = format!(
        "SELECT DISTINCT {} FROM {}",
        quote_identifier(&pk_column),
        transition_table_name // No quoting - it's a special reference
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[&pk_column as &str].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

/// Get primary key column name for a table
/// Uses convention: `pk_<entity>` where entity is derived from table name `tb_<entity>`
fn get_pk_column_name(table_oid: pg_sys::Oid) -> spi::Result<String> {
    // Get entity name from table OID
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => {
            return Err(crate::TViewError::SpiError {
                query: "entity_for_table".to_string(),
                error: "Table not managed by pg_tviews".to_string(),
            }
            .into());
        }
        Err(e) => {
            return Err(crate::TViewError::SpiError {
                query: "entity_for_table".to_string(),
                error: format!("Failed to get entity: {e:?}"),
            }
            .into());
        }
    };

    // Convention: pk_<entity>
    Ok(format!("pk_{entity}"))
}
