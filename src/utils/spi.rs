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

/// A row of a query, its columns as text (`None` for NULL).
pub type TextRow = Vec<Option<String>>;

/// The rows of the read-only query `sql` as text, under the latest snapshot
/// instead of the transaction's: what a REPEATABLE READ transaction can't see
/// yet, as PostgreSQL's foreign-key checks read it. From its kept plan when
/// `keep` (never for a query over a trigger's transition tables, which are the
/// statement's own). The caller is inside an SPI connection (one that sees the
/// transition tables, if the query reads them).
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows.
pub fn latest_rows_connected(
    sql: &str,
    args: &[DatumWithOid<'_>],
    keep: bool,
) -> TViewResult<Vec<TextRow>> {
    let (mut types, mut values, nulls) = bind(args);
    let kept = if keep {
        Some(kept_plan(sql, &mut types)?)
    } else {
        None
    };
    // SAFETY: inside an SPI connection; the plan (kept, or prepared here and
    // freed below) and the arguments, of the types it was prepared for, live
    // until the call returns.
    unsafe {
        let plan = if let Some(plan) = &kept {
            plan.0.as_ptr()
        } else {
            let src = std::ffi::CString::new(sql).map_err(|e| error(sql, &e))?;
            let nargs = i32::try_from(types.len()).map_err(|e| error(sql, &e))?;
            let plan = pg_sys::SPI_prepare(src.as_ptr(), nargs, types.as_mut_ptr());
            if plan.is_null() {
                return Err(error(
                    sql,
                    &format!("SPI_prepare failed ({})", { pg_sys::SPI_result }),
                ));
            }
            plan
        };
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
        let rows = tuptable_rows(sql, rc);
        if kept.is_none() {
            pg_sys::SPI_freeplan(plan);
        }
        rows
    }
}

/// A plan prepared once per backend and kept across transactions, always run
/// as a generic plan: no planning per execution. PostgreSQL revalidates it when
/// what it reads changes.
pub struct KeptPlan(std::ptr::NonNull<pg_sys::_SPI_plan>);

impl Drop for KeptPlan {
    fn drop(&mut self) {
        // SAFETY: the plan was kept by SPI_keepplan and is freed once, here.
        unsafe {
            pg_sys::SPI_freeplan(self.0.as_ptr());
        }
    }
}

/// The rows of `sql` as text, run read-write (a fresh snapshot under READ
/// COMMITTED) from its kept plan, prepared on first use.
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows.
pub fn kept_rows(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<TextRow>> {
    let (mut types, mut values, nulls) = bind(args);
    Spi::connect(|_| {
        let plan = kept_plan(sql, &mut types)?;
        // SAFETY: inside an SPI connection, with a valid kept plan and arguments
        // of the types it was prepared for, living until the call returns.
        unsafe {
            let rc = pg_sys::SPI_execute_plan(
                plan.0.as_ptr(),
                values.as_mut_ptr(),
                nulls.as_ptr(),
                false,
                0,
            );
            tuptable_rows(sql, rc)
        }
    })
}

/// The kept generic plan of `sql` for arguments of `types`, prepared on first
/// use. Inside an SPI connection.
fn kept_plan(sql: &str, types: &mut [Oid]) -> TViewResult<std::rc::Rc<KeptPlan>> {
    if let Some(plan) = crate::cache::PLANS.with(|m| m.get(&sql.to_string())) {
        return Ok(plan);
    }
    let src = std::ffi::CString::new(sql).map_err(|e| error(sql, &e))?;
    let nargs = i32::try_from(types.len()).map_err(|e| error(sql, &e))?;
    // SAFETY: inside an SPI connection; the plan is kept before the connection
    // ends, and owned by the cache from then on.
    let plan = unsafe {
        let plan = pg_sys::SPI_prepare_cursor(
            src.as_ptr(),
            nargs,
            types.as_mut_ptr(),
            pg_sys::CURSOR_OPT_GENERIC_PLAN.cast_signed(),
        );
        let plan = std::ptr::NonNull::new(plan).ok_or_else(|| {
            error(
                sql,
                &format!("SPI_prepare failed ({})", { pg_sys::SPI_result }),
            )
        })?;
        pg_sys::SPI_keepplan(plan.as_ptr());
        std::rc::Rc::new(KeptPlan(plan))
    };
    crate::cache::PLANS.with(|m| m.insert(sql.to_string(), plan.clone()));
    Ok(plan)
}

/// The types, values and null flags of `args`, as SPI takes them.
fn bind(args: &[DatumWithOid<'_>]) -> (Vec<Oid>, Vec<pg_sys::Datum>, Vec<std::ffi::c_char>) {
    let types = args.iter().map(DatumWithOid::oid).collect();
    let values = args
        .iter()
        .map(|a| {
            a.datum()
                .map_or_else(|| pg_sys::Datum::from(0), pgrx::datum::Datum::sans_lifetime)
        })
        .collect();
    let nulls = args
        .iter()
        .map(|a| if a.datum().is_some() { b' ' } else { b'n' }.cast_signed())
        .collect();
    (types, values, nulls)
}

/// The rows of the result an SPI call returned with `rc`, as text.
///
/// # Safety
/// The SPI call that returned `rc` was the last one, in the current connection.
unsafe fn tuptable_rows(sql: &str, rc: i32) -> TViewResult<Vec<TextRow>> {
    if rc != pg_sys::SPI_OK_SELECT.cast_signed() {
        return Err(error(sql, &format!("SPI returned {rc}")));
    }
    // SAFETY: per the contract, SPI_tuptable holds the last result.
    unsafe {
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
        Ok(rows)
    }
}
