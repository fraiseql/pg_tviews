use pgrx::pg_sys;
use pgrx::prelude::*;

pub mod ident;
pub mod lexer;
pub mod shmem;
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

/// Raise `54001 stack depth limit exceeded` when the stack is past
/// `max_stack_depth`: called at the entry of every function that recurses over a
/// parse or plan tree without going through PostgreSQL's walkers, which check it
/// themselves. A deep definition then fails cleanly instead of overflowing the
/// stack.
#[inline]
pub fn guard_depth() {
    // SAFETY: reads the stack pointer and `max_stack_depth`; raises an ERROR,
    // which pgrx turns into a Rust panic unwound to the nearest guard.
    unsafe { pg_sys::check_stack_depth() }
}

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
        if nsp.is_null() {
            return Err(crate::TViewError::CatalogError {
                operation: format!("Name relation {oid:?}"),
                pg_error: "its schema does not exist".to_string(),
            });
        }
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
    crate::utils::spi::connect(|client| {
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

/// True the first time this backend sees the condition `key`, false afterwards.
pub fn first_time(key: &str) -> bool {
    LOGGED_ONCE.with(|seen| seen.borrow_mut().insert(key.to_string()))
}

/// Schema the `pg_tviews` extension is installed in. It needs no quoting.
pub const fn ext_schema() -> &'static str {
    EXT_SCHEMA
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

unsafe extern "C" {
    /// `PostgreSQL`'s `datumIsEqual` (`src/backend/utils/adt/datum.c`) — exported
    /// by the backend but absent from the pgrx bindings; resolved at `.so` load
    /// time like every other `pg_sys` symbol. Compares datums for physical equality.
    #[link_name = "datumIsEqual"]
    pub(crate) fn datum_is_equal(
        value1: pg_sys::Datum,
        value2: pg_sys::Datum,
        typ_by_val: bool,
        typ_len: i32,
    ) -> bool;
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
