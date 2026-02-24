use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;
use pgrx::pg_sys;

/// Execute a DDL statement via SPI in non-atomic mode.
///
/// In PostgreSQL 18.1 compiled with assertions enabled, calling `SPI_execute()` for DDL
/// (CREATE VIEW, CREATE TABLE, CREATE TRIGGER, etc.) from within an atomic SPI context
/// triggers an assertion failure → SIGSEGV.  The fix is two-fold:
///
/// 1. Connect via `SPI_connect_ext(SPI_OPT_NONATOMIC)` to open a non-atomic SPI context.
/// 2. Execute via `SPI_execute_extended()` with `allow_nonatomic = true`, which suppresses
///    PostgreSQL's internal assertion that DDL cannot run in an atomic transaction context.
///
/// Using `SPI_execute()` even after `SPI_connect_ext(SPI_OPT_NONATOMIC)` still fires the
/// assertion in PG18 assert builds; `SPI_execute_extended` with `allow_nonatomic` is the
/// correct API for DDL executed from SPI callbacks.
///
/// This function is used for all DDL calls issued internally by `pg_tviews_create` and
/// related functions.
///
/// # Errors
/// Returns an error string if `SPI_connect_ext` or `SPI_execute` fails.
///
/// # Safety
/// Calls raw PostgreSQL SPI functions.  Must only be called from a PostgreSQL backend.
pub fn spi_run_ddl(sql: &str) -> Result<(), String> {
    use std::ffi::CString;

    let c_sql = CString::new(sql)
        .map_err(|e| format!("DDL SQL contains null byte: {e}"))?;

    unsafe {
        // SPI_OPT_NONATOMIC allows DDL in SPI context without triggering the
        // "attempted to execute DDL in atomic SPI context" assertion in PG18.
        let connect_result = pg_sys::SPI_connect_ext(pg_sys::SPI_OPT_NONATOMIC as i32);
        if connect_result != pg_sys::SPI_OK_CONNECT as i32 {
            return Err(format!("SPI_connect_ext failed: {connect_result}"));
        }

        // Use SPI_execute_extended with allow_nonatomic=true so PostgreSQL 18's
        // assertion (IsTransactionOrTransactionBlock assertion for DDL in atomic
        // context) is suppressed.
        let opts = pg_sys::SPIExecuteOptions {
            read_only: false,
            allow_nonatomic: true,
            tcount: 0,
            ..pg_sys::SPIExecuteOptions::default()
        };

        let execute_result = pg_sys::SPI_execute_extended(c_sql.as_ptr(), &opts);

        // Always finish even on error
        pg_sys::SPI_finish();

        if execute_result < 0 {
            return Err(format!("SPI_execute_extended returned error code {execute_result} for DDL: {sql}"));
        }
    }

    Ok(())
}

/// Safe wrapper for `Spi::get_one::<String>()` that avoids SIGABRT in pgrx 0.16.1.
///
/// `Spi::get_one::<String>()` invokes `SPI_getvalue` which returns a `*const c_char`
/// owned by the SPI memory context. The `String` conversion attempts to free that
/// pointer after the SPI call returns, causing an abort. This helper keeps the SPI
/// context alive during value extraction.
pub fn spi_get_string(query: &str) -> spi::Result<Option<String>> {
    Spi::connect(|client| {
        let mut rows = client.select(query, Some(1), &[])?;
        match rows.next() {
            Some(row) => Ok(row[1].value::<String>()?),
            None => Ok(None),
        }
    })
}

/// Utilities: Common Helper Functions and `PostgreSQL` Integration
///
/// This module provides utility functions used throughout `pg_tviews`:
/// - **Primary Key Extraction**: Gets PK values from trigger tuples
/// - **OID Resolution**: Maps `PostgreSQL` OIDs to names and vice versa
/// - **SPI Helpers**: Common database query patterns
/// - **Type Conversions**: `PostgreSQL` type handling
///
/// ## Key Functions
///
/// - `extract_pk()`: Primary key extraction from trigger data
/// - `relname_from_oid()`: Table/view name lookup by OID
/// - `lookup_view_for_source()`: View OID resolution
///
/// ## Design Principles
///
/// - Pure functions where possible
/// - SPI error handling with proper Result types
/// - Minimal dependencies on global state
/// - Reusable across different modules
use pgrx::pg_sys::Oid;

/// Extracts a `pk_*` integer from `NEW` or `OLD` tuple by convention.
/// For MVP we assume the column name is literally `pk_*`.
#[allow(dead_code)]
pub fn extract_pk(trigger: &PgTrigger) -> spi::Result<i64> {
    // For simplicity we assume there's a column named 'pk_*' and you know the entity.
    // For real code:
    //  - inspect relation attributes,
    //  - find first "pk_" column,
    //  - read value.
    let tuple = trigger
        .new()
        .or_else(|| trigger.old())
        .expect("Row must exist for AFTER trigger");

    // This function is no longer used in the main code path
    let pk: i64 = tuple
        .get_by_name("pk_post")? // <-- placeholder: replace per entity
        .expect("pk_post must not be null");
    Ok(pk)
}

/// Look up the view name from an OID
/// Used to find the backing view (`v_entity`) for a TVIEW
pub fn lookup_view_for_source(view_oid: Oid) -> spi::Result<String> {
    // Simply get the relation name from pg_class
    relname_from_oid(view_oid)
}

/// Look up the TVIEW table name given its OID (from `pg_tview_meta`).
pub fn relname_from_oid(oid: Oid) -> spi::Result<String> {
    Spi::connect(|client| {
        let args = vec![unsafe { DatumWithOid::new(oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
        let mut rows = client.select(
            "SELECT relname::text AS relname FROM pg_class WHERE oid = $1",
            None,
            &args,
        )?;

        if let Some(row) = rows.next() {
            row["relname"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT relname::text AS relname FROM pg_class WHERE oid = $1".to_string(),
                    error: "relname column is NULL".to_string(),
                }))
        } else {
            Err(spi::Error::from(crate::TViewError::SpiError {
                query: "SELECT relname::text AS relname FROM pg_class WHERE oid = $1".to_string(),
                error: format!("No pg_class entry for oid: {oid:?}"),
            }))
        }
    })
}

