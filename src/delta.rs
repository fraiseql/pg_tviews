//! Map a statement's changed rows to TVIEW keys (ADR 0157).
//!
//! A `mapped` base table carries, per TVIEW, three statement-level triggers that
//! see the statement's transition tables (`INSERT`, `UPDATE`, `DELETE`: a trigger
//! with transition tables takes one event). The handler runs the table's mapping
//! query once over the changed rows, as the TVIEW's owner, and enqueues the keys it
//! returns. `all_keys` tables carry the same triggers: under the `full_refresh`
//! policy they enqueue the whole TVIEW. A partitioned table maps each row from the
//! row trigger instead: `PostgreSQL` copies only row triggers onto partitions, and
//! the transition tables of the root's statement triggers would miss the rows of
//! statements naming a partition. `TRUNCATE` refreshes the whole TVIEW, once per
//! statement however many truncated partitions fire.

use crate::catalog::TviewMeta;
use crate::config::UncascadedPolicy;
use crate::dependency::triggers::{NEW_TABLE, OLD_TABLE};
use crate::error::{TViewError, TViewResult};
use crate::lineage::{DELTA, KeyMapping};
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// The statement event a delta trigger fired for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    Insert,
    Update,
    Delete,
}

/// `(relid, event, attnums)`: what a cached query of changed rows depends on.
type DeltaKey = (u32, Event, Vec<i16>);

thread_local! {
    /// `(entity, relid)` → the rendered mapping query (`None`: a relation is gone).
    static MAPPINGS: RefCell<HashMap<(String, u32), Option<String>>> = RefCell::new(HashMap::new());
    /// `(relid, event, attnums)` → the query of the changed rows.
    static DELTAS: RefCell<HashMap<DeltaKey, String>> = RefCell::new(HashMap::new());
    /// Table → the root of its partition tree (itself when it is not a partition).
    static ROOTS: RefCell<HashMap<Oid, Oid>> = RefCell::new(HashMap::new());
    /// Number of the current `TRUNCATE` statement, counted by the `ProcessUtility`
    /// hook; 0 when the hook is not loaded.
    static TRUNCATE_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The entities the `TRUNCATE` statement numbered `.0` already refreshed.
    static TRUNCATE_REFRESHED: RefCell<(u64, std::collections::HashSet<String>)> =
        RefCell::new((0, std::collections::HashSet::new()));
}

/// Forget the cached queries; part of [`crate::queue::cache::invalidate_all_caches`].
pub fn clear_caches() {
    MAPPINGS.with(|m| m.borrow_mut().clear());
    DELTAS.with(|d| d.borrow_mut().clear());
    ROOTS.with(|r| r.borrow_mut().clear());
}

/// The entity a `pg_tviews` trigger serves: its argument (`None` for a trigger an
/// older release installed without one).
pub fn trigger_entity(trigger: &PgTrigger<'_>) -> Option<String> {
    trigger.extra_args().ok()?.into_iter().next()
}

fn suspended() -> bool {
    crate::config::suspend_triggers() || crate::suspend::is_suspended()
}

/// Statement-level trigger over the transition tables of a `mapped` or
/// `all_keys` table.
#[pg_trigger]
fn pg_tview_delta_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    crate::revision::check();
    let Some(entity) = trigger_entity(trigger) else {
        return Ok(None);
    };
    if suspended() {
        crate::suspend::record_change(&entity);
        return Ok(None);
    }
    let event = match trigger.op() {
        Ok(PgTriggerOperation::Insert) => Event::Insert,
        Ok(PgTriggerOperation::Update) => Event::Update,
        Ok(PgTriggerOperation::Delete) => Event::Delete,
        _ => return Ok(None),
    };
    let table_oid = match trigger.relation() {
        Ok(rel) => rel.oid(),
        Err(e) => error!("pg_tviews: delta trigger without a relation: {e:?}"),
    };
    if let Err(e) = map_statement(trigger, &entity, table_oid, event) {
        error!("pg_tviews: could not map the changed rows to tv_{entity} keys: {e}");
    }
    Ok(None)
}

/// A `TRUNCATE` statement starts: its truncate triggers refresh each TVIEW once.
/// Called by the `ProcessUtility` hook. No SPI.
pub fn begin_truncate() {
    TRUNCATE_EPOCH.set(TRUNCATE_EPOCH.get().wrapping_add(1).max(1));
}

/// Whether this `TRUNCATE` statement has yet to refresh `entity`, recording that it
/// does now. Always true without the hook (no statement numbers).
fn first_truncate_refresh(entity: &str) -> bool {
    let epoch = TRUNCATE_EPOCH.get();
    epoch == 0
        || TRUNCATE_REFRESHED.with(|r| {
            let mut refreshed = r.borrow_mut();
            if refreshed.0 != epoch {
                *refreshed = (epoch, std::collections::HashSet::new());
            }
            refreshed.1.insert(entity.to_string())
        })
}

/// `TRUNCATE` of a base table: refresh the whole TVIEW now (no flush trigger
/// follows a `TRUNCATE`). `TRUNCATE` of a partitioned table fires this on each
/// truncated partition too, after all of them are empty: the first one refreshes.
#[pg_trigger]
fn pg_tview_truncate_trigger<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    crate::revision::check();
    let Some(entity) = trigger_entity(trigger) else {
        return Ok(None);
    };
    if suspended() {
        crate::suspend::record_change(&entity);
        return Ok(None);
    }
    if !first_truncate_refresh(&entity) {
        return Ok(None);
    }
    crate::queue::enqueue_refresh_all(&entity);
    if let Err(e) = crate::queue::flush_refresh_queue() {
        error!("pg_tviews: could not refresh tv_{entity} after TRUNCATE: {e}");
    }
    Ok(None)
}

/// Refresh in full every TVIEW whose row trigger sits on the root of `table`'s
/// partition tree: the rows of a partition attached to it or detached from it
/// changed with no row trigger firing.
///
/// # Errors
/// Returns an error if the catalog query or a refresh fails.
pub fn refresh_tviews_over(table: Oid) -> TViewResult<()> {
    let root = partition_root(table)?;
    let entities = crate::dependency::triggers::row_trigger_entities(root)?;
    if entities.is_empty() {
        return Ok(());
    }
    for entity in &entities {
        if suspended() {
            crate::suspend::record_change(entity);
        } else {
            crate::queue::enqueue_refresh_all(entity);
        }
    }
    crate::queue::flush_refresh_queue()
}

/// The query that maps changed rows of `base_table`, read from a relation named
/// `pg_tviews_delta`, to keys of TVIEW `tview` (ADR 0157), with the current names
/// of what it reads. NULL when writes to the table do not map through a query of
/// their own (`propagated`, `all_keys`) or the TVIEW does not read it.
#[pg_extern]
fn pg_tviews_mapping_query(tview: &str, base_table: pg_sys::Oid) -> Option<String> {
    crate::revision::check();
    let entity = tview.strip_prefix("tv_").unwrap_or(tview);
    let meta = TviewMeta::load_by_entity(entity).ok()??;
    let mapping = meta.key_mapping(base_table, None)?;
    match mapping.kind.as_str() {
        "local" => Some(format!(
            "SELECT DISTINCT {} FROM {DELTA}",
            crate::utils::quote_identifier(mapping.column.as_deref()?)
        )),
        "mapped" => rendered(entity, mapping).ok()?,
        _ => None,
    }
}

/// Map the rows a statement changed in `table_oid` to `entity`'s keys and enqueue
/// them.
fn map_statement(
    trigger: &PgTrigger<'_>,
    entity: &str,
    table_oid: Oid,
    event: Event,
) -> TViewResult<()> {
    let meta = TviewMeta::load_by_entity(entity)?.ok_or_else(|| TViewError::MetadataNotFound {
        entity: entity.to_string(),
    })?;
    let Some(mapping) = meta.key_mapping(table_oid, None) else {
        return refresh_all(entity, "its mapping of a written table is unknown");
    };
    match mapping.kind.as_str() {
        "mapped" => {
            if event == Event::Update && fan_out(trigger, entity, table_oid, mapping)? {
                return Ok(());
            }
            let Some(keys_sql) = rendered(entity, mapping)? else {
                return refresh_all(entity, "a relation its mapping reads is gone");
            };
            let delta = delta_sql(table_oid, event, &mapping.attnums)?;
            let keys = run_with_transition_tables(
                trigger,
                entity,
                &format!(
                    "WITH {DELTA} AS ({delta}) \
                     SELECT DISTINCT k::pg_catalog.int8 FROM ({keys_sql}) s(k) WHERE k IS NOT NULL"
                ),
            )?;
            if !keys.is_empty() {
                crate::queue::enqueue_refresh_bulk(entity, keys);
            }
        }
        "all_keys" if meta.uncascaded_policy == UncascadedPolicy::FullRefresh => {
            let table = if event == Event::Delete {
                OLD_TABLE
            } else {
                NEW_TABLE
            };
            let changed = run_with_transition_tables(
                trigger,
                entity,
                &format!("SELECT 1::pg_catalog.int8 FROM {table} LIMIT 1"),
            )?;
            if !changed.is_empty() {
                crate::queue::enqueue_refresh_all(entity);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Write an UPDATE into every TVIEW row it reaches instead of recomputing them
/// (issue #120), when the mapping has a fan-out patch and every updated row
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
    if !crate::config::direct_patch_enabled() || !crate::lifecycle::check_jsonb_delta_available() {
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
    let pairs = with_transition_tables(trigger, entity, |client| {
        let mut pairs = Vec::new();
        for row in client.select(&sql, None, &[])? {
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
        let mut fields = serde_json::Map::new();
        for column in changed {
            let Some((_, data_key)) = fanout.fields.iter().find(|(c, _)| c == column) else {
                return Ok(false);
            };
            fields.insert(
                data_key.clone(),
                new.get(column).cloned().unwrap_or_default(),
            );
        }
        patches.push((key, fields));
    }
    for (key, fields) in patches {
        crate::queue::patch::record_fanout(
            (entity.to_string(), fanout.lookup_col.clone(), key),
            fields,
        );
    }
    Ok(true)
}

/// Map one changed row of a partitioned `mapped` / `all_keys` table, from the row
/// trigger (the partition the row is in cannot have transition tables). Returns
/// false when `entity` has no such mapping of the table.
pub fn map_row(trigger: &PgTrigger<'_>, entity: &str, table_oid: Oid) -> TViewResult<bool> {
    let Some(meta) = TviewMeta::load_by_entity(entity)? else {
        return Ok(false);
    };
    let root = partition_root(table_oid)?;
    let Some(mapping) = meta.key_mapping(table_oid, Some(root)) else {
        return Ok(false);
    };
    match mapping.kind.as_str() {
        "mapped" => {
            let Some(keys_sql) = rendered(entity, mapping)? else {
                refresh_all(entity, "a relation its mapping reads is gone")?;
                return Ok(true);
            };
            // SAFETY: inside a row trigger the TriggerData, its relation and the
            // OLD/NEW tuples are valid; each is copied into a composite datum of the
            // partition's row type.
            let images = unsafe {
                let td = trigger.trigger_data();
                let rel = td.tg_relation;
                let rowtype = pg_sys::get_rel_type_id((*rel).rd_id);
                let mut images = Vec::new();
                for tuple in [td.tg_trigtuple, td.tg_newtuple] {
                    if !tuple.is_null() {
                        let datum = pg_sys::heap_copy_tuple_as_datum(tuple, (*rel).rd_att);
                        images.push(DatumWithOid::new(datum, rowtype));
                    }
                }
                // An UPDATE has OLD in tg_trigtuple and NEW in tg_newtuple; an
                // INSERT has NEW in tg_trigtuple.
                images
            };
            let delta = (1..=images.len())
                .map(|i| format!("SELECT (${i}).*"))
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            let sql = format!(
                "WITH {DELTA} AS ({delta}) \
                 SELECT DISTINCT k::pg_catalog.int8 FROM ({keys_sql}) s(k) WHERE k IS NOT NULL"
            );
            let _owner = crate::owner::AsOwner::of_entity(entity)?;
            let keys = Spi::connect(|client| {
                let mut keys = Vec::new();
                for row in client.select(&sql, None, &images)? {
                    if let Some(k) = row.get::<i64>(1)? {
                        keys.push(k);
                    }
                }
                Ok::<_, spi::Error>(keys)
            })
            .map_err(|e| spi_error(&sql, &e))?;
            if !keys.is_empty() {
                crate::queue::enqueue_refresh_bulk(entity, keys);
            }
            Ok(true)
        }
        "all_keys" => {
            if meta.uncascaded_policy == UncascadedPolicy::FullRefresh {
                crate::queue::enqueue_refresh_all(entity);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Refresh `entity` in full because its mapping cannot run, and say once why.
fn refresh_all(entity: &str, why: &str) -> TViewResult<()> {
    crate::utils::log_once(
        &format!("unmapped:{entity}"),
        &format!(
            "tv_{entity} is refreshed in full on writes because {why}; \
             SELECT tviews.pg_tviews_reregister('{entity}') re-derives it"
        ),
    );
    crate::queue::enqueue_refresh_all(entity);
    Ok(())
}

/// The mapping query of `mapping`, with the current names of what it reads
/// (cached until one of them changes).
fn rendered(entity: &str, mapping: &KeyMapping) -> TViewResult<Option<String>> {
    let key = (entity.to_string(), mapping.relid);
    if let Some(sql) = MAPPINGS.with(|m| m.borrow().get(&key).cloned()) {
        return Ok(sql);
    }
    let template = mapping.sql.clone().unwrap_or_default();
    let sql = crate::lineage::render_template(&template).map_err(|e| spi_error(&template, &e))?;
    // A rename of any relation it reads invalidates the cache.
    let relids: Vec<Oid> = crate::lineage::template_placeholders(&template)
        .into_iter()
        .map(|p| match p {
            crate::lineage::Placeholder::Relation(r)
            | crate::lineage::Placeholder::Column(r, _) => Oid::from(r),
        })
        .collect();
    crate::queue::cache::watch(&relids);
    MAPPINGS.with(|m| m.borrow_mut().insert(key, sql.clone()));
    Ok(sql)
}

/// The changed rows of a statement: the new rows of an INSERT, the old rows of a
/// DELETE, both images of an UPDATE. An UPDATE row whose columns the TVIEW reads
/// (`attnums`) are unchanged, matched to its other image by primary key, is left
/// out of both.
fn delta_sql(table_oid: Oid, event: Event, attnums: &[i16]) -> TViewResult<String> {
    let key = (table_oid.to_u32(), event, attnums.to_vec());
    if let Some(sql) = DELTAS.with(|d| d.borrow().get(&key).cloned()) {
        return Ok(sql);
    }
    let sql = match event {
        Event::Insert => format!("SELECT * FROM {NEW_TABLE}"),
        Event::Delete => format!("SELECT * FROM {OLD_TABLE}"),
        Event::Update => match update_filter(table_oid, attnums)? {
            Some(same) => format!(
                "SELECT o.* FROM {OLD_TABLE} o \
                   WHERE NOT EXISTS (SELECT 1 FROM {NEW_TABLE} n WHERE {same}) \
                 UNION ALL \
                 SELECT n.* FROM {NEW_TABLE} n \
                   WHERE NOT EXISTS (SELECT 1 FROM {OLD_TABLE} o WHERE {same})"
            ),
            None => format!("SELECT * FROM {OLD_TABLE} UNION ALL SELECT * FROM {NEW_TABLE}"),
        },
    };
    DELTAS.with(|d| d.borrow_mut().insert(key, sql.clone()));
    Ok(sql)
}

/// `n.pk = o.pk AND ROW(n.<cols>) *= ROW(o.<cols>)`: the same row, its columns
/// the TVIEW reads unchanged. `None` without a usable primary key or a known
/// column list (then every row counts).
fn update_filter(table_oid: Oid, attnums: &[i16]) -> TViewResult<Option<String>> {
    let Some((same_row, columns)) = row_pairing(table_oid, attnums)? else {
        return Ok(None);
    };
    let record = |alias: &str| {
        format!(
            "ROW({})::pg_catalog.record",
            columns
                .iter()
                .map(|c| format!("{alias}.{}", crate::utils::quote_identifier(c)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(Some(format!(
        "{same_row} AND {} OPERATOR(pg_catalog.*=) {}",
        record("n"),
        record("o")
    )))
}

/// How to pair the old and new image of an updated row, `n.pk = o.pk` over the
/// primary key, and the names of the columns `attnums` the TVIEW reads. `None`
/// without a primary key whose types have a `pg_catalog` equality, or without a
/// known column list.
fn row_pairing(table_oid: Oid, attnums: &[i16]) -> TViewResult<Option<(String, Vec<String>)>> {
    if attnums.is_empty() {
        return Ok(None);
    }
    // SAFETY: plain OID / int2[] datums.
    let args = unsafe {
        [
            DatumWithOid::new(table_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()),
            DatumWithOid::new(
                attnums.to_vec(),
                PgOid::BuiltIn(PgBuiltInOids::INT2ARRAYOID).value(),
            ),
        ]
    };
    let (keys, columns) = Spi::connect(|client| {
        // Key columns with the equality operator of their type, if it is in pg_catalog.
        let mut keys = Vec::new();
        for row in client.select(
            "SELECT a.attname::pg_catalog.text, \
                    EXISTS (SELECT 1 FROM pg_catalog.pg_operator o \
                            WHERE o.oprname = '=' AND o.oprleft = a.atttypid \
                              AND o.oprright = a.atttypid \
                              AND o.oprnamespace = 'pg_catalog'::pg_catalog.regnamespace) \
             FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey) \
             WHERE i.indrelid = $1 AND i.indisprimary ORDER BY a.attnum",
            None,
            &args[..1],
        )? {
            keys.push((row.get::<String>(1)?.unwrap_or_default(), row.get::<bool>(2)?.unwrap_or(false)));
        }
        let mut columns = Vec::new();
        for row in client.select(
            "SELECT attname::pg_catalog.text FROM pg_catalog.pg_attribute \
             WHERE attrelid = $1 AND attnum = ANY ($2) AND NOT attisdropped ORDER BY attnum",
            None,
            &args,
        )? {
            columns.push(row.get::<String>(1)?.unwrap_or_default());
        }
        Ok::<_, spi::Error>((keys, columns))
    })
    .map_err(|e| spi_error("primary key and columns of a mapped table", &e))?;
    if keys.is_empty()
        || keys.iter().any(|(_, catalog_eq)| !catalog_eq)
        || columns.len() != attnums.len()
    {
        return Ok(None);
    }
    let same_row = keys
        .iter()
        .map(|(k, _)| {
            let k = crate::utils::quote_identifier(k);
            format!("n.{k} OPERATOR(pg_catalog.=) o.{k}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(Some((same_row, columns)))
}

/// Run `sql`, which reads the statement's transition tables, as `entity`'s owner
/// and return the `int8` of each row's first column.
fn run_with_transition_tables(
    trigger: &PgTrigger<'_>,
    entity: &str,
    sql: &str,
) -> TViewResult<Vec<i64>> {
    with_transition_tables(trigger, entity, |client| {
        let mut keys = Vec::new();
        for row in client.select(sql, None, &[])? {
            if let Some(k) = row.get::<i64>(1)? {
                keys.push(k);
            }
        }
        Ok(keys)
    })
}

/// Run `f` on an SPI connection that sees the statement's transition tables, as
/// `entity`'s owner.
fn with_transition_tables<T>(
    trigger: &PgTrigger<'_>,
    entity: &str,
    f: impl FnOnce(&spi::SpiClient<'_>) -> spi::Result<T>,
) -> TViewResult<T> {
    let _owner = crate::owner::AsOwner::of_entity(entity)?;
    Spi::connect(|client| {
        // SAFETY: inside an AFTER statement trigger the TriggerData is valid; this
        // registers its transition tables with the SPI connection just opened.
        let registered = unsafe {
            pg_sys::SPI_register_trigger_data(std::ptr::from_ref(trigger.trigger_data()).cast_mut())
        };
        if registered != pg_sys::SPI_OK_TD_REGISTER.cast_signed() {
            return Err(spi::Error::from(TViewError::SpiError {
                query: "SPI_register_trigger_data".to_string(),
                error: format!("returned {registered}"),
            }));
        }
        f(client)
    })
    .map_err(|e| spi_error("a query over the transition tables", &e))
}

/// The root of `table_oid`'s partition tree, `table_oid` itself if it is not a
/// partition (cached).
pub fn partition_root(table_oid: Oid) -> TViewResult<Oid> {
    if let Some(root) = ROOTS.with(|r| r.borrow().get(&table_oid).copied()) {
        return Ok(root);
    }
    let root = Spi::get_one_with_args::<Oid>(
        "SELECT COALESCE(pg_catalog.pg_partition_root($1)::pg_catalog.oid, $1)",
        // SAFETY: a plain OID datum.
        &[unsafe {
            DatumWithOid::new(
                table_oid,
                PgOid::BuiltIn(PgBuiltInOids::REGCLASSOID).value(),
            )
        }],
    )
    .map_err(|e| spi_error("pg_partition_root", &e))?
    .unwrap_or(table_oid);
    ROOTS.with(|r| r.borrow_mut().insert(table_oid, root));
    Ok(root)
}

fn spi_error(query: &str, e: &impl std::fmt::Display) -> TViewError {
    TViewError::SpiError {
        query: query.to_string(),
        error: e.to_string(),
    }
}
