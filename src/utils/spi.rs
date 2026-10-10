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
