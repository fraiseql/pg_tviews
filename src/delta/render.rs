//! The SQL a mapping runs over the changed rows: the delta relation, the
//! columns it reads (virtual generated columns computed), the pairing of each
//! updated row's two images, and the keys it returns.

use super::Event;
use crate::catalog::KeyType;
use crate::dependency::triggers::{NEW_TABLE, OLD_TABLE};
use crate::error::TViewResult;
use crate::queue::key::KeyValue;
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;

/// The changed rows of a statement: the new rows of an INSERT, the old rows of a
/// DELETE, both images of an UPDATE. An UPDATE row whose columns the TVIEW reads
/// (`attnums`) are unchanged, matched to its other image by primary key, is left
/// out of both. Virtual generated columns, NULL in transition tables, are computed.
pub(super) fn delta_sql(
    entity: &str,
    table_oid: Oid,
    event: Event,
    attnums: &[i16],
) -> TViewResult<String> {
    let key = (table_oid.to_u32(), event, attnums.to_vec());
    if let Some(sql) = crate::cache::DELTAS.with(|m| m.get(&key)) {
        return Ok(sql);
    }
    let (new, old) = match computed_columns(entity, table_oid)? {
        Some(columns) => (
            format!("(SELECT {columns} FROM {NEW_TABLE})"),
            format!("(SELECT {columns} FROM {OLD_TABLE})"),
        ),
        None => (NEW_TABLE.to_string(), OLD_TABLE.to_string()),
    };
    let sql = match event {
        Event::Insert => format!("SELECT * FROM {new}"),
        Event::Delete => format!("SELECT * FROM {old}"),
        Event::Update => match update_filter(table_oid, attnums)? {
            Some(same) => format!(
                "SELECT o.* FROM {old} o \
                   WHERE NOT EXISTS (SELECT 1 FROM {new} n WHERE {same}) \
                 UNION ALL \
                 SELECT n.* FROM {new} n \
                   WHERE NOT EXISTS (SELECT 1 FROM {old} o WHERE {same})"
            ),
            None => format!("SELECT * FROM {old} UNION ALL SELECT * FROM {new}"),
        },
    };
    crate::cache::DELTAS.with(|m| m.insert(key, sql.clone()));
    Ok(sql)
}

/// The select list of table `table_oid`'s rows with its virtual generated columns
/// computed from their expressions, in column order and under the column names;
/// `None` when it has none (always before PostgreSQL 18). The expressions are
/// written as `entity`'s owner sees them, so they resolve under its `search_path`.
pub(super) fn computed_columns(entity: &str, table_oid: Oid) -> TViewResult<Option<String>> {
    if let Some(columns) = crate::cache::COMPUTED.with(|m| m.get(&table_oid)) {
        return Ok(columns);
    }
    let _owner = crate::owner::AsOwner::of_entity(entity)?;
    let (columns, any_virtual) = crate::utils::spi::catalog_rows(
        "SELECT pg_catalog.string_agg( \
                          CASE WHEN a.attgenerated = 'v' \
                               THEN '(' || pg_catalog.pg_get_expr(ad.adbin, ad.adrelid) || ') AS ' \
                                    || pg_catalog.quote_ident(a.attname) \
                               ELSE pg_catalog.quote_ident(a.attname) END, \
                          ', ' ORDER BY a.attnum), \
                        pg_catalog.bool_or(a.attgenerated = 'v') \
                 FROM pg_catalog.pg_attribute a \
                 LEFT JOIN pg_catalog.pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
                 WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped",
        &[crate::utils::spi::oid(table_oid)],
        |row| Ok((row.get::<String>(1)?, row.get::<bool>(2)?)),
    )?
    .into_iter()
    .next()
    .unwrap_or_default();
    let columns = columns.filter(|_| any_virtual == Some(true));
    crate::cache::watch(&[table_oid]);
    crate::cache::COMPUTED.with(|m| m.insert(table_oid, columns.clone()));
    Ok(columns)
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
                .map(|c| format!("{alias}.{}", crate::utils::ident::quoted(c)))
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
pub(super) fn row_pairing(
    table_oid: Oid,
    attnums: &[i16],
) -> TViewResult<Option<crate::cache::RowPairing>> {
    if attnums.is_empty() {
        return Ok(None);
    }
    crate::cache::PAIRINGS.with(|m| {
        m.get_or_load((table_oid, attnums.to_vec()), || {
            crate::cache::watch(&[table_oid]);
            read_row_pairing(table_oid, attnums)
        })
    })
}

/// [`row_pairing`], read from the catalog.
fn read_row_pairing(table_oid: Oid, attnums: &[i16]) -> TViewResult<Option<(String, Vec<String>)>> {
    let keys = primary_key(table_oid)?;
    let mut ordered = attnums.to_vec();
    ordered.sort_unstable();
    ordered.dedup();
    // SAFETY: catalog lookups by OID and attribute number.
    let columns: Vec<String> = ordered
        .iter()
        .filter_map(|&attnum| unsafe {
            let name = pg_sys::get_attname(table_oid, attnum, true);
            (!name.is_null()).then(|| {
                std::ffi::CStr::from_ptr(name)
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect();
    if keys.is_empty()
        || keys.iter().any(|(_, catalog_eq)| !catalog_eq)
        || columns.len() != attnums.len()
    {
        return Ok(None);
    }
    let same_row = keys
        .iter()
        .map(|(k, _)| {
            let k = crate::utils::ident::quoted(k);
            format!("n.{k} OPERATOR(pg_catalog.=) o.{k}")
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    Ok(Some((same_row, columns)))
}

/// The primary key columns of `table_oid`, each with whether the equality operator
/// of its type is in `pg_catalog` (cached).
fn primary_key(table_oid: Oid) -> TViewResult<std::rc::Rc<[(String, bool)]>> {
    crate::cache::PRIMARY_KEYS.with(|m| {
        m.get_or_load(table_oid, || {
            crate::cache::watch(&[table_oid]);
            Ok(read_primary_key(table_oid)?.into())
        })
    })
}

/// [`primary_key`], read from the catalog.
fn read_primary_key(table_oid: Oid) -> TViewResult<Vec<(String, bool)>> {
    crate::utils::spi::catalog_rows(
        "SELECT a.attname::pg_catalog.text, \
                EXISTS (SELECT 1 FROM pg_catalog.pg_operator o \
                        WHERE o.oprname = '=' AND o.oprleft = a.atttypid \
                          AND o.oprright = a.atttypid \
                          AND o.oprnamespace = 'pg_catalog'::pg_catalog.regnamespace) \
         FROM pg_catalog.pg_index i \
         JOIN pg_catalog.pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY (i.indkey) \
         WHERE i.indrelid = $1 AND i.indisprimary ORDER BY a.attnum",
        &[crate::utils::spi::oid(table_oid)],
        |row| {
            Ok((
                row.get::<String>(1)?.unwrap_or_default(),
                row.get::<bool>(2)?.unwrap_or(false),
            ))
        },
    )
}

/// The type a mapping query casts its keys to: `int8` for an integer identity,
/// its canonical text otherwise.
pub(super) const fn key_sql_type(key_type: &KeyType) -> &'static str {
    match key_type {
        KeyType::Int => "pg_catalog.int8",
        KeyType::Text(_) => "pg_catalog.text",
    }
}

/// The key in the first column of `row`, cast by [`key_sql_type`].
pub(super) fn key_of(
    row: &spi::SpiHeapTupleData<'_>,
    key_type: &KeyType,
) -> crate::TViewResult<Option<KeyValue>> {
    Ok(match key_type {
        KeyType::Int => row.get::<i64>(1)?.map(KeyValue::Int),
        KeyType::Text(_) => row.get::<String>(1)?.map(KeyValue::Text),
    })
}
