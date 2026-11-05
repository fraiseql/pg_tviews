//! `pg_tviews_flush_and_report()`: the read-model rows a transaction changed, in
//! the GraphQL Cascade shape.
//!
//! A mutation function calls it last. It flushes whatever is still queued, then
//! reports every TVIEW row the transaction's refreshes inserted, updated or deleted
//! so far (the write log in [`crate::queue::log`]), with each row's public `id`
//! and, optionally, its fresh `data`.

use crate::config::Publish;
use crate::error::{TViewError, TViewResult};
use crate::queue::log::feed::{self, Listed};
use crate::queue::log::{self, Change, NetChange};
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
/// journal overflowed its 10,000 rows, `truncated` is true and
/// `invalidated_types` lists the types of the rows left out. With `reset` (the
/// default) the next call reports only what changed after this one.
///
/// With `types` (GraphQL type names, [`crate::catalog::typename`]), only rows of
/// those types are reported and counted against `max_entities`; the others are
/// left out and their types listed in `omitted_types` (#230). `reset` clears them
/// too: they were seen, just not asked for.
///
/// Rows of a TVIEW the caller may not read ([`crate::visibility`]) are left out
/// the same way, asked for or not, and its type listed in `omitted_types` (#270):
/// the report tells no more than reading the TVIEW would. So are the rows of a
/// TVIEW dropped since. Under row security, a row is listed only when a read as
/// the caller returns it, with or without `include_data`; a deleted row cannot
/// be read any more, so the deleted rows of such a TVIEW are not listed and its
/// type is in `invalidated_types`. A collection is listed only when the caller
/// may read both its child and its parent TVIEW, and the child without row
/// security (its type is in `invalidated_types` otherwise), and names only the
/// parents a read as the caller returns.
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
    let summary = log::summarize(reset, capturing);
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

/// Per-entity type name, TVIEW table, whether the caller may read it, and
/// whether row security applies to the caller's reads of it.
struct EntityInfo {
    typename: String,
    oid: pgrx::pg_sys::Oid,
    readable: bool,
    row_security: bool,
}

fn entity_info(entities: &BTreeSet<&str>) -> TViewResult<HashMap<String, EntityInfo>> {
    let names: Vec<String> = entities.iter().map(|e| (*e).to_string()).collect();
    Ok(crate::catalog::queries::typenames_and_tables(names)?
        .into_iter()
        .map(|(entity, typename, _table, oid)| {
            let readable = crate::visibility::may_read_relation(oid);
            // Unknown counts as active: its rows are then read as the caller.
            let row_security =
                readable && crate::visibility::rls_active_for_caller(oid).unwrap_or(true);
            (
                entity,
                EntityInfo {
                    typename,
                    oid,
                    readable,
                    row_security,
                },
            )
        })
        .collect())
}

/// `entity`'s rows `pks` a read as the caller returns, by `pk_<entity>`, with
/// their current `data` when `with_data` (`null` otherwise): what the report
/// lists, under the caller's privileges and row security.
///
/// `pk_<entity>` is BIGINT (#209) and indexed (its primary key, or its own index on a
/// TVIEW named by another key): the keys are bound as `int8` and each row is one index
/// probe, from a kept plan shaped by their count ([`LookupKeys`]), not a scan (#240).
///
/// # Errors
/// A failed read.
fn caller_rows(
    entity: &str,
    table: pgrx::pg_sys::Oid,
    pks: Vec<i64>,
    with_data: bool,
) -> TViewResult<HashMap<i64, Value>> {
    let Some(lookup) = LookupKeys::of(pks) else {
        return Ok(HashMap::new());
    };
    let Some(meta) = crate::catalog::TviewMeta::load_by_entity(entity)? else {
        return Ok(HashMap::new());
    };
    let qi_pk = ident::quoted(meta.key_column());
    let data = if with_data {
        "t.data"
    } else {
        "NULL::pg_catalog.jsonb"
    };
    let sql = crate::lineage::render_template(&format!(
        "SELECT t.{qi_pk}, {data} FROM {{r:{}}} t WHERE {}",
        table.to_u32(),
        lookup.predicate(&format!("t.{qi_pk}"), 1)
    ))
    .map_err(|e| crate::utils::spi::error("the reported rows", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: format!("Read the rows of tv_{entity}"),
        pg_error: "its table is gone".to_string(),
    })?;
    let rows = crate::utils::spi::kept_rows_as(&sql, &lookup.args(), lookup.plan(), |row| {
        Ok((row.get::<i64>(1)?, row.get::<pgrx::JsonB>(2)?))
    })?;
    Ok(rows
        .into_iter()
        .filter_map(|(pk, data)| Some((pk?, data.map_or(Value::Null, |d| d.0))))
        .collect())
}

/// Rows read as the caller, by entity and `pk_<entity>`: their `data`, or
/// `null` when not asked for.
type Rows = HashMap<(String, i64), Value>;

/// Under row security, the changed rows a read as the caller returns (with
/// their data when `include_data`), read before the limit applies, and the
/// entities with deleted rows, which cannot be read any more.
///
/// # Errors
/// A failed read.
fn read_under_row_security<'a>(
    changes: &[&'a NetChange],
    info: &HashMap<String, EntityInfo>,
    include_data: bool,
) -> TViewResult<(Rows, BTreeSet<&'a str>)> {
    let mut rows = HashMap::new();
    let mut deleted = BTreeSet::new();
    let mut pks_by_entity: HashMap<&str, (pgrx::pg_sys::Oid, Vec<i64>)> = HashMap::new();
    for c in changes {
        let Some(i) = info.get(&c.entity).filter(|i| i.row_security) else {
            continue;
        };
        if c.change == Change::Deleted {
            deleted.insert(c.entity.as_str());
        } else {
            pks_by_entity
                .entry(&c.entity)
                .or_insert_with(|| (i.oid, Vec::new()))
                .1
                .push(c.pk);
        }
    }
    for (entity, (oid, pks)) in pks_by_entity {
        for (pk, data) in caller_rows(entity, oid, pks, include_data)? {
            rows.insert((entity.to_string(), pk), data);
        }
    }
    Ok((rows, deleted))
}

fn build_report(
    summary: &log::Summary,
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
    // A TVIEW dropped since its rows were journaled has nothing left to read.
    let readable = |entity: &str| info.get(entity).is_some_and(|i| i.readable);
    let row_security = |entity: &str| info.get(entity).is_some_and(|i| i.row_security);
    let asked =
        |entity: &str| readable(entity) && wanted.is_none_or(|w| w.contains(&typename(entity)));

    // Rows of the types not asked for, or not readable, are left out before the
    // limit applies.
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
    let changes: Vec<&NetChange> = changes
        .iter()
        .filter(|c| asked(&c.entity))
        .filter(listed)
        .collect();

    let (mut rows, shielded) = read_under_row_security(&changes, &info, include_data)?;
    let shielded: BTreeSet<String> = shielded.into_iter().map(typename).collect();
    let mut changes: Vec<NetChange> = changes
        .into_iter()
        .filter(|c| {
            !row_security(&c.entity)
                || (c.change != Change::Deleted && rows.contains_key(&(c.entity.clone(), c.pk)))
        })
        .cloned()
        .collect();
    changes.sort_by(|a, b| (&a.entity, a.pk).cmp(&(&b.entity, b.pk)));
    let (reported, left_out) = changes.split_at(changes.len().min(max_entities));

    // The fresh documents, one read per entity, only when asked for: the ids
    // come from the journal.
    if include_data {
        let mut pks_by_entity: HashMap<&str, Vec<i64>> = HashMap::new();
        for c in reported
            .iter()
            .filter(|c| c.change != Change::Deleted && !row_security(&c.entity))
        {
            pks_by_entity.entry(&c.entity).or_default().push(c.pk);
        }
        for (entity, pks) in pks_by_entity {
            if let Some(i) = info.get(entity) {
                for (pk, data) in caller_rows(entity, i.oid, pks, true)? {
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
        .chain(shielded)
        .collect();
    let types = |keep: &dyn Fn(&str) -> bool| -> BTreeSet<String> {
        entities
            .iter()
            .filter(|e| keep(e))
            .map(|e| typename(e))
            .collect()
    };
    let hidden = types(&|entity| !readable(entity));
    // Which rows of a child under row security joined or left a parent is not told.
    let row_secured = types(&row_security);
    let collections = summary
        .parents
        .as_deref()
        .map(|parents| collections(parents, wanted, &hidden, &row_secured, &mut invalidated))
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
    if wanted.is_some() || !hidden.is_empty() {
        report["omitted_types"] = json!(omitted.into_iter().collect::<Vec<_>>());
    }
    Ok(report)
}

/// The public ids (JSON text) among `ids` of rows of the TVIEW table `parent` a
/// read as the caller returns.
///
/// # Errors
/// A failed read.
fn visible_parents(
    parent: pgrx::pg_sys::Oid,
    ids: &BTreeSet<String>,
) -> TViewResult<BTreeSet<String>> {
    let plain: Vec<String> = ids
        .iter()
        .filter_map(|id| serde_json::from_str::<String>(id).ok())
        .collect();
    let sql = crate::lineage::render_template(&format!(
        "SELECT pg_catalog.to_jsonb(t.id)::pg_catalog.text FROM {{r:{}}} t \
         WHERE t.id OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.uuid[])",
        parent.to_u32()
    ))
    .map_err(|e| crate::utils::spi::error("the listed parents", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: "Read the parents of a collection".to_string(),
        pg_error: "the parent TVIEW's table is gone".to_string(),
    })?;
    let rows = crate::utils::spi::catalog_rows(
        &sql,
        &[crate::utils::spi::text_array_of(&plain)],
        |row| row.get::<String>(1),
    )?;
    Ok(rows.into_iter().flatten().collect())
}

/// The collections the journaled rows left or joined, `{"Comment.fk_post": [<post
/// id>, …]}`, named as the change feed names them (ADR 0231 §8): every parent
/// each row belonged to before or after a write, by public id, ids in text order.
/// Only rows of the `wanted` types count. A parent the feed cannot list puts its
/// child type in `invalidated`; one no row holds any more names nothing. A child
/// type in `invalidated` (rows left out past `max_entities`, an overflowed
/// journal, an unlistable parent) stands for all its collections, as a type in
/// the feed's `t` does: they are not listed. A collection whose child type is
/// `hidden`, or whose parent TVIEW the caller may not read, is left out; one of
/// a type in `row_secured` too, its type then in `invalidated`.
///
/// # Errors
/// A captured id that is not JSON.
fn collections(
    parents: &[(
        std::rc::Rc<crate::catalog::Collection>,
        crate::catalog::collections::ParentRef,
    )],
    wanted: Option<&BTreeSet<String>>,
    hidden: &BTreeSet<String>,
    row_secured: &BTreeSet<String>,
    invalidated: &mut BTreeSet<String>,
) -> TViewResult<Value> {
    let mut parent_readable: HashMap<pgrx::pg_sys::Oid, bool> = HashMap::new();
    let asked: Vec<_> = parents
        .iter()
        .filter(|(c, _)| wanted.is_none_or(|w| w.contains(&*c.child_type)))
        .filter(|(c, _)| !hidden.contains(&*c.child_type))
        .filter(|(c, _)| {
            let secured = row_secured.contains(&*c.child_type);
            if secured {
                invalidated.insert(c.child_type.to_string());
            }
            !secured
        })
        .filter(|(c, _)| {
            *parent_readable
                .entry(c.parent_table_oid)
                .or_insert_with(|| crate::visibility::may_read_relation(c.parent_table_oid))
        })
        .cloned()
        .collect();
    let mut ids: std::collections::BTreeMap<&str, (&str, pgrx::pg_sys::Oid, BTreeSet<String>)> =
        std::collections::BTreeMap::new();
    for ((collection, _), listed) in asked.iter().zip(feed::listed(&asked)) {
        let child = &*collection.child_type;
        match listed {
            Listed::Id(id) => {
                ids.entry(&collection.key)
                    .or_insert_with(|| (child, collection.parent_table_oid, BTreeSet::new()))
                    .2
                    .insert(id);
            }
            Listed::Gone => {}
            Listed::Unlisted => {
                invalidated.insert(child.to_string());
            }
        }
    }
    let mut out = Map::new();
    for (key, (child, parent, ids)) in ids {
        if invalidated.contains(child) {
            continue;
        }
        // Unknown counts as active: the parents are then read as the caller.
        let ids = if crate::visibility::rls_active_for_caller(parent).unwrap_or(true) {
            visible_parents(parent, &ids)?
        } else {
            ids
        };
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
