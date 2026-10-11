use pgrx::datum::DatumWithOid;
use pgrx::pg_sys;
use pgrx::prelude::*;

pub mod ident;
pub mod spi;

/// Emit an internal diagnostic: a `DEBUG1` message, visible with
/// `client_min_messages = debug1` or `log_min_messages = debug1`. Use it for tracing
/// only; anything the user must act on belongs in `warning!`/`error!`.
macro_rules! log_debug {
    ($($arg:tt)+) => {
        ::pgrx::debug1!($($arg)+);
    };
}
pub(crate) use log_debug;

/// Execute a DDL statement via SPI in non-atomic mode.
///
/// In `PostgreSQL` 18.1 compiled with assertions enabled, calling `SPI_execute()` for DDL
/// (CREATE VIEW, CREATE TABLE, CREATE TRIGGER, etc.) from within an atomic SPI context
/// triggers an assertion failure → SIGSEGV.  The fix is two-fold:
///
/// 1. Connect via `SPI_connect_ext(SPI_OPT_NONATOMIC)` to open a non-atomic SPI context.
/// 2. Execute via `SPI_execute_extended()` with `allow_nonatomic = true`, which suppresses
///    `PostgreSQL`'s internal assertion that DDL cannot run in an atomic transaction context.
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
/// Calls raw `PostgreSQL` SPI functions.  Must only be called from a `PostgreSQL` backend.
pub fn spi_run_ddl(sql: &str) -> Result<(), String> {
    use std::ffi::CString;

    log_debug!(
        "spi_run_ddl() called with SQL ({} chars): {}",
        sql.len(),
        truncate_chars(sql, 200)
    );

    let c_sql = CString::new(sql).map_err(|e| format!("DDL SQL contains null byte: {e}"))?;

    // SAFETY: spi_run_ddl is only called from PostgreSQL backend context where SPI
    // functions are valid. SPI_connect_ext/SPI_execute_extended/SPI_finish
    // are thread-local PostgreSQL operations.
    unsafe {
        // Atomic, as the caller is (a trigger, a hook, a function called from a
        // query): only a caller that may itself commit (a procedure) gets a
        // non-atomic connection, so the DDL can never end its transaction.
        let nonatomic = pg_sys::SPI_inside_nonatomic_context();
        #[allow(clippy::cast_possible_wrap)]
        // Reason: PostgreSQL SPI constants are u32, API takes i32
        let connect_result = pg_sys::SPI_connect_ext(if nonatomic {
            pg_sys::SPI_OPT_NONATOMIC as i32
        } else {
            0
        });
        #[allow(clippy::cast_possible_wrap)]
        // Reason: PostgreSQL SPI constants are u32, API takes i32
        if connect_result != pg_sys::SPI_OK_CONNECT as i32 {
            error!(
                "spi_run_ddl() FAILED: SPI_connect_ext returned error code: {}",
                connect_result
            );
        }

        let opts = pg_sys::SPIExecuteOptions {
            read_only: false,
            allow_nonatomic: nonatomic,
            tcount: 0,
            ..pg_sys::SPIExecuteOptions::default()
        };

        let execute_result =
            pg_sys::SPI_execute_extended(c_sql.as_ptr(), std::ptr::from_ref(&opts));

        // Always finish even on error
        pg_sys::SPI_finish();

        if execute_result < 0 {
            error!(
                "spi_run_ddl() FAILED: SPI_execute_extended error {} for DDL: {}",
                execute_result, sql
            );
        }
    }

    log_debug!("spi_run_ddl() succeeded");
    Ok(())
}

/// Safe wrapper for `Spi::get_one::<String>()` that avoids SIGABRT in pgrx 0.16.1.
///
/// `Spi::get_one::<String>()` invokes `SPI_getvalue` which returns a `*const c_char`
/// owned by the SPI memory context. The `String` conversion attempts to free that
/// pointer after the SPI call returns, causing an abort. This helper keeps the SPI
/// context alive during value extraction.
pub fn spi_get_string(query: &str) -> crate::TViewResult<Option<String>> {
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
/// - `qualified_relname_from_oid()`: Schema-qualified relation name by OID
///
/// ## Design Principles
///
/// - Pure functions where possible
/// - SPI error handling with proper Result types
/// - Minimal dependencies on global state
/// - Reusable across different modules
use pgrx::pg_sys::Oid;

/// The schema-qualified, quoted name of relation `oid` (`quote_ident` on each
/// part), read from the syscache, so it is current after a rename or a move made
/// in any backend.
///
/// # Errors
/// Returns an error if no relation has that OID.
pub fn qualified_relname_from_oid(oid: Oid) -> crate::TViewResult<String> {
    // SAFETY: syscache lookups by OID; the returned names are palloc'd copies.
    unsafe {
        let rel = pg_sys::get_rel_name(oid);
        if rel.is_null() {
            return Err(crate::TViewError::CatalogError {
                operation: format!("Name relation {oid:?}"),
                pg_error: "relation does not exist".to_string(),
            });
        }
        let nsp = pg_sys::get_namespace_name(pg_sys::get_rel_namespace(oid));
        let name = |ptr: *const std::ffi::c_char| {
            std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
        };
        Ok(format!(
            "{}.{}",
            ident::quote_if_needed(&name(nsp)),
            ident::quote_if_needed(&name(rel))
        ))
    }
}

/// The SQL name of type `typid` with modifier `typmod` (`numeric(6,2)`, `bit(4)`),
/// schema-qualified unless it is one of the SQL-standard names, so it means the
/// same type whatever the `search_path` (`app.mood`, `"Other"."Weird Type"`,
/// `pg_catalog.text`). No SPI.
#[must_use]
pub fn qualified_type_name(typid: Oid, typmod: i32) -> String {
    #[allow(clippy::cast_possible_truncation)] // Reason: the flags are 1 and 4, bits16 holds them
    const FLAGS: u16 =
        (pg_sys::FORMAT_TYPE_TYPEMOD_GIVEN | pg_sys::FORMAT_TYPE_FORCE_QUALIFY) as u16;
    // SAFETY: format_type_extended returns a palloc'd C string (it raises on an
    // unknown type), copied before it is freed.
    unsafe {
        let name = pg_sys::format_type_extended(typid, typmod, FLAGS);
        let out = std::ffi::CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned();
        pg_sys::pfree(name.cast());
        out
    }
}

/// Each column of relation `relid` with its [`qualified_type_name`], in order.
///
/// # Errors
/// Returns an error if the catalog query fails.
pub fn column_types(relid: Oid) -> crate::TViewResult<Vec<(String, String)>> {
    Spi::connect(|client| {
        let args = [crate::utils::spi::oid(relid)];
        let mut out = Vec::new();
        for row in client.select(
            "SELECT attname::pg_catalog.text, atttypid, atttypmod FROM pg_catalog.pg_attribute \
             WHERE attrelid = $1 AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
            None,
            &args,
        )? {
            if let (Some(name), Some(typid), Some(typmod)) = (
                row.get::<String>(1)?,
                row.get::<Oid>(2)?,
                row.get::<i32>(3)?,
            ) {
                out.push((name, qualified_type_name(typid, typmod)));
            }
        }
        Ok::<_, pgrx::spi::Error>(out)
    })
    .map_err(|e| crate::TViewError::CatalogError {
        operation: format!("Read the column types of relation {relid:?}"),
        pg_error: e.to_string(),
    })
}

/// Schema every `pg_tviews` object lives in, fixed by the control file.
const EXT_SCHEMA: &str = "tviews";

thread_local! {
    /// Keys of the conditions [`log_once`] already reported in this backend.
    static LOGGED_ONCE: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Write `message` to the server log (`LOG`) the first time this backend sees
/// the condition `key`; later calls are silent. For conditions a
/// normal workload hits on every write, where a client WARNING would be noise.
pub fn log_once(key: &str, message: &str) {
    if first_time(key) {
        log!("pg_tviews: {message}");
    }
}

/// True the first time this backend sees the condition `key`, false afterwards
/// (until [`forget_logged`]).
pub fn first_time(key: &str) -> bool {
    LOGGED_ONCE.with(|seen| seen.borrow_mut().insert(key.to_string()))
}

/// Let [`log_once`] report `key` again, after the condition may have changed.
pub fn forget_logged(key: &str) {
    LOGGED_ONCE.with(|seen| seen.borrow_mut().remove(key));
}

/// `pg_tview_meta`, qualified with the extension's schema, so catalog queries do
/// not depend on the session's `search_path`.
pub fn meta_table() -> String {
    format!("{EXT_SCHEMA}.pg_tview_meta")
}

/// Schema the `pg_tviews` extension is installed in. It needs no quoting.
pub const fn ext_schema() -> &'static str {
    EXT_SCHEMA
}

/// The column names of relation `rel_oid`, in order (cached per backend). Used
/// for the column lists of upserts.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn get_view_columns_by_oid(rel_oid: Oid) -> crate::TViewResult<Vec<String>> {
    crate::cache::COLUMNS.with(|m| {
        m.get_or_load(rel_oid, || {
            crate::metrics::metrics_api::record_catalog_lookup();
            crate::cache::watch(&[rel_oid]);
            Spi::connect(|client| -> crate::TViewResult<Vec<String>> {
                let rows = client.select(
                    "SELECT a.attname::pg_catalog.text FROM pg_catalog.pg_attribute a \
                     WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped \
                     ORDER BY a.attnum",
                    None,
                    &[DatumWithOid::from(rel_oid)],
                )?;
                let mut columns = Vec::new();
                for row in rows {
                    if let Some(name) = row.get::<String>(1)? {
                        columns.push(name);
                    }
                }
                Ok(columns)
            })
        })
    })
}

/// `text` as an SQL string literal, as `PostgreSQL`'s `quote_literal()` writes it:
/// quotes doubled, and an `E''` literal with backslashes doubled when it holds a
/// backslash, so it reads the same whatever `standard_conforming_strings` is.
/// For code that cannot call the server (pure Rust, unit-tested); the query-tree
/// walker uses the server's `quote_literal_cstr`. Values passed to a query are bind
/// parameters.
#[must_use]
pub fn quote_literal(text: &str) -> String {
    let quoted = text.replace('\'', "''");
    if text.contains('\\') {
        format!("E'{}'", quoted.replace('\\', "\\\\"))
    } else {
        format!("'{quoted}'")
    }
}

/// Longest identifier `PostgreSQL` keeps (`NAMEDATALEN - 1` bytes).
pub const MAX_IDENTIFIER_BYTES: usize = 63;

/// Fit a generated identifier into 63 bytes.
///
/// `PostgreSQL` silently truncates identifiers longer than 63 bytes, so two long
/// names could collide. An over-long name is cut at a char boundary and suffixed
/// with an FNV-1a hash of the full name, which keeps it unique and stable.
#[must_use]
pub fn fit_identifier(full: String) -> String {
    if full.len() <= MAX_IDENTIFIER_BYTES {
        return full;
    }
    let hash = full.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    let tag = format!("_{hash:08x}");
    format!(
        "{}{tag}",
        truncate_chars(&full, MAX_IDENTIFIER_BYTES - tag.len())
    )
}

/// At most the first `max` bytes of `s`, cut on a character boundary.
pub(crate) fn truncate_chars(s: &str, max: usize) -> &str {
    &s[..s.floor_char_boundary(max)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_literal_reads_the_same_under_any_string_setting() {
        assert_eq!(quote_literal("post"), "'post'");
        assert_eq!(quote_literal("it's"), "'it''s'");
        assert_eq!(quote_literal(r"a\b'c"), r"E'a\\b''c'");
    }

    #[test]
    fn quote_identifier_always_quotes_and_doubles_quotes() {
        assert_eq!(ident::quoted("post"), "\"post\"");
        assert_eq!(ident::quoted("Post"), "\"Post\"");
        assert_eq!(ident::quoted("test\"col"), "\"test\"\"col\"");
    }

    #[test]
    fn test_quote_identifier_normal() {
        assert_eq!(ident::quoted("post"), "\"post\"");
    }

    #[test]
    fn test_quote_identifier_uppercase() {
        assert_eq!(ident::quoted("Post"), "\"Post\"");
    }

    #[test]
    fn test_quote_identifier_with_underscore() {
        assert_eq!(ident::quoted("pk_user"), "\"pk_user\"");
    }

    #[test]
    fn test_quote_identifier_with_internal_quotes() {
        assert_eq!(ident::quoted("test\"col"), "\"test\"\"col\"");
    }
}
