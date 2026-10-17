//! `pg_tviews_flush_and_report()`: the read-model rows a transaction changed, in
//! the GraphQL Cascade shape.
//!
//! A mutation function calls it last. It flushes whatever is still queued, then
//! reports every TVIEW row the transaction's refreshes inserted, updated or deleted
//! so far (the journal in [`crate::queue::affected`]), with each row's public `id`
//! and, optionally, its fresh `data`.

use crate::error::{TViewError, TViewResult};
use crate::queue::affected::{self, Change, NetChange};
use crate::queue::published::{self, Listed, Publish};
use crate::utils::ident;
use crate::utils::spi::LookupKeys;
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};

/// Flush pending refreshes and report the TVIEW rows this transaction changed.
///
/// Returns `{"updated": [...], "deleted": [...], "truncated": bool,
/// "invalidated_types": [...]}`. An updated entry is
/// `{"__typename", "id", "operation": "CREATED" | "UPDATED", "data"}` (`data` only
/// with `include_data`); a deleted entry is `{"__typename", "id"}`. Entries are
/// ordered by entity, then primary key. Past `max_entities` entries, or when the
/// journal overflowed `pg_tviews.report_max_tracked`, `truncated` is true and
/// `invalidated_types` lists the types of the rows left out. With `reset` (the
/// default) the next call reports only what changed after this one.
///
/// With `types` (GraphQL type names, [`crate::catalog::typename`]), only rows of
/// those types are reported and counted against `max_entities`; the others are
/// left out and their types listed in `omitted_types` (#230). `reset` clears them
/// too: they were seen, just not asked for.
///
/// When the rows were written with `pg_tviews.publish_changes = 'rows'`,
/// `collections` names the parents whose child lists they left or joined (#244),
/// as the change feed does ([`collections`]); absent otherwise, nothing having
/// been captured.
///
/// # Errors
/// `InvalidInput` (22023) for a type name no TVIEW has, before anything is
/// flushed or reset; otherwise an error if the flush or a catalog/TVIEW read fails.
pub(crate) fn flush_and_report(
    max_entities: i32,
    include_data: bool,
    reset: bool,
    types: Option<Vec<String>>,
) -> TViewResult<Value> {
    let wanted = types.map(requested_types).transpose()?;
    crate::flush::flush_refresh_queue()?;
    let capturing = crate::config::publish_changes() == Some(Publish::Rows);
    let summary = affected::summarize(reset, capturing);
    let limit = usize::try_from(max_entities).unwrap_or(0);
    build_report(&summary, limit, include_data, wanted.as_ref())
}

/// How many known type names an unknown-type error lists.
const KNOWN_TYPES_LISTED: usize = 20;

/// `types` as a set, once each is known to be a TVIEW's type name.
fn requested_types(types: Vec<String>) -> TViewResult<BTreeSet<String>> {
    let known: BTreeSet<String> = crate::catalog::row::typenames()?
        .into_iter()
        .map(|(_, typename)| typename)
        .collect();
    let unknown: Vec<&str> = types
        .iter()
        .filter(|t| !known.contains(*t))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        let listed: Vec<&str> = known
            .iter()
            .take(KNOWN_TYPES_LISTED)
            .map(String::as_str)
            .collect();
        let more = if known.len() > KNOWN_TYPES_LISTED {
            ", …"
        } else {
            ""
        };
        return Err(TViewError::InvalidInput {
            parameter: "types".to_string(),
            reason: format!(
                "unknown type(s) {}; known: {}{more}",
                unknown.join(", "),
                listed.join(", ")
            ),
        });
    }
    Ok(types.into_iter().collect())
}

/// Per-entity type name and qualified TVIEW table.
struct EntityInfo {
    typename: String,
    table: String,
}

fn entity_info(entities: &BTreeSet<&str>) -> TViewResult<HashMap<String, EntityInfo>> {
    let names: Vec<String> = entities.iter().map(|e| (*e).to_string()).collect();
    Ok(crate::catalog::queries::typenames_and_tables(names)?
        .into_iter()
        .map(|(entity, typename, table)| (entity, EntityInfo { typename, table }))
        .collect())
}

/// The current `data` of `entity`'s rows `pks`, by `pk_<entity>`.
///
/// `pk_<entity>` is BIGINT (#209) and indexed (its primary key, or its own index on a
/// TVIEW named by another key): the keys are bound as `int8` and each row is one index
/// probe, from a kept plan shaped by their count ([`LookupKeys`]), not a scan (#240).
///
/// # Errors
/// A failed read.
fn current_data(entity: &str, table: &str, pks: Vec<i64>) -> TViewResult<HashMap<i64, Value>> {
    let Some(lookup) = LookupKeys::of(pks) else {
        return Ok(HashMap::new());
    };
    let qi_pk = ident::quoted(&format!("pk_{entity}"));
    let sql = format!(
        "SELECT t.{qi_pk}, t.data FROM {table} t WHERE {}",
        lookup.predicate(&format!("t.{qi_pk}"), 1)
    );
    let rows = crate::utils::spi::kept_rows_as(&sql, &[lookup.arg()], lookup.plan(), |row| {
        Ok((row.get::<i64>(1)?, row.get::<pgrx::JsonB>(2)?))
    })?;
    Ok(rows
        .into_iter()
        .filter_map(|(pk, data)| Some((pk?, data.map_or(Value::Null, |d| d.0))))
        .collect())
}

fn build_report(
    summary: &affected::Summary,
    max_entities: usize,
    include_data: bool,
    wanted: Option<&BTreeSet<String>>,
) -> TViewResult<Value> {
    let (changes, overflow) = (&summary.changes, &summary.overflow);
    let entities: BTreeSet<&str> = changes
        .iter()
        .map(|c| c.entity.as_str())
        .chain(overflow.iter().map(String::as_str))
        .collect();
    let info = entity_info(&entities)?;
    let typename = |entity: &str| {
        info.get(entity).map_or_else(
            || crate::catalog::typename(entity, None),
            |i| i.typename.clone(),
        )
    };
    let asked = |entity: &str| wanted.is_none_or(|w| w.contains(&typename(entity)));

    // Rows of the types not asked for are left out before the limit applies.
    let omitted: BTreeSet<String> = changes
        .iter()
        .map(|c| c.entity.as_str())
        .chain(overflow.iter().map(String::as_str))
        .filter(|entity| !asked(entity))
        .map(typename)
        .collect();

    // A type whose rows were not all journaled (rebuilt, or past the cap) is
    // reported by name, and stands for all its rows: without the data, which
    // tells, a row journaled before a rebuild may name one that is gone. The
    // change feed publishes such a type by name, without its rows, too.
    let listed = |c: &&NetChange| include_data || !overflow.contains(&c.entity);
    // Flush order is not deterministic across runs, so report in a fixed order:
    // entity, then key.
    let mut changes: Vec<NetChange> = changes
        .iter()
        .filter(|c| asked(&c.entity))
        .filter(listed)
        .cloned()
        .collect();
    changes.sort_by(|a, b| (&a.entity, a.pk).cmp(&(&b.entity, b.pk)));
    let (reported, left_out) = changes.split_at(changes.len().min(max_entities));

    // The fresh documents, one read per entity, only when asked for: the ids
    // come from the journal.
    let mut rows: HashMap<(String, i64), Value> = HashMap::new();
    if include_data {
        let mut pks_by_entity: HashMap<&str, Vec<i64>> = HashMap::new();
        for c in reported.iter().filter(|c| c.change != Change::Deleted) {
            pks_by_entity.entry(&c.entity).or_default().push(c.pk);
        }
        for (entity, pks) in pks_by_entity {
            if let Some(i) = info.get(entity) {
                for (pk, data) in current_data(entity, &i.table, pks)? {
                    rows.insert((entity.to_string(), pk), data);
                }
            }
        }
    }

    let mut updated = Vec::new();
    let mut deleted = Vec::new();
    for c in reported {
        let mut item = Map::new();
        item.insert("__typename".into(), Value::String(typename(&c.entity)));
        item.insert(
            "id".into(),
            c.id.as_ref()
                .map_or(Value::Null, crate::catalog::RowId::value),
        );
        if c.change == Change::Deleted {
            deleted.push(Value::Object(item));
            continue;
        }
        let op = if c.change == Change::Inserted {
            "CREATED"
        } else {
            "UPDATED"
        };
        item.insert("operation".into(), Value::String(op.into()));
        if include_data {
            // A row written then removed by a later, unjournaled path is skipped.
            let Some(data) = rows.remove(&(c.entity.clone(), c.pk)) else {
                continue;
            };
            item.insert("data".into(), data);
        }
        updated.push(Value::Object(item));
    }

    let mut invalidated: BTreeSet<String> = left_out
        .iter()
        .map(|c| c.entity.as_str())
        .chain(overflow.iter().map(String::as_str).filter(|e| asked(e)))
        .map(typename)
        .collect();
    let collections = summary
        .parents
        .as_deref()
        .map(|parents| collections(parents, wanted, &mut invalidated))
        .transpose()?;
    let mut report = json!({
        "updated": updated,
        "deleted": deleted,
        "truncated": !invalidated.is_empty(),
        "invalidated_types": invalidated.into_iter().collect::<Vec<_>>(),
    });
    if let Some(collections) = collections {
        report["collections"] = collections;
    }
    if wanted.is_some() {
        report["omitted_types"] = json!(omitted.into_iter().collect::<Vec<_>>());
    }
    Ok(report)
}

/// The collections the journaled rows left or joined, `{"Comment.fk_post": [<post
/// id>, …]}`, named as the change feed names them (ADR 0231 §8): every parent
/// each row belonged to before or after a write, by public id, ids in text order.
/// Only rows of the `wanted` types count. A parent the feed cannot list puts its
/// child type in `invalidated`; one no row holds any more names nothing. A child
/// type in `invalidated` (rows left out past `max_entities`, an overflowed
/// journal, an unlistable parent) stands for all its collections, as a type in
/// the feed's `t` does: they are not listed.
///
/// # Errors
/// A captured id that is not JSON.
fn collections(
    parents: &[(
        std::rc::Rc<crate::catalog::Collection>,
        crate::catalog::collections::ParentRef,
    )],
    wanted: Option<&BTreeSet<String>>,
    invalidated: &mut BTreeSet<String>,
) -> TViewResult<Value> {
    let asked: Vec<_> = parents
        .iter()
        .filter(|(c, _)| wanted.is_none_or(|w| w.contains(&*c.child_type)))
        .cloned()
        .collect();
    let mut ids: std::collections::BTreeMap<&str, (&str, BTreeSet<String>)> =
        std::collections::BTreeMap::new();
    for ((collection, _), listed) in asked.iter().zip(published::listed(&asked)) {
        let child = &*collection.child_type;
        match listed {
            Listed::Id(id) => {
                ids.entry(&collection.key)
                    .or_insert_with(|| (child, BTreeSet::new()))
                    .1
                    .insert(id);
            }
            Listed::Gone => {}
            Listed::Unlisted => {
                invalidated.insert(child.to_string());
            }
        }
    }
    let mut out = Map::new();
    for (key, (child, ids)) in ids {
        if invalidated.contains(child) {
            continue;
        }
        let values = ids
            .iter()
            .map(|id| serde_json::from_str(id))
            .collect::<Result<Vec<Value>, _>>()
            .map_err(|e| TViewError::SerializationError {
                message: format!("{key}: unreadable parent id: {e}"),
            })?;
        out.insert(key.to_string(), Value::Array(values));
    }
    Ok(Value::Object(out))
}
