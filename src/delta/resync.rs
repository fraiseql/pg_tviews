//! `pg_tviews_resync`: refresh what given rows of a base table reach, as an UPDATE
//! of them would, without writing them (#280).
//!
//! The rows' current images stand for the changed rows, a synthetic UPDATE
//! delta: the caller reads them once, as itself, and each TVIEW reading the table
//! maps them to its keys through the query a delta trigger runs over a
//! statement's rows ([`DELTA`]), as the TVIEW's owner. No owner reads the table
//! itself. One flush recomputes those keys (and the parents of rows that
//! changed). A key whose row is gone is
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
/// key) reach. The caller needs `UPDATE` on `source`, and `SELECT`: it reads the
/// rows.
///
/// # Errors
/// `PermissionDenied` without `UPDATE` or `SELECT`, or under row security;
/// `InvalidInput` for a table without a single-column integer primary key; a
/// failed lookup or refresh.
pub fn resync(source: Oid, keys: &[i64]) -> TViewResult<Vec<Resynced>> {
    // SAFETY: a lock on a relation by OID, held until the transaction ends: no
    // other session renames or drops it meanwhile.
    unsafe { pg_sys::LockRelationOid(source, pg_sys::AccessShareLock.cast_signed()) };
    let name = qualified(source)?;
    if !may(source, pg_sys::ACL_UPDATE) {
        return Err(TViewError::PermissionDenied {
            reason: format!("permission denied to resync {name}: it needs UPDATE on the table"),
        });
    }
    if !may(source, pg_sys::ACL_SELECT) {
        return Err(TViewError::PermissionDenied {
            reason: format!("resync reads {name} as you: SELECT on it is required"),
        });
    }
    if crate::visibility::rls_active_for_caller(source)? {
        return Err(TViewError::PermissionDenied {
            reason: format!(
                "resync of {name}, a table with row security active for you, is not supported: \
                 ask its owner"
            ),
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
    let rows = read_rows(source, &pk, keys, &name)?;
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

/// The rows of `source` keyed `keys`, as a JSON array of their row images, read
/// as the caller under the caller's privileges.
fn read_rows(source: Oid, pk: &PrimaryKey, keys: &[i64], name: &str) -> TViewResult<pgrx::JsonB> {
    let sql = crate::lineage::render_template(&format!(
        "SELECT COALESCE(pg_catalog.jsonb_agg(pg_catalog.to_jsonb(s)), '[]') \
         FROM {{r:{}}} s WHERE s.{{c:{}:{}}} OPERATOR(pg_catalog.=) ANY ($1)",
        source.to_u32(),
        source.to_u32(),
        pk.attnum
    ))
    .map_err(|e| crate::utils::spi::error("the resynced rows", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: format!("Resync {name}"),
        pg_error: "the table or its key column is gone".to_string(),
    })?;
    let rows = crate::utils::spi::one::<pgrx::JsonB>(
        &sql,
        &[crate::utils::spi::int8_array(keys.to_vec())],
    )?;
    Ok(rows.unwrap_or_else(|| pgrx::JsonB(serde_json::Value::Array(Vec::new()))))
}

/// Enqueue `meta`'s keys that the rows `rows` (of `source`, keyed `keys`) reach.
/// Returns true when the whole TVIEW was enqueued.
fn enqueue(
    meta: &TviewMeta,
    source: Oid,
    pk: &PrimaryKey,
    rows: &pgrx::JsonB,
    keys: &[i64],
) -> TViewResult<bool> {
    let entity = meta.entity_name.as_str();
    let Some(mapping) = meta.key_mapping(source, None) else {
        crate::concurrency::enqueue_refresh_all(entity);
        return Ok(true);
    };
    let full_refresh = meta.policy_for(Oid::from(mapping.relid)) == UncascadedPolicy::FullRefresh;
    match mapping.kind {
        MappingKind::AllKeys if full_refresh => {
            crate::concurrency::enqueue_refresh_all(entity);
            return Ok(true);
        }
        MappingKind::Local | MappingKind::Mapped | MappingKind::AllKeys => {}
        MappingKind::Propagated => return Ok(false),
    }
    let key_type = meta.key_type.clone();
    // Rendered in the owner's frame, by OID, right before it runs.
    let found =
        crate::owner::AsOwner::run(crate::owner::Owner::of_table(meta.tview_oid)?, |frame| {
            frame.require_owner_of(meta.tview_oid)?;
            let Some(keys_sql) = super::mapping_query(meta, source)? else {
                return Ok(None);
            };
            lookup(&key_type, source, rows, &keys_sql).map(Some)
        })?;
    let Some(mut found) = found else {
        return Ok(false);
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

/// The keys the mapping query `keys_sql` returns over the rows `rows` of
/// `source`: their images made rows of its row type again, by column name.
fn lookup(
    key_type: &KeyType,
    source: Oid,
    rows: &pgrx::JsonB,
    keys_sql: &str,
) -> TViewResult<Vec<KeyValue>> {
    // SAFETY: a syscache lookup by OID; 0 for a relation that is gone.
    let row_type = unsafe { pg_sys::get_rel_type_id(source) };
    if row_type == pg_sys::InvalidOid {
        return Err(TViewError::CatalogError {
            operation: "Resync a table".to_string(),
            pg_error: format!("relation {} is gone", source.to_u32()),
        });
    }
    let sql = format!(
        "WITH {DELTA} AS (SELECT * FROM pg_catalog.jsonb_populate_recordset(NULL::{}, $1)) \
         SELECT DISTINCT k::{} FROM ({keys_sql}) s(k) WHERE k IS NOT NULL",
        crate::utils::qualified_type_name(row_type, -1),
        super::render::key_sql_type(key_type)
    );
    crate::utils::spi::connect_mut(|client| {
        let mut out = Vec::new();
        let args = [crate::utils::spi::jsonb(pgrx::JsonB(rows.0.clone()))];
        for row in client
            .update(&sql, None, &args)
            .map_err(|e| crate::utils::spi::error(&sql, &e))?
        {
            out.extend(key_of(&row, key_type)?);
        }
        Ok(out)
    })
}

/// Whether the current user holds `mode` (`UPDATE`, `SELECT`) on the table
/// `relid`, or on each of its columns.
fn may(relid: Oid, mode: u32) -> bool {
    let mode = pg_sys::AclMode::from(mode);
    // SAFETY: ACL checks by OID for the current user, outside any owner frame.
    unsafe {
        let user = pg_sys::GetUserId();
        pg_sys::pg_class_aclcheck(relid, user, mode) == pg_sys::AclResult::ACLCHECK_OK
            || pg_sys::pg_attribute_aclcheck_all(relid, user, mode, pg_sys::AclMaskHow::ACLMASK_ALL)
                == pg_sys::AclResult::ACLCHECK_OK
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
