//! One way to run SQL from `pg_tviews`: every call maps its error once, naming the
//! query, and every argument says its SQL type in its name, checked by the compiler
//! against the Rust value.

use crate::error::{TViewError, TViewResult};
use pgrx::datum::{DatumWithOid, FromDatum, IntoDatum};
use pgrx::pg_sys::Oid;
use pgrx::prelude::*;
use pgrx::spi::{SpiClient, SpiHeapTupleData, SpiTupleTable};

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

/// The `bigint` keys a statement looks rows up by, in a column that may hold
/// each of them many times (`fk_<entity>`). How many there are decides the
/// statement's shape and plan: a generic plan can't see an array, and assumes
/// ten keys each matching the column's average number of rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupKeys {
    /// Compared with `=`, from a generic plan: it estimates the rows of one
    /// value, as a plan made for the key would, without planning again.
    One(i64),
    /// An array, planned at each execution for its count.
    Many(Vec<i64>),
}

impl LookupKeys {
    /// The lookup of `keys`; `None` without any.
    #[must_use]
    pub fn of(mut keys: Vec<i64>) -> Option<Self> {
        match keys.len() {
            0 => None,
            1 => keys.pop().map(Self::One),
            _ => Some(Self::Many(keys)),
        }
    }

    /// `column` holds one of the keys, bound as parameter `$param`.
    #[must_use]
    pub fn predicate(&self, column: &str, param: usize) -> String {
        match self {
            Self::One(_) => format!("{column} = ${param}::pg_catalog.int8"),
            Self::Many(_) => format!("{column} = ANY (${param}::pg_catalog.int8[])"),
        }
    }

    /// The keys, as the parameter [`Self::predicate`] reads.
    #[must_use]
    pub fn arg<'a>(&self) -> DatumWithOid<'a> {
        match self {
            Self::One(key) => int8(*key),
            Self::Many(keys) => int8_array(keys.clone()),
        }
    }

    /// How a statement looking up these keys is planned.
    #[must_use]
    pub const fn plan(&self) -> Plan {
        match self {
            Self::One(_) => Plan::Generic,
            Self::Many(_) => Plan::PerExecution,
        }
    }
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

/// The rows of the table `table_oid`.
///
/// # Errors
/// Returns the query's error.
pub fn count_rows(table_oid: Oid) -> TViewResult<u64> {
    let sql = format!(
        "SELECT pg_catalog.count(*) FROM {}",
        crate::utils::qualified_relname_from_oid(table_oid)?
    );
    let count = Spi::get_one::<i64>(&sql).map_err(|e| error(&sql, &e))?;
    Ok(count.unwrap_or(0).unsigned_abs())
}

/// Run a data-modifying statement without RETURNING; the rows it processed.
///
/// # Errors
/// Returns the statement's error.
pub fn run_counted(sql: &str) -> TViewResult<u64> {
    Spi::connect_mut(|client| client.update(sql, None, &[]).map(|done| done.len() as u64))
        .map_err(|e| error(sql, &e))
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

/// The statement `sql` kept for the backend: prepared on first use for the
/// types of `args` (allowed to change data when `mutating`), then run from its
/// plan. `PostgreSQL` picks custom or generic plans for it, and plans it again when
/// what it reads changes; a changed TVIEW changes the SQL `pg_tviews` generates.
/// Inside `client`'s connection. A statement over a trigger's transition tables
/// names them alike whatever the table: it is kept per table (`over`), as
/// PL/pgSQL keeps a trigger function's plans per relation, and dropped with the
/// other caches when the table changes.
fn statement(
    client: &SpiClient<'_>,
    sql: &str,
    args: &[DatumWithOid<'_>],
    mutating: bool,
    over: Option<Oid>,
) -> pgrx::spi::Result<std::rc::Rc<pgrx::spi::OwnedPreparedStatement>> {
    let types: Vec<Oid> = args.iter().map(DatumWithOid::oid).collect();
    let key = (sql.to_string(), types, mutating, over);
    if let Some(kept) = crate::cache::STATEMENTS.with(|m| m.get(&key)) {
        return Ok(kept);
    }
    let pg_types: Vec<pgrx::PgOid> = key.1.iter().map(|&t| pgrx::PgOid::from(t)).collect();
    let prepared = if mutating {
        client.prepare_mut(sql, &pg_types)?
    } else {
        client.prepare(sql, &pg_types)?
    };
    let kept = std::rc::Rc::new(prepared.keep());
    crate::metrics::metrics_api::record_plan_prepared();
    if let Some(table) = over {
        crate::cache::watch(&[table]);
    }
    crate::cache::STATEMENTS.with(|m| m.insert(key, kept.clone()));
    Ok(kept)
}

/// Run the read-only query `sql` from its kept plan ([`statement`]).
///
/// # Errors
/// The query's error.
pub fn kept_select<'conn>(
    client: &SpiClient<'conn>,
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> pgrx::spi::Result<SpiTupleTable<'conn>> {
    let kept = statement(client, sql, args, false, None)?;
    client.select(&*kept, None, args)
}

/// Run `sql`, which may change data, from its kept plan ([`statement`]).
///
/// # Errors
/// The statement's error.
pub fn kept_update<'conn>(
    client: &mut SpiClient<'conn>,
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> pgrx::spi::Result<SpiTupleTable<'conn>> {
    let kept = statement(client, sql, args, true, None)?;
    client.update(&*kept, None, args)
}

/// Run `sql`, a statement over the transition tables of `table`'s trigger, from
/// its kept plan ([`statement`]).
///
/// # Errors
/// The statement's error.
pub fn kept_update_over<'conn>(
    client: &mut SpiClient<'conn>,
    table: Oid,
    sql: &str,
    args: &[DatumWithOid<'_>],
) -> pgrx::spi::Result<SpiTupleTable<'conn>> {
    let kept = statement(client, sql, args, true, Some(table))?;
    client.update(&*kept, None, args)
}

/// A row of a query, its columns as text (`None` for NULL).
pub type TextRow = Vec<Option<String>>;

/// The rows of the read-only query `sql` as text, under the latest snapshot
/// instead of the transaction's: what a REPEATABLE READ transaction can't see
/// yet, as PostgreSQL's foreign-key checks read it. From its plan kept with
/// `keep` when there is one (never for a query over a trigger's transition
/// tables, which are the statement's own). The caller is inside an SPI
/// connection (one that sees the transition tables, if the query reads them).
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows.
pub fn latest_rows_connected(
    sql: &str,
    args: &[DatumWithOid<'_>],
    keep: Option<Plan>,
) -> TViewResult<Vec<TextRow>> {
    let (mut types, mut values, nulls) = bind(args);
    let kept = match keep {
        Some(plan) => Some(kept_plan(sql, &mut types, plan)?),
        None => None,
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

/// How a kept statement is planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plan {
    /// One plan, made without the arguments and never again: for a statement
    /// whose best plan doesn't depend on them, as a lookup by a unique key (a
    /// primary key, a TVIEW's identity, a catalog key) or by one key compared
    /// with `=` ([`LookupKeys`]).
    Generic,
    /// Planned at each execution for its arguments, its parse and analysis
    /// kept: for a lookup by an array of keys of a column that isn't unique,
    /// whose generic plan would assume ten keys of an average value and may scan
    /// the whole table for a few rows.
    PerExecution,
}

impl Plan {
    /// The plan of a statement holding lookups planned `self` and `other`.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Generic, Self::Generic) => Self::Generic,
            _ => Self::PerExecution,
        }
    }

    /// The cursor option `SPI_prepare_cursor` takes for this plan.
    const fn cursor_option(self) -> u32 {
        match self {
            Self::Generic => pg_sys::CURSOR_OPT_GENERIC_PLAN,
            Self::PerExecution => pg_sys::CURSOR_OPT_CUSTOM_PLAN,
        }
    }
}

/// A statement prepared once per backend and kept across transactions, planned
/// as its [`Plan`] says. PostgreSQL revalidates it when what it reads changes.
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
/// COMMITTED) from its statement kept with `plan`, prepared on first use. A
/// [`Plan::Generic`] statement is never planned again, where `PostgreSQL`'s
/// own choice would plan it at every execution whenever a custom plan looks
/// cheaper.
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows.
pub fn kept_rows(sql: &str, args: &[DatumWithOid<'_>], plan: Plan) -> TViewResult<Vec<TextRow>> {
    kept_rows_as(sql, args, plan, |row| Ok(row.texts()))
}

/// The rows of `sql`, run as [`kept_rows`] runs it, each read by `read` with
/// its columns typed ([`KeptRow::get`]).
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows, or
/// what `read` returns.
pub fn kept_rows_as<R>(
    sql: &str,
    args: &[DatumWithOid<'_>],
    plan: Plan,
    read: impl FnMut(&KeptRow<'_>) -> TViewResult<R>,
) -> TViewResult<Vec<R>> {
    let (mut types, mut values, nulls) = bind(args);
    Spi::connect(|_| {
        let plan = kept_plan(sql, &mut types, plan)?;
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
            tuptable_map(sql, rc, read)
        }
    })
}

/// The rows of the catalog query `sql`, each read by `read` ([`kept_rows_as`]),
/// counted as a catalog lookup. A catalog read is a lookup by key: its plan is
/// made once per backend and outlives the caches, whose invalidations are what
/// `PostgreSQL` revalidates it for.
///
/// # Errors
/// Returns an error if the query can't be prepared or doesn't return rows, or
/// what `read` returns.
pub fn catalog_rows<R>(
    sql: &str,
    args: &[DatumWithOid<'_>],
    read: impl FnMut(&KeptRow<'_>) -> TViewResult<R>,
) -> TViewResult<Vec<R>> {
    crate::metrics::metrics_api::record_catalog_lookup();
    kept_rows_as(sql, args, Plan::Generic, read)
}

/// The rows of the catalog query `sql` as text ([`catalog_rows`]).
///
/// # Errors
/// As [`catalog_rows`].
pub fn catalog_text_rows(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Vec<TextRow>> {
    catalog_rows(sql, args, |row| Ok(row.texts()))
}

/// The first column of the first row of the catalog query `sql`
/// ([`catalog_rows`]), `None` without one.
///
/// # Errors
/// As [`catalog_rows`].
pub fn catalog_one<T: OwnedColumn>(sql: &str, args: &[DatumWithOid<'_>]) -> TViewResult<Option<T>> {
    Ok(catalog_rows(sql, args, |row| row.get::<T>(1))?
        .into_iter()
        .next()
        .flatten())
}

/// `sql` for arguments of `types`, kept with `mode`, prepared on first use.
/// Inside an SPI connection.
fn kept_plan(sql: &str, types: &mut [Oid], mode: Plan) -> TViewResult<std::rc::Rc<KeptPlan>> {
    let key = (mode, sql.to_string());
    if let Some(plan) = crate::cache::PLANS.with(|m| m.get(&key)) {
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
            mode.cursor_option().cast_signed(),
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
    crate::metrics::metrics_api::record_plan_prepared();
    crate::cache::PLANS.with(|m| m.insert(key, plan.clone()));
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

/// A row of the last SPI result, valid while the function reading it runs.
pub struct KeptRow<'a> {
    sql: &'a str,
    tuple: pg_sys::HeapTuple,
    tupdesc: pg_sys::TupleDesc,
}

/// The types a [`KeptRow`] column is read as: owned values, copied out of the
/// row before the result is freed (a `&str` would point into it).
pub trait OwnedColumn: FromDatum + IntoDatum + sealed::Sealed {}
impl OwnedColumn for i64 {}
impl OwnedColumn for i32 {}
impl OwnedColumn for i16 {}
impl OwnedColumn for bool {}
impl OwnedColumn for Oid {}
impl OwnedColumn for String {}
impl OwnedColumn for pgrx::JsonB {}
impl OwnedColumn for Vec<Oid> {}
impl OwnedColumn for Vec<Option<Oid>> {}
impl OwnedColumn for Vec<String> {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for i64 {}
    impl Sealed for i32 {}
    impl Sealed for i16 {}
    impl Sealed for bool {}
    impl Sealed for super::Oid {}
    impl Sealed for String {}
    impl Sealed for pgrx::JsonB {}
    impl Sealed for Vec<super::Oid> {}
    impl Sealed for Vec<Option<super::Oid>> {}
    impl Sealed for Vec<String> {}
}

impl KeptRow<'_> {
    /// Column `column` (from 1) as `T`, `None` for NULL.
    ///
    /// # Errors
    /// The column is not of `T`'s SQL type.
    pub fn get<T: OwnedColumn>(&self, column: i32) -> TViewResult<Option<T>> {
        // SAFETY: the tuple and its descriptor belong to the last SPI result,
        // which `tuptable_map` frees only after the reading function returned.
        unsafe {
            let mut is_null = false;
            let datum = pg_sys::SPI_getbinval(self.tuple, self.tupdesc, column, &raw mut is_null);
            let type_oid = pg_sys::SPI_gettypeid(self.tupdesc, column);
            T::try_from_datum(datum, is_null, type_oid).map_err(|e| error(self.sql, &e))
        }
    }

    /// Column `name` as `T`, `None` for NULL.
    ///
    /// # Errors
    /// The row has no such column, or it is not of `T`'s SQL type.
    pub fn named<T: OwnedColumn>(&self, name: &str) -> TViewResult<Option<T>> {
        let c_name = std::ffi::CString::new(name).map_err(|e| error(self.sql, &e))?;
        // SAFETY: as in `get`; the name is a NUL-terminated string.
        let column = unsafe { pg_sys::SPI_fnumber(self.tupdesc, c_name.as_ptr()) };
        if column <= 0 {
            return Err(error(self.sql, &format!("no column {name}")));
        }
        self.get(column)
    }

    /// Every column as text (`None` for NULL).
    fn texts(&self) -> TextRow {
        // SAFETY: as in `get`.
        unsafe {
            (1..=(*self.tupdesc).natts)
                .map(|column| {
                    let value = pg_sys::SPI_getvalue(self.tuple, self.tupdesc, column);
                    if value.is_null() {
                        None
                    } else {
                        let text = std::ffi::CStr::from_ptr(value)
                            .to_string_lossy()
                            .into_owned();
                        pg_sys::pfree(value.cast());
                        Some(text)
                    }
                })
                .collect()
        }
    }
}

/// The rows of the result an SPI call returned with `rc`, as text.
///
/// # Safety
/// The SPI call that returned `rc` was the last one, in the current connection.
unsafe fn tuptable_rows(sql: &str, rc: i32) -> TViewResult<Vec<TextRow>> {
    // SAFETY: the caller's contract.
    unsafe { tuptable_map(sql, rc, |row| Ok(row.texts())) }
}

/// The rows of the result an SPI call returned with `rc`, each read by `read`.
///
/// # Safety
/// The SPI call that returned `rc` was the last one, in the current connection.
unsafe fn tuptable_map<R>(
    sql: &str,
    rc: i32,
    mut read: impl FnMut(&KeptRow<'_>) -> TViewResult<R>,
) -> TViewResult<Vec<R>> {
    let returning = [
        pg_sys::SPI_OK_SELECT,
        pg_sys::SPI_OK_INSERT_RETURNING,
        pg_sys::SPI_OK_UPDATE_RETURNING,
        pg_sys::SPI_OK_DELETE_RETURNING,
    ];
    if !returning.iter().any(|&ok| rc == ok.cast_signed()) {
        return Err(error(sql, &format!("SPI returned {rc}")));
    }
    // SAFETY: per the contract, SPI_tuptable holds the last result, freed once
    // every row was read.
    unsafe {
        let table = pg_sys::SPI_tuptable;
        let tupdesc = (*table).tupdesc;
        let rows = (0..usize::try_from(pg_sys::SPI_processed).unwrap_or(0))
            .map(|i| {
                read(&KeptRow {
                    sql,
                    tuple: *(*table).vals.add(i),
                    tupdesc,
                })
            })
            .collect();
        pg_sys::SPI_freetuptable(table);
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::{LookupKeys, Plan};

    #[test]
    fn no_keys_is_no_lookup() {
        assert!(LookupKeys::of(Vec::new()).is_none());
    }

    #[test]
    fn one_key_is_compared_from_a_generic_plan() {
        let keys = LookupKeys::of(vec![7]).expect("a key");
        assert_eq!(
            keys.predicate("t.\"fk_cat\"", 1),
            "t.\"fk_cat\" = $1::pg_catalog.int8"
        );
        assert_eq!(keys.plan(), Plan::Generic);
    }

    #[test]
    fn several_keys_are_an_array_planned_per_execution() {
        let keys = LookupKeys::of(vec![7, 8]).expect("keys");
        assert_eq!(
            keys.predicate("\"fk_cat\"", 2),
            "\"fk_cat\" = ANY ($2::pg_catalog.int8[])"
        );
        assert_eq!(keys.plan(), Plan::PerExecution);
    }
}
