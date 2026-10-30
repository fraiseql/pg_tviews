//! A statement's changed rows mapped to TVIEW keys through each table's mapping
//! query, or written into every row they reach as a fan-out patch.

use super::Event;
use super::render::{computed_columns, delta_sql, key_sql_type, row_pairing};
use super::transition::{
    changed_values, reads_unchanged, run_with_transition_tables, texts, with_transition_tables,
};
use crate::catalog::{KeyType, TviewMeta};
use crate::concurrency::{LockTarget, LockValue};
use crate::config::UncascadedPolicy;
use crate::dependency::triggers::{NEW_TABLE, OLD_TABLE};
use crate::error::{TViewError, TViewResult};
use crate::lineage::{DELTA, KeyMapping, MappingKind};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// The query that maps changed rows of `base_table`, read from a relation named
/// `pg_tviews_delta`, to keys of `meta`'s TVIEW (ADR 0157), with the current names
/// of what it reads. `None` when writes to the table do not map through a query
/// of their own (`propagated`, `all_keys`) or the TVIEW does not read it.
///
/// # Errors
/// Returns an error if the query cannot be rendered.
pub(crate) fn mapping_query(
    meta: &TviewMeta,
    base_table: pg_sys::Oid,
) -> TViewResult<Option<String>> {
    let Some(mapping) = meta.key_mapping(base_table, None) else {
        return Ok(None);
    };
    match mapping.kind {
        MappingKind::Local => Ok(mapping.column.as_deref().map(|column| {
            format!(
                "SELECT DISTINCT {} FROM {DELTA}",
                crate::utils::ident::quoted(column)
            )
        })),
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            rendered(&meta.entity_name, mapping)
        }
        _ => Ok(None),
    }
}

/// What a refresh of `meta`'s rows reads of `base_table`, for value locks (ADR
/// 0207): for each column of the table its mapping joins on (`None` when the table
/// is locked as a whole), the query from the TVIEW's keys (`$1`) to the values a
/// refresh locks. Empty when writes to the table map through no query of their own.
///
/// # Errors
/// Returns an error if a catalog read or a rendering fails.
pub(crate) fn read_set_queries(
    meta: &TviewMeta,
    base_table: pg_sys::Oid,
) -> TViewResult<Vec<(Option<String>, Option<String>)>> {
    let Some(mapping) = meta.key_mapping(base_table, None) else {
        return Ok(Vec::new());
    };
    let key_type = meta.key_type.clone();
    let mut rows = Vec::new();
    for set in &mapping.reads {
        let column = (set.attnum != 0)
            .then(|| {
                Spi::get_one_with_args::<String>(
                    "SELECT attname::pg_catalog.text FROM pg_catalog.pg_attribute \
                     WHERE attrelid = $1 AND attnum = $2",
                    &[
                        crate::utils::spi::oid(base_table),
                        crate::utils::spi::int2(set.attnum),
                    ],
                )
            })
            .transpose()
            .map_err(|e| crate::utils::spi::error("the column's name", &e))?
            .flatten();
        let keys = format!("ANY ({})", crate::refresh::key_cast(&key_type, "$1", true));
        let query = crate::concurrency::reads::rendered(&meta.entity_name, mapping, set, &keys)?;
        rows.push((column, query));
    }
    Ok(rows)
}

/// Map the rows a statement changed in `table_oid` to `entity`'s keys and enqueue
/// them.
pub(super) fn map_statement(
    trigger: &PgTrigger<'_>,
    entity: &str,
    table_oid: Oid,
    event: Event,
) -> TViewResult<()> {
    let meta =
        TviewMeta::load_reaching(entity, table_oid)?.ok_or_else(|| TViewError::TviewNotFound {
            name: entity.to_string(),
        })?;
    let Some(mapping) = meta.key_mapping(table_oid, None) else {
        refresh_all(entity, "its mapping of a written table is unknown");
        return Ok(());
    };
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    let computed = computed_columns(entity, table_oid)?;
    // An UPDATE that changed none of the columns the TVIEW reads maps to no key:
    // decided from the transition tables themselves, before any query.
    let reads_same = event == Event::Update
        && matches!(mapping.kind, MappingKind::Mapped | MappingKind::AllKeys)
        && computed.is_none()
        // SAFETY: an AFTER statement trigger's TriggerData.
        && unsafe { reads_unchanged(trigger.trigger_data(), &mapping.attnums) };
    // The links are columns it reads: unchanged when those are.
    crate::concurrency::note_write(Some(mapping), event, |links| {
        // SAFETY: an AFTER statement trigger's TriggerData.
        computed.is_some()
            || (!reads_same && !unsafe { reads_unchanged(trigger.trigger_data(), links) })
    });
    if reads_same {
        return Ok(());
    }
    match mapping.kind {
        MappingKind::AllKeys if full_refresh => {
            let table = if event == Event::Delete {
                OLD_TABLE
            } else {
                NEW_TABLE
            };
            let changed = run_with_transition_tables(
                trigger,
                entity,
                &KeyType::Int,
                &format!("SELECT 1::pg_catalog.int8 FROM {table} LIMIT 1"),
            )?;
            if !changed.is_empty() {
                crate::queue::enqueue_refresh_all(entity);
            }
        }
        // An `all_keys` table outside `full_refresh` still maps the reads of it
        // that can be traced.
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            let delta = delta_sql(entity, table_oid, event, &mapping.attnums)?;
            // Refreshes of the TVIEW rows these rows feed meet this write on the
            // values (ADR 0207): locked before any query finds rows by them.
            crate::concurrency::lock_changed_rows(mapping, |column| {
                with_transition_tables(trigger, entity, |client, table| {
                    texts(client, Some(table), &changed_values(&delta, column), &[])
                })
            })?;
            if mapping.kind == MappingKind::Mapped
                && event == Event::Update
                && fan_out(trigger, entity, table_oid, mapping)?
            {
                return Ok(());
            }
            let Some(keys_sql) = rendered(entity, mapping)? else {
                refresh_all(entity, "a relation its mapping reads is gone");
                return Ok(());
            };
            let key_type = &meta.key_type;
            let keys = run_with_transition_tables(
                trigger,
                entity,
                key_type,
                &format!(
                    "WITH {DELTA} AS ({delta}) \
                     SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
                    key_sql_type(key_type)
                ),
            )?;
            if !keys.is_empty() {
                crate::queue::enqueue_refresh_bulk(entity, keys);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Write an UPDATE into every TVIEW row it reaches instead of recomputing them,
/// when the mapping has a fan-out patch and every updated row
/// qualifies: same key, and only columns the patch copies into `data` changed.
/// Returns false (map the rows instead) otherwise.
fn fan_out(
    trigger: &PgTrigger<'_>,
    entity: &str,
    table_oid: Oid,
    mapping: &KeyMapping,
) -> TViewResult<bool> {
    let (Some(fanout), Some(key_col)) = (&mapping.fanout, &mapping.key_col) else {
        return Ok(false);
    };
    if !crate::config::direct_patch_enabled() || !crate::jsonb_delta::check_jsonb_delta_available()
    {
        return Ok(false);
    }
    let Some((same_row, columns)) = row_pairing(table_oid, &mapping.attnums)? else {
        return Ok(false);
    };
    // Both images of each updated row; an image without its pair moved its key.
    let sql = format!(
        "SELECT pg_catalog.to_jsonb(o), pg_catalog.to_jsonb(n), \
                (SELECT pg_catalog.count(*) FROM {OLD_TABLE}) = (SELECT pg_catalog.count(*) FROM {NEW_TABLE}) \
         FROM {OLD_TABLE} o JOIN {NEW_TABLE} n ON {same_row}"
    );
    let pairs = with_transition_tables(trigger, entity, |client, table| {
        let mut pairs = Vec::new();
        for row in crate::utils::spi::kept_update_over(client, table, &sql, &[])? {
            if row.get::<bool>(3)? != Some(true) {
                return Ok(None);
            }
            if let (Some(old), Some(new)) = (row.get::<pgrx::JsonB>(1)?, row.get::<pgrx::JsonB>(2)?)
            {
                pairs.push((old.0, new.0));
            }
        }
        Ok(Some(pairs))
    })?;
    let Some(pairs) = pairs else { return Ok(false) };
    let mut patches = Vec::new();
    for (old, new) in &pairs {
        let changed: Vec<&String> = columns
            .iter()
            .filter(|c| old.get(*c) != new.get(*c))
            .collect();
        if changed.is_empty() {
            continue;
        }
        let Some(key) = new.get(key_col).and_then(serde_json::Value::as_i64) else {
            return Ok(false);
        };
        if old.get(key_col) != new.get(key_col) {
            return Ok(false);
        }
        // Each changed column at every path it is copied to.
        let mut fields = serde_json::Map::new();
        for column in changed {
            let mut paths = fanout.fields.iter().filter(|(c, _)| c == column).peekable();
            if paths.peek().is_none() {
                return Ok(false);
            }
            for (_, path) in paths {
                fields.insert(
                    crate::queue::patch::fanout_path_key(path),
                    new.get(column).cloned().unwrap_or_default(),
                );
            }
        }
        patches.push((key, fields));
    }
    // Two joins away: the rows are those whose lookup column holds a value the
    // table in between pairs with the changed key (#228).
    let patches = if fanout.via.is_some() {
        let Some(reached) = reached_through(entity, mapping, &patches)? else {
            return Ok(false);
        };
        reached
    } else {
        patches
    };
    for (key, fields) in patches {
        crate::queue::patch::record_fanout(
            (entity.to_string(), fanout.lookup_col.clone(), key),
            fields,
        );
    }
    Ok(true)
}

/// A fan-out key and the fields it writes, by path.
type FanoutFields = (i64, serde_json::Map<String, serde_json::Value>);

/// The fan-out `patches` of keys of a table two joins away, by the lookup value
/// each key reaches through the table in between (`fanout.via`): a key reaching
/// none writes nothing. The lookup reads as the TVIEW's owner, as every refresh
/// read does, so the writer needs no privilege on the table in between and its
/// policies hide none of the rows. Under REPEATABLE READ the lookup is checked
/// against the latest snapshot, as the patch's own lookup is. `None` when a
/// relation the lookup reads is gone, a value it reaches is no integer, or the
/// rows in between can't be locked one by one: the rows are then mapped and
/// recomputed.
///
/// # Errors
/// Returns an error if the lookup fails; a missed row raises 40001.
fn reached_through(
    entity: &str,
    mapping: &KeyMapping,
    patches: &[FanoutFields],
) -> TViewResult<Option<Vec<FanoutFields>>> {
    if patches.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let Some(sql) = rendered_via(entity, mapping)? else {
        return Ok(None);
    };
    let template = mapping.fanout.as_ref().and_then(|f| f.via.as_deref());
    let Some((target, value)) = via_lock(entity, template.unwrap_or_default())? else {
        return Ok(None);
    };
    let sql = format!("SELECT s.k, s.v, {} FROM ({sql}) s(k, v)", value.of("s.v"));
    let keys: Vec<i64> = patches.iter().map(|(key, _)| *key).collect();
    let args = [crate::utils::spi::int8_array(keys)];
    let plan = crate::utils::spi::Plan::PerExecution;
    let _owner = crate::owner::AsOwner::of_entity(entity)?;
    let Some(found) = locked_lookup(&sql, &args, plan, &target)? else {
        return Ok(None);
    };
    crate::concurrency::crosscheck::discovered_by(&sql, &args, plan, &found)?;
    let fields: std::collections::HashMap<i64, &serde_json::Map<String, serde_json::Value>> =
        patches.iter().map(|(key, fields)| (*key, fields)).collect();
    let mut reached = Vec::with_capacity(found.len());
    for row in &found {
        let parse = |i: usize| row.get(i)?.as_deref()?.parse::<i64>().ok();
        let (Some(key), Some(value)) = (parse(0), parse(1)) else {
            return Ok(None);
        };
        if let Some(fields) = fields.get(&key) {
            reached.push((value, (*fields).clone()));
        }
    }
    Ok(Some(reached))
}

/// The rows of a two-hop lookup (`sql`: key, lookup value, the value's lock
/// form), each row in between it reached share-locked by its lookup value
/// (ADR 0207). The rows in between hold the link the patch follows, and the
/// patched rows don't: a concurrent re-point of one locks that value
/// exclusively, so it waits for this write, or this lookup waits for it. No row
/// of the table is locked, so the owner needs no more than SELECT on it.
///
/// Under READ COMMITTED the lookup runs again after its locks, in a new
/// snapshot, until it reaches no row it hasn't locked: it then drops a row a
/// re-point moved away, whether it waited for the re-point or the re-point
/// committed between the lookup and the lock. Under REPEATABLE READ a lock
/// held elsewhere raises 40001, and the snapshot can't change.
///
/// `None` past `pg_tviews.lock_escalation_threshold` rows: a lock on the whole
/// table would wait for every writer of it, among them one moving a row to
/// the changed key, which waits for this write's own lock on that key. The
/// rows are then mapped and recomputed, which needs no lock in between: a
/// recompute reads the link itself.
///
/// # Errors
/// Returns an error if the lookup fails.
fn locked_lookup(
    sql: &str,
    args: &[pgrx::datum::DatumWithOid<'_>],
    plan: crate::utils::spi::Plan,
    target: &LockTarget,
) -> TViewResult<Option<Vec<crate::utils::spi::TextRow>>> {
    let mut locked = std::collections::HashSet::new();
    loop {
        let found = crate::utils::spi::kept_rows(sql, args, plan)?;
        let new: Vec<String> = found
            .iter()
            .filter_map(|row| row.get(2).cloned().flatten())
            .filter(|value| !locked.contains(value))
            .collect();
        if new.is_empty() {
            return Ok(Some(found));
        }
        if !crate::concurrency::lock_values_unescalated(
            target,
            crate::concurrency::Side::Refresh,
            &new,
        ) {
            return Ok(None);
        }
        if crate::concurrency::crosscheck::enabled() {
            return Ok(Some(found));
        }
        locked.extend(new);
    }
}

/// Where a two-hop lookup locks the rows in between it reaches: their lookup
/// value (the second column the `via` template reads, see
/// `QueryGraph::hop_via`), when it is a read set of the table in between, so
/// that every write of such a row locks it exclusively (old and new images).
/// `None` otherwise, or when the column is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
fn via_lock(entity: &str, template: &str) -> TViewResult<Option<(LockTarget, LockValue)>> {
    let Some(crate::lineage::Placeholder::Column(relid, attnum)) =
        crate::lineage::template_placeholders(template)
            .get(1)
            .copied()
    else {
        return Ok(None);
    };
    let Some(meta) = TviewMeta::load_by_entity(entity)? else {
        return Ok(None);
    };
    let read = meta
        .key_mapping(Oid::from(relid), None)
        .is_some_and(|m| m.reads.iter().any(|set| set.attnum == attnum));
    if !read {
        return Ok(None);
    }
    let Some(value) = crate::concurrency::lock_value(relid, attnum)? else {
        return Ok(None);
    };
    let target = LockTarget {
        relid,
        attnums: vec![attnum],
    };
    Ok(Some((target, value)))
}

/// Refresh `entity` in full because its mapping cannot run, and say once why.
pub(super) fn refresh_all(entity: &str, why: &str) {
    crate::utils::log_once(
        &format!("unmapped:{entity}"),
        &format!(
            "tv_{entity} is refreshed in full on writes because {why}; \
             SELECT tviews.pg_tviews_reregister('{entity}') re-derives it"
        ),
    );
    crate::queue::enqueue_refresh_all(entity);
}

/// The mapping query of `mapping`, with the current names of what it reads
/// (cached until one of them changes).
pub(super) fn rendered(entity: &str, mapping: &KeyMapping) -> TViewResult<Option<String>> {
    let key = (entity.to_string(), mapping.relid);
    if let Some(sql) = crate::cache::MAPPINGS.with(|m| m.get(&key)) {
        return Ok(sql);
    }
    let sql = render_watched(mapping.sql.as_deref().unwrap_or_default())?;
    crate::cache::MAPPINGS.with(|m| m.insert(key, sql.clone()));
    Ok(sql)
}

/// The lookup through the table in between of `mapping`'s fan-out
/// ([`crate::catalog::plan::FanoutPatch::via`]), with the current names of what
/// it reads (cached until one of them changes).
fn rendered_via(entity: &str, mapping: &KeyMapping) -> TViewResult<Option<String>> {
    let key = (entity.to_string(), mapping.relid);
    if let Some(sql) = crate::cache::FANOUT_VIAS.with(|m| m.get(&key)) {
        return Ok(sql);
    }
    let template = mapping.fanout.as_ref().and_then(|f| f.via.as_deref());
    let sql = render_watched(template.unwrap_or_default())?;
    crate::cache::FANOUT_VIAS.with(|m| m.insert(key, sql.clone()));
    Ok(sql)
}

/// `template` with the current names of what it reads; a rename of any of them
/// invalidates the caches.
fn render_watched(template: &str) -> TViewResult<Option<String>> {
    let sql = crate::lineage::render_template(template)
        .map_err(|e| crate::utils::spi::error(template, &e))?;
    let relids: Vec<Oid> = crate::lineage::template_placeholders(template)
        .into_iter()
        .map(|p| match p {
            crate::lineage::Placeholder::Relation(r)
            | crate::lineage::Placeholder::Column(r, _) => Oid::from(r),
        })
        .collect();
    crate::cache::watch(&relids);
    Ok(sql)
}
