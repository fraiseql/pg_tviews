//! One way to run SQL from `pg_tviews`: every call maps its error once, naming the
//! query, and every argument says its SQL type in its name, checked by the compiler
//! against the Rust value.

use crate::error::{TViewError, TViewResult};
use pgrx::datum::{DatumWithOid, FromDatum, IntoDatum};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use pgrx::spi::SpiHeapTupleData;

/// Values bound as `text` (`None` is NULL).
pub trait Text: IntoDatum {}
impl Text for &str {}
impl Text for String {}
impl Text for &String {}
impl<T: Text> Text for Option<T> {}

/// Values bound as `oid` (`None` is NULL).
pub trait OidValue: IntoDatum {}
impl OidValue for Oid {}
impl OidValue for Option<Oid> {}

/// Values bound as `jsonb` (`None` is NULL).
pub trait JsonbValue: IntoDatum {}
impl JsonbValue for pgrx::JsonB {}
impl JsonbValue for Option<pgrx::JsonB> {}

/// A `text` argument.
pub fn text<'a, T: Text>(value: T) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `text[]` argument.
pub fn text_array<'a, T: Text>(values: Vec<T>) -> DatumWithOid<'a> {
    DatumWithOid::from(values)
}

/// A `text[]` argument from a slice.
pub fn text_array_of<'a>(values: &[String]) -> DatumWithOid<'a> {
    DatumWithOid::from(values.to_vec())
}

/// An `oid` argument.
pub fn oid<'a, T: OidValue>(value: T) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `regclass` argument.
pub fn regclass<'a>(relid: Oid) -> DatumWithOid<'a> {
    // SAFETY: a regclass datum is an OID datum: the value is the same, only the
    // type it is labelled with differs.
    unsafe { DatumWithOid::new(relid, pgrx::pg_sys::REGCLASSOID) }
}

/// An `oid[]` argument.
pub fn oid_array<'a>(values: Vec<Oid>) -> DatumWithOid<'a> {
    DatumWithOid::from(values)
}

/// A `bigint` argument.
pub fn int8<'a>(value: i64) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `bigint[]` argument.
pub fn int8_array<'a>(values: Vec<i64>) -> DatumWithOid<'a> {
    DatumWithOid::from(values)
}

/// An `integer` argument.
pub fn int4<'a>(value: i32) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `smallint` argument.
pub fn int2<'a>(value: i16) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `smallint[]` argument.
pub fn int2_array<'a>(values: Vec<i16>) -> DatumWithOid<'a> {
    DatumWithOid::from(values)
}

/// A `boolean` argument.
pub fn boolean<'a>(value: bool) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// A `jsonb` argument.
pub fn jsonb<'a, T: JsonbValue>(value: T) -> DatumWithOid<'a> {
    DatumWithOid::from(value)
}

/// The error of `query`.
pub fn error(query: &str, e: &impl std::fmt::Display) -> TViewError {
    TViewError::SpiError {
        query: query.to_string(),
        error: e.to_string(),
    }
}

/// A failed catalog read or write, described by what it was for.
pub fn catalog_error(operation: &str, e: &impl std::fmt::Display) -> TViewError {
    TViewError::CatalogError {
        operation: operation.to_string(),
        pg_error: e.to_string(),
    }
}

/// The first column of every row of `sql`, skipping NULLs.
///
/// # Errors
/// Returns the query's error.
pub fn strings(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<String>> {
    Ok(rows(sql, args, |row| Ok(row.get::<String>(1)?))?
        .into_iter()
        .flatten()
        .collect())
}

/// The first column of every row of `sql`, skipping NULLs.
///
/// # Errors
/// Returns the query's error.
pub fn oids(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<Oid>> {
    Ok(rows(sql, args, |row| Ok(row.get::<Oid>(1)?))?
        .into_iter()
        .flatten()
        .collect())
}

/// The first column of the first row of `sql`, `None` without one.
///
/// # Errors
/// Returns the query's error.
pub fn one<T: FromDatum + IntoDatum>(
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> TViewResult<Option<T>> {
    match Spi::get_one_with_args::<T>(sql, args) {
        Ok(value) => Ok(value),
        Err(pgrx::spi::Error::InvalidPosition) => Ok(None),
        Err(e) => Err(error(sql, &e)),
    }
}

/// Run `sql` for its effect.
///
/// # Errors
/// Returns the query's error.
pub fn run(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<()> {
    Spi::run_with_args(sql, args).map_err(|e| error(sql, &e))
}

/// Each row of `sql`, read by `read`.
///
/// # Errors
/// Returns the query's error, or `read`'s.
pub fn rows<T>(
    sql: &str,
    args: &[DatumWithOid<'_>],
    mut read: impl FnMut(&SpiHeapTupleData<'_>) -> TViewResult<T>,
) -> TViewResult<Vec<T>> {
    Spi::connect(|client| {
        let mut out = Vec::new();
        for row in client.select(sql, None, args).map_err(|e| error(sql, &e))? {
            out.push(read(&row)?);
        }
        Ok(out)
    })
}

/// Run a DDL statement (atomically, unless the caller may commit).
///
/// # Errors
/// Returns the statement's error.
pub fn run_ddl(sql: &str) -> TViewResult<()> {
    super::spi_run_ddl(sql).map_err(|e| error(sql, &e))
}

/// The rows of the read-only query `sql` as text (`None` for NULL), under the
/// latest snapshot instead of the transaction's: what a REPEATABLE READ
/// transaction can't see yet, as PostgreSQL's foreign-key checks read it. The
/// caller is inside an SPI connection (one that sees a trigger's transition
/// tables, if the query reads them).
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows.
pub fn latest_rows_connected(
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> TViewResult<Vec<Vec<Option<String>>>> {
    let src = std::ffi::CString::new(sql).map_err(|e| error(sql, &e))?;
    let mut types: Vec<Oid> = args.iter().map(DatumWithOid::oid).collect();
    let mut values: Vec<pg_sys::Datum> = args
        .iter()
        .map(|a| {
            a.datum()
                .map_or_else(|| pg_sys::Datum::from(0), pgrx::datum::Datum::sans_lifetime)
        })
        .collect();
    let nulls: Vec<std::ffi::c_char> = args
        .iter()
        .map(|a| if a.datum().is_some() { b' ' } else { b'n' }.cast_signed())
        .collect();
    let nargs = i32::try_from(args.len()).map_err(|e| error(sql, &e))?;
    // SAFETY: the caller holds an SPI connection; every pointer passed lives
    // until the call returns, and the result is read before the next SPI call.
    unsafe {
        let plan = pg_sys::SPI_prepare(src.as_ptr(), nargs, types.as_mut_ptr());
        if plan.is_null() {
            return Err(error(
                sql,
                &format!("SPI_prepare failed ({})", { pg_sys::SPI_result }),
            ));
        }
        let rc = pg_sys::SPI_execute_snapshot(
            plan,
            values.as_mut_ptr(),
            nulls.as_ptr(),
            pg_sys::GetLatestSnapshot(),
            std::ptr::null_mut(),
            true,
            false,
            0,
        );
        if rc != pg_sys::SPI_OK_SELECT.cast_signed() {
            pg_sys::SPI_freeplan(plan);
            return Err(error(sql, &format!("SPI_execute_snapshot returned {rc}")));
        }
        let table = pg_sys::SPI_tuptable;
        let tupdesc = (*table).tupdesc;
        let mut rows = Vec::new();
        for i in 0..usize::try_from(pg_sys::SPI_processed).unwrap_or(0) {
            let tuple = *(*table).vals.add(i);
            let mut row = Vec::new();
            for column in 1..=(*tupdesc).natts {
                let value = pg_sys::SPI_getvalue(tuple, tupdesc, column);
                row.push(if value.is_null() {
                    None
                } else {
                    let text = std::ffi::CStr::from_ptr(value)
                        .to_string_lossy()
                        .into_owned();
                    pg_sys::pfree(value.cast());
                    Some(text)
                });
            }
            rows.push(row);
        }
        pg_sys::SPI_freetuptable(table);
        pg_sys::SPI_freeplan(plan);
        Ok(rows)
    }
}
