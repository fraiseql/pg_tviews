//! `pg_tviews_resync`: refresh what given rows of a base table reach, as an UPDATE
//! of them would, without writing them (#280).
//!
//! The rows' current images stand for the changed rows: each TVIEW reading the
//! table maps them to its keys through the query a delta trigger runs over a
//! statement's rows ([`DELTA`]), as the TVIEW's owner, and one flush recomputes
//! those keys (and the parents of rows that changed). A key whose row is gone is
//! enqueued as it is for a TVIEW keyed by the table's key, whose recompute then
//! deletes the row; a TVIEW mapping through the row's links cannot find the rows
//! a deleted or moved row left, which `pg_tviews_refresh` rebuilds.

use super::render::key_of;
use crate::catalog::{KeyType, TviewMeta};
use crate::config::UncascadedPolicy;
use crate::error::{TViewError, TViewResult};
use crate::lineage::{DELTA, MappingKind};
use crate::queue::key::KeyValue;
use pgrx::pg_sys::{self, Oid};

/// What a resync did to one TVIEW (its table), one reading the source or one
/// that changed through another: the rows it changed, or `None` when it was
/// rebuilt whole.
pub struct Resynced {
    pub table: Oid,
    pub changed: Option<i64>,
}

/// `schema.name` of relation `relid`, each quoted as needed.
///
/// # Errors
/// The relation does not exist.
pub fn qualified(relid: Oid) -> TViewResult<String> {
    let (schema, name) = crate::ddl::relation_name(relid)?;
    Ok(format!(
        "{}.{}",
        crate::utils::ident::quote_if_needed(&schema),
        crate::utils::ident::quote_if_needed(&name)
    ))
}

/// Recompute the TVIEW rows that the rows of `source` keyed `keys` (its primary
/// key) reach. The caller needs `UPDATE` on `source`.
///
/// # Errors
/// `PermissionDenied` without `UPDATE`; `InvalidInput` for a table without a
/// single-column integer primary key; a failed lookup or refresh.
pub fn resync(source: Oid, keys: &[i64]) -> TViewResult<Vec<Resynced>> {
    let name = qualified(source)?;
    if !may_update(source) {
        return Err(TViewError::PermissionDenied {
            reason: format!("permission denied to resync {name}: it needs UPDATE on the table"),
        });
    }
    let pk = primary_key(source, &name)?;
    // What is queued already is flushed first: the counts are the resync's own.
    crate::flush::flush_refresh_queue()?;
    let readers = crate::catalog::table_readers(source)?;
    let mut entities: Vec<String> = readers
        .row()
        .chain(readers.delta())
        .map(str::to_string)
        .collect();
    entities.sort();
    entities.dedup();
    let rows = format!(
        "SELECT s.* FROM {{r:{}}} s WHERE s.{{c:{}:{}}} OPERATOR(pg_catalog.=) ANY ($1)",
        source.to_u32(),
        source.to_u32(),
        pk.attnum
    );
    let rows = crate::lineage::render_template(&rows)
        .map_err(|e| crate::utils::spi::error("the resynced rows", &e))?
        .ok_or_else(|| TViewError::CatalogError {
            operation: format!("Resync {name}"),
            pg_error: "the table or its key column is gone".to_string(),
        })?;
    let mut done = Vec::new();
    for entity in entities {
        let Some(meta) = TviewMeta::load_reaching(&entity, source)? else {
            continue;
        };
        let rebuilt = enqueue(&meta, source, &pk, &rows, keys)?;
        done.push((entity, meta.tview_oid, rebuilt));
    }
    // Counted from this flush's changed rows, none when nothing was queued.
    crate::queue::log::begin_flush();
    crate::flush::flush_refresh_queue()?;
    let changed = crate::queue::log::changed_in_flush_by_entity();
    // The TVIEWs embedding those changed through them.
    let mut further: Vec<&String> = changed
        .keys()
        .filter(|e| !done.iter().any(|(d, _, _)| d == *e))
        .collect();
    further.sort();
    for entity in further {
        if let Some(meta) = TviewMeta::load_by_entity(entity)? {
            done.push((entity.clone(), meta.tview_oid, false));
        }
    }
    Ok(done
        .into_iter()
        .map(|(entity, table, rebuilt)| Resynced {
            changed: (!rebuilt).then(|| changed.get(&entity).copied().unwrap_or(0)),
            table,
        })
        .collect())
}

/// The source table's primary key column: its number and name.
struct PrimaryKey {
    attnum: i16,
    name: String,
}

/// Enqueue `meta`'s keys that the rows `rows` (of `source`, keyed `keys`) reach.
/// Returns true when the whole TVIEW was enqueued.
fn enqueue(
    meta: &TviewMeta,
    source: Oid,
    pk: &PrimaryKey,
    rows: &str,
    keys: &[i64],
) -> TViewResult<bool> {
    let entity = meta.entity_name.as_str();
    let Some(mapping) = meta.key_mapping(source, None) else {
        crate::concurrency::enqueue_refresh_all(entity);
        return Ok(true);
    };
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    let keys_sql = match mapping.kind {
        MappingKind::AllKeys if full_refresh => {
            crate::concurrency::enqueue_refresh_all(entity);
            return Ok(true);
        }
        MappingKind::Local | MappingKind::Mapped | MappingKind::AllKeys => {
            match super::mapping_query(meta, source)? {
                Some(sql) => sql,
                None => return Ok(false),
            }
        }
        MappingKind::Propagated => return Ok(false),
    };
    let key_type = meta.key_type.clone();
    let mut found = {
        let _owner = crate::owner::AsOwner::of_entity(entity)?;
        lookup(&key_type, rows, &keys_sql, keys)?
    };
    // A row gone is still a key of the TVIEW keyed by the table's key.
    if mapping.kind == MappingKind::Local && mapping.column.as_deref() == Some(pk.name.as_str()) {
        found.extend(keys.iter().map(|&k| match &key_type {
            KeyType::Int => KeyValue::Int(k),
            KeyType::Text(_) => KeyValue::Text(k.to_string()),
        }));
    }
    found.sort();
    found.dedup();
    if !found.is_empty() {
        crate::queue::enqueue_refresh_bulk(entity, found);
    }
    Ok(false)
}

/// The keys the mapping query `keys_sql` returns over the rows `rows`.
fn lookup(
    key_type: &KeyType,
    rows: &str,
    keys_sql: &str,
    keys: &[i64],
) -> TViewResult<Vec<KeyValue>> {
    let sql = format!(
        "WITH {DELTA} AS ({rows}) \
         SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
        super::render::key_sql_type(key_type)
    );
    crate::utils::spi::connect_mut(|client| {
        let mut out = Vec::new();
        let args = [crate::utils::spi::int8_array(keys.to_vec())];
        for row in client
            .update(&sql, None, &args)
            .map_err(|e| crate::utils::spi::error(&sql, &e))?
        {
            out.extend(key_of(&row, key_type)?);
        }
        Ok(out)
    })
}

/// Whether the current user holds `UPDATE` on the table `relid`.
fn may_update(relid: Oid) -> bool {
    // SAFETY: an ACL check by OID for the current user.
    unsafe {
        pg_sys::pg_class_aclcheck(
            relid,
            pg_sys::GetUserId(),
            pg_sys::AclMode::from(pg_sys::ACL_UPDATE),
        ) == pg_sys::AclResult::ACLCHECK_OK
    }
}

/// The single-column integer primary key of `source`.
fn primary_key(source: Oid, name: &str) -> TViewResult<PrimaryKey> {
    let found = crate::utils::spi::catalog_rows(
        "SELECT a.attnum, a.attname::pg_catalog.text FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0] \
         JOIN pg_catalog.pg_type t ON t.oid = a.atttypid \
         WHERE i.indrelid = $1 AND i.indisprimary AND i.indnatts = 1 \
           AND CASE t.typtype WHEN 'd' THEN t.typbasetype ELSE t.oid END \
               IN ('pg_catalog.int2'::pg_catalog.regtype, 'pg_catalog.int4'::pg_catalog.regtype, \
                   'pg_catalog.int8'::pg_catalog.regtype)",
        &[crate::utils::spi::oid(source)],
        |row| Ok((row.get::<i16>(1)?, row.get::<String>(2)?)),
    )?;
    match found.into_iter().next() {
        Some((Some(attnum), Some(name))) => Ok(PrimaryKey { attnum, name }),
        _ => Err(TViewError::InvalidInput {
            parameter: "source".to_string(),
            reason: format!("{name} has no single-column integer primary key to resync by"),
        }),
    }
}
