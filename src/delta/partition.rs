//! Partitioned tables: a row mapped from the row trigger PostgreSQL copies onto
//! each partition, the rows of a partition leaving the tree (`DROP TABLE`, plain
//! `DETACH`), and the TVIEWs refreshed in full when a partition comes or goes
//! otherwise.

use super::mapping::{refresh_all, rendered};
use super::render::{computed_columns, key_of, key_sql_type};
use super::transition::{changed_values, crosscheck_keys, texts};
use super::trigger::{suspended, truncate_mapped};
use crate::catalog::TviewMeta;
use crate::config::UncascadedPolicy;
use crate::error::{TViewError, TViewResult};
use crate::lineage::{DELTA, MappingKind};
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

/// Refresh in full every TVIEW whose row trigger sits on the root of `table`'s
/// partition tree: the rows of a partition attached to it or detached from it
/// changed with no row trigger firing.
///
/// # Errors
/// Returns an error if the catalog query or a refresh fails.
pub fn refresh_tviews_over(table: Oid) -> TViewResult<()> {
    let root = crate::catalog::partition_root(table)?;
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
    crate::flush::flush_refresh_queue()
}

/// The rows of `partition` are about to leave its partition tree (`DROP TABLE`,
/// `DROP FOREIGN TABLE`, `DETACH PARTITION`, `TRUNCATE`): enqueue, for every TVIEW over a table
/// above it in the tree, the keys they reach, as a `DELETE` of those rows would.
/// The refresh that follows the statement finds those keys gone. Called before
/// `PostgreSQL` runs the statement, with the partition locked as the statement
/// locks it.
///
/// # Errors
/// Returns an error if the catalog or a mapping query fails.
pub fn map_leaving_partition(partition: Oid, lock_parent: bool) -> TViewResult<()> {
    // Its rows leave the tree: a link change.
    crate::concurrency::note_link_change();
    let readers = readers_above(partition)?;
    if readers.is_empty() {
        return Ok(());
    }
    // Lock as the statement will (the parent for DROP and DETACH, the partition,
    // then everything below it), so that no write reaches the rows between reading
    // them and the statement: a write naming a sub-partition locks neither of the
    // first two. TRUNCATE leaves the parent alone.
    let lockmode = pg_sys::AccessExclusiveLock.cast_signed();
    let parent = if lock_parent {
        oids(
            "SELECT inhparent FROM pg_catalog.pg_inherits WHERE inhrelid = $1",
            partition,
        )?
    } else {
        Vec::new()
    };
    let below = oids(
        "SELECT relid FROM pg_catalog.pg_partition_tree($1) WHERE relid <> $1 ORDER BY level, relid",
        partition,
    )?;
    // SAFETY: relation locks by OID, held until the transaction ends.
    unsafe {
        for relid in parent.iter().chain([&partition]).chain(&below) {
            pg_sys::LockRelationOid(*relid, lockmode);
        }
    }
    // A foreign partition is not read: it may be unreachable, and its rows leave
    // with a full refresh.
    // SAFETY: a syscache lookup by OID.
    let foreign = unsafe { pg_sys::get_rel_relkind(partition) }
        == pg_sys::RELKIND_FOREIGN_TABLE.cast_signed();
    let truncated: Vec<Oid> = std::iter::once(partition).chain(below).collect();
    for (entity, ancestor) in readers {
        if suspended() {
            crate::suspend::record_change(&entity);
        } else if foreign {
            crate::queue::enqueue_refresh_all(&entity);
        } else {
            map_partition_rows(&entity, partition, ancestor, false)?;
        }
        truncate_mapped(&entity, &truncated);
    }
    Ok(())
}

/// `partition` was just attached to a partition tree (`ATTACH PARTITION`): enqueue,
/// for every TVIEW over a table above it, the keys its rows reach, as an `INSERT`
/// of those rows would, with the same value locks. A foreign partition, or one
/// the TVIEW's owner cannot read whole, refreshes the TVIEW in full.
///
/// # Errors
/// Returns an error if the catalog or a mapping query fails.
pub fn map_entering_partition(partition: Oid) -> TViewResult<()> {
    // Its rows enter the tree: a link change.
    crate::concurrency::note_link_change();
    let readers = readers_above(partition)?;
    // SAFETY: a syscache lookup by OID.
    let foreign = unsafe { pg_sys::get_rel_relkind(partition) }
        == pg_sys::RELKIND_FOREIGN_TABLE.cast_signed();
    for (entity, ancestor) in readers {
        if suspended() {
            crate::suspend::record_change(&entity);
        } else if foreign {
            crate::queue::enqueue_refresh_all(&entity);
        } else {
            map_partition_rows(&entity, partition, ancestor, true)?;
        }
    }
    Ok(())
}

/// The TVIEWs over each table above `partition` in its tree (the root, and a
/// partitioned table in between), with that table.
fn readers_above(partition: Oid) -> TViewResult<Vec<(String, Oid)>> {
    let mut readers = Vec::new();
    for ancestor in oids(
        "SELECT relid FROM pg_catalog.pg_partition_ancestors($1) WHERE relid <> $1",
        partition,
    )? {
        for entity in crate::dependency::triggers::row_trigger_entities(ancestor)? {
            readers.push((entity, ancestor));
        }
    }
    Ok(readers)
}

/// The OIDs `sql` returns for `relid` (its `$1`).
fn oids(sql: &str, relid: Oid) -> TViewResult<Vec<Oid>> {
    Spi::connect(|client| {
        let mut out = Vec::new();
        for row in client.select(sql, None, &[crate::utils::spi::oid(relid)])? {
            out.extend(row.get::<Oid>(1)?);
        }
        Ok::<_, spi::Error>(out)
    })
    .map_err(|e| crate::utils::spi::error(sql, &e))
}

/// Enqueue `entity`'s keys reached by the rows of `partition`, below `root`, the
/// table the TVIEW reads. Rows `entering` the tree lock their join values first,
/// as any writer does (ADR 0207); rows leaving it are covered by the statement's
/// `ACCESS EXCLUSIVE` locks.
fn map_partition_rows(entity: &str, partition: Oid, root: Oid, entering: bool) -> TViewResult<()> {
    let Some(meta) = TviewMeta::load_by_entity(entity)? else {
        return Ok(());
    };
    let Some(mapping) = meta.key_mapping(partition, Some(root)) else {
        refresh_all(entity, "its mapping of a written table is unknown");
        return Ok(());
    };
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    let keys_sql = match mapping.kind {
        MappingKind::AllKeys if full_refresh => None,
        MappingKind::Local => mapping.column.as_deref().map(|column| {
            format!(
                "SELECT {} FROM {DELTA}",
                crate::utils::ident::quoted(column)
            )
        }),
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            let Some(sql) = rendered(entity, mapping)? else {
                refresh_all(entity, "a relation its mapping reads is gone");
                return Ok(());
            };
            Some(sql)
        }
        // Writes to it map to no key (an `all_keys` read that cannot be traced).
        MappingKind::AllKeys => return Ok(()),
        _ => None,
    };
    let _owner = crate::owner::AsOwner::of_entity(entity)?;
    // The owner reads the partition itself, under the partition's own privileges
    // and row security, not the root's: one it cannot read whole is refreshed in
    // full.
    let readable = Spi::get_one_with_args::<bool>(
        "SELECT pg_catalog.has_table_privilege($1, 'SELECT') AND NOT c.relrowsecurity \
         FROM pg_catalog.pg_class c WHERE c.oid = $1",
        &[crate::utils::spi::regclass(partition)],
    )
    .map_err(|e| crate::utils::spi::error("has_table_privilege", &e))?
    .unwrap_or(false);
    let Some(keys_sql) = keys_sql.filter(|_| readable) else {
        crate::queue::enqueue_refresh_all(entity);
        return Ok(());
    };
    let rows = format!(
        "SELECT * FROM {}",
        crate::utils::qualified_relname_from_oid(partition)?
    );
    let delta = match computed_columns(entity, root)? {
        Some(columns) => format!("SELECT {columns} FROM ({rows}) r"),
        None => rows,
    };
    let key_type = &meta.key_type;
    let sql = format!(
        "WITH {DELTA} AS ({delta}) \
         SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
        key_sql_type(key_type)
    );
    if entering {
        crate::concurrency::lock_changed_rows(mapping, |column| {
            Spi::connect_mut(|client| texts(client, None, &changed_values(&delta, column), &[]))
                .map_err(|e| crate::utils::spi::error("the values the entering rows hold", &e))
        })?;
    }
    // Read-write: a fresh snapshot, which sees every row committed before the locks.
    let keys = Spi::connect_mut(|client| {
        let mut keys = Vec::new();
        for row in crate::utils::spi::kept_update(client, &sql, &[])? {
            keys.extend(key_of(&row, key_type)?);
        }
        crosscheck_keys(&sql, &[], &keys)?;
        Ok::<_, TViewError>(keys)
    })
    .map_err(|e| crate::utils::spi::error(&sql, &e))?;
    if !keys.is_empty() {
        crate::queue::enqueue_refresh_bulk(entity, keys);
    }
    Ok(())
}

/// Map one changed row of a partitioned `mapped` / `all_keys` table, from the row
/// trigger that `PostgreSQL` copied onto the partition the row is in (only row
/// triggers are copied). Returns false when `entity` has no such mapping of the
/// table.
pub fn map_row(trigger: &PgTrigger<'_>, entity: &str, table_oid: Oid) -> TViewResult<bool> {
    let Some(meta) = TviewMeta::load_by_entity(entity)? else {
        return Ok(false);
    };
    let root = crate::catalog::partition_root(table_oid)?;
    let Some(mapping) = meta.key_mapping(table_oid, Some(root)) else {
        return Ok(false);
    };
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    match mapping.kind {
        MappingKind::AllKeys if full_refresh => {
            crate::queue::enqueue_refresh_all(entity);
            Ok(true)
        }
        // An `all_keys` table outside `full_refresh` still maps the reads of it
        // that can be traced.
        MappingKind::Mapped | MappingKind::AllKeys if mapping.sql.is_some() => {
            let Some(keys_sql) = rendered(entity, mapping)? else {
                refresh_all(entity, "a relation its mapping reads is gone");
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
            let images_sql = (1..=images.len())
                .map(|i| format!("SELECT (${i}).*"))
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            let delta = match computed_columns(entity, root)? {
                Some(columns) => format!("SELECT {columns} FROM ({images_sql}) r"),
                None => images_sql,
            };
            let key_type = &meta.key_type;
            let sql = format!(
                "WITH {DELTA} AS ({delta}) \
                 SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
                key_sql_type(key_type)
            );
            let _owner = crate::owner::AsOwner::of_entity(entity)?;
            // As for a statement's rows (map_statement), on the partition tree's root.
            crate::concurrency::lock_changed_rows(mapping, |column| {
                Spi::connect_mut(|client| {
                    texts(client, None, &changed_values(&delta, column), &images)
                })
                .map_err(|e| crate::utils::spi::error("the values a row changed", &e))
            })?;
            // Read-write: a fresh snapshot, which sees what the locks waited for.
            let keys = Spi::connect_mut(|client| {
                let mut keys = Vec::new();
                for row in crate::utils::spi::kept_update(client, &sql, &images)? {
                    keys.extend(key_of(&row, key_type)?);
                }
                crosscheck_keys(&sql, &images, &keys)?;
                Ok::<_, TViewError>(keys)
            })
            .map_err(|e| crate::utils::spi::error(&sql, &e))?;
            if !keys.is_empty() {
                crate::queue::enqueue_refresh_bulk(entity, keys);
            }
            Ok(true)
        }
        MappingKind::AllKeys => Ok(true),
        _ => Ok(false),
    }
}
