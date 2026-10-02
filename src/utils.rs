use pgrx::AllocatedByPostgres;
use pgrx::datum::DatumWithOid;
use pgrx::heap_tuple::PgHeapTuple;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// Emit an internal diagnostic. Silent at the default settings: it is a `DEBUG1` message
/// (visible with `client_min_messages = debug1`), or a `NOTICE` when the session sets
/// `pg_tviews.log_level = 'debug'`. Use it for tracing only; anything the user must act on
/// belongs in `warning!`/`error!`.
macro_rules! log_debug {
    ($($arg:tt)+) => {
        if $crate::config::log_level().eq_ignore_ascii_case("debug") {
            ::pgrx::notice!($($arg)+);
        } else {
            ::pgrx::debug1!($($arg)+);
        }
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
        &sql[..sql.len().min(200)]
    );

    let c_sql = CString::new(sql).map_err(|e| format!("DDL SQL contains null byte: {e}"))?;

    // SAFETY: spi_run_ddl is only called from PostgreSQL backend context where SPI
    // functions are valid. SPI_connect_ext/SPI_execute_extended/SPI_finish
    // are thread-local PostgreSQL operations.
    unsafe {
        // SPI_OPT_NONATOMIC allows DDL in SPI context without triggering the
        // "attempted to execute DDL in atomic SPI context" assertion in PG18.
        #[allow(clippy::cast_possible_wrap)] // PostgreSQL SPI constants are u32, API takes i32
        let connect_result = pg_sys::SPI_connect_ext(pg_sys::SPI_OPT_NONATOMIC as i32);
        #[allow(clippy::cast_possible_wrap)]
        // Reason: PostgreSQL SPI constants are u32, API takes i32
        if connect_result != pg_sys::SPI_OK_CONNECT as i32 {
            error!(
                "spi_run_ddl() FAILED: SPI_connect_ext returned error code: {}",
                connect_result
            );
            #[allow(unreachable_code)]
            return Err(format!(
                "SPI_connect_ext failed (error! should diverge): {connect_result}"
            ));
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

        let execute_result =
            pg_sys::SPI_execute_extended(c_sql.as_ptr(), std::ptr::from_ref(&opts));

        // Always finish even on error
        pg_sys::SPI_finish();

        if execute_result < 0 {
            error!(
                "spi_run_ddl() FAILED: SPI_execute_extended error {} for DDL: {}",
                execute_result, sql
            );
            #[allow(unreachable_code)]
            return Err(format!(
                "SPI_execute_extended failed (error! should diverge): {execute_result}"
            ));
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
/// - `qualified_relname_from_oid()`: Schema-qualified relation name by OID
///
/// ## Design Principles
///
/// - Pure functions where possible
/// - SPI error handling with proper Result types
/// - Minimal dependencies on global state
/// - Reusable across different modules
use pgrx::pg_sys::Oid;

/// Result of extracting an integer column from a tuple.
///
/// Parallels `KeyExtraction` in `trigger.rs` but for integer (PK/FK) columns.
pub enum IntExtraction {
    /// Column exists and has a non-NULL integer value.
    Value(i64),
    /// Column exists but the value is NULL.
    Null,
    /// Column not found or type is not integer (i32/i64).
    Missing,
}

/// Extract an integer column value as i64, supporting both INTEGER and BIGINT columns.
///
/// Tries BIGINT (i64) first, then falls back to INTEGER (i32) with promotion.
/// This allows triggers to work regardless of whether the PK/FK column is
/// `INTEGER`/`SERIAL` or `BIGINT`/`BIGSERIAL`.
///
/// Returns `IntExtraction::Null` when the column exists but is NULL (normal for
/// optional FKs), and `IntExtraction::Missing` when the column is absent entirely
/// (likely a misconfiguration).
pub fn tuple_get_i64(tuple: &PgHeapTuple<'_, AllocatedByPostgres>, col: &str) -> IntExtraction {
    match tuple.get_by_name::<i64>(col) {
        Ok(Some(v)) => return IntExtraction::Value(v),
        Ok(None) => return IntExtraction::Null,
        Err(_) => {} // not i64, try i32
    }
    match tuple.get_by_name::<i32>(col) {
        Ok(Some(v)) => IntExtraction::Value(i64::from(v)),
        Ok(None) => IntExtraction::Null,
        Err(_) => IntExtraction::Missing,
    }
}

/// Extracts `pk_<entity>` from the trigger's `NEW` or `OLD` tuple.
///
/// The caller passes the entity it resolved from the table, or from the
/// partitioned table when the trigger fired on a partition (a partition has the
/// same columns, but it is not `tb_<entity>`).
pub fn extract_pk(trigger: &PgTrigger, entity: &str) -> spi::Result<i64> {
    let tuple = trigger
        .new()
        .or_else(|| trigger.old())
        .expect("Row must exist for AFTER trigger");

    let pk_column = format!("pk_{entity}");

    match tuple_get_i64(&tuple, &pk_column) {
        IntExtraction::Value(v) => Ok(v),
        IntExtraction::Null => Err(crate::TViewError::SpiError {
            query: pk_column.clone(),
            error: format!("{pk_column} must not be NULL"),
        }
        .into()),
        IntExtraction::Missing => Err(crate::TViewError::SpiError {
            query: pk_column.clone(),
            error: format!("{pk_column} column not found on tuple (expected INTEGER or BIGINT)"),
        }
        .into()),
    }
}

/// Global cache for OID → qualified relname mappings (schema-qualified)
/// Populated by `qualified_relname_from_oid`; invalidated on DDL.
static OID_QUALIFIED_RELNAME_CACHE: LazyLock<Mutex<HashMap<Oid, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the OID→qualified relname cache.
/// Called when DDL creates/drops tables.
pub fn invalidate_oid_relname_cache() {
    OID_QUALIFIED_RELNAME_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// Global cache for view column names (`view_name` → column names)
/// View column lists are stable within a session (only change on DDL)
pub static VIEW_COLUMNS_CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the view columns cache
/// Called when DDL creates/drops/alters tables with columns
pub fn invalidate_view_columns_cache() {
    let mut cache = VIEW_COLUMNS_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.clear();
}

/// DML components for dedup key refresh: (`col_list`, `do_update_clause`)
/// Precomputed once per TVIEW to avoid repeated string building
pub type DedupDmlCache = HashMap<String, (String, String)>;

/// Global cache for dedup key DML strings (`view_name` → (`col_list`, `do_update`))
/// DML strings are stable within a session (only change on DDL)
pub static DEDUP_DML_CACHE: LazyLock<Mutex<DedupDmlCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the dedup key DML cache
/// Called when DDL creates/drops/alters tables with columns
pub fn invalidate_dedup_dml_cache() {
    let mut cache = DEDUP_DML_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.clear();
}

/// Bound a per-session memoization cache to `pg_tviews.cache_size` entries.
///
/// Call immediately before inserting a fresh entry: if the cache is already at the
/// configured limit it is cleared and repopulated lazily on subsequent misses.
/// Safe because every cache this is used on is pure catalog-lookup memoization —
/// clearing it only costs a re-query, never correctness.
pub fn bound_cache<K, V>(cache: &mut HashMap<K, V>) {
    if cache.len() >= crate::config::cache_size() {
        cache.clear();
    }
}

/// Look up the schema-qualified, properly-quoted name for a relation OID.
///
/// Returns `"schema"."table"` using `quote_ident` on each part so the result is safe
/// for direct embedding in a FROM clause regardless of `search_path` or special characters.
/// Results are cached per session.
pub fn qualified_relname_from_oid(oid: Oid) -> spi::Result<String> {
    // Fast path: check cache
    {
        let cache = OID_QUALIFIED_RELNAME_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(name) = cache.get(&oid) {
            return Ok(name.clone());
        }
    }

    // Slow path: resolve via pg_class + pg_namespace
    crate::metrics::metrics_api::record_catalog_lookup();
    let qname: String = Spi::connect(|client| {
        let args =
            vec![unsafe { DatumWithOid::new(oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
        let mut rows = client.select(
            "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname) AS qname \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = $1",
            None,
            &args,
        )?;

        if let Some(row) = rows.next() {
            row["qname"].value::<String>()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: "qualified_relname_from_oid".to_string(),
                    error: "qname column is NULL".to_string(),
                })
            })
        } else {
            Err(spi::Error::from(crate::TViewError::SpiError {
                query: "qualified_relname_from_oid".to_string(),
                error: format!("No pg_class entry for oid: {oid:?}"),
            }))
        }
    })?;

    // Cache the result (bounded by pg_tviews.cache_size)
    {
        let mut cache = OID_QUALIFIED_RELNAME_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        bound_cache(&mut cache);
        cache.insert(oid, qname.clone());
    }
    Ok(qname)
}

/// Schema every `pg_tviews` object lives in, fixed by the control file.
const EXT_SCHEMA: &str = "tviews";

thread_local! {
    /// Keys of the conditions [`log_once`] already reported in this backend.
    static LOGGED_ONCE: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Write `message` to the server log (`LOG`) the first time this backend sees
/// the condition `key`; later calls are silent (issue #159). For conditions a
/// normal workload hits on every write, where a client WARNING would be noise.
pub fn log_once(key: &str, message: &str) {
    if LOGGED_ONCE.with(|seen| seen.borrow_mut().insert(key.to_string())) {
        log!("pg_tviews: {message}");
    }
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

/// Get the list of column names for a view/table by schema-qualified name. Results are cached per session.
/// Used for UPSERT column lists to avoid repeated `pg_attribute` queries.
///
/// The cache key includes the schema name to avoid collisions when multiple views
/// have the same name in different schemas.
pub fn get_view_columns(schema_name: &str, view_name: &str) -> spi::Result<Vec<String>> {
    let cache_key = format!("{schema_name}.{view_name}");

    // Fast path: check cache
    {
        let cache = VIEW_COLUMNS_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cols) = cache.get(&cache_key) {
            return Ok(cols.clone());
        }
    }

    // Slow path: query and cache
    crate::metrics::metrics_api::record_catalog_lookup();
    let cols: Vec<String> = Spi::connect(|client| -> spi::Result<Vec<String>> {
        let args = vec![
            unsafe {
                DatumWithOid::new(schema_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
            },
            unsafe { DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
        ];
        let rows = client.select(
            "SELECT a.attname::text \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
            None,
            &args,
        )?;
        // Pre-allocate with estimated capacity (typical views have 5-20 columns)
        let mut result = Vec::with_capacity(10);
        for r in rows {
            if let Some(name) = r["attname"].value::<String>()? {
                result.push(name);
            }
        }
        Ok(result)
    })?;

    // Cache the result (bounded by pg_tviews.cache_size)
    {
        let mut cache = VIEW_COLUMNS_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        bound_cache(&mut cache);
        cache.insert(cache_key, cols.clone());
    }
    Ok(cols)
}

/// Get column names for a relation by OID. Resolves schema and name from the OID,
/// then delegates to `get_view_columns` for caching.
pub fn get_view_columns_by_oid(rel_oid: Oid) -> spi::Result<Vec<String>> {
    // Fast path: the columns of this relation were resolved before.
    let oid_key = format!("oid:{}", rel_oid.to_u32());
    {
        let cache = VIEW_COLUMNS_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cols) = cache.get(&oid_key) {
            return Ok(cols.clone());
        }
    }
    crate::metrics::metrics_api::record_catalog_lookup();
    // Get schema and table name from OID
    let (schema_name, table_name): (String, String) = Spi::connect(|client| {
        let args = vec![unsafe {
            DatumWithOid::new(rel_oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value())
        }];
        let mut rows = client.select(
            "SELECT n.nspname::text, c.relname::text \
             FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = $1",
            None,
            &args,
        )?;

        if let Some(row) = rows.next() {
            let schema = row["nspname"].value::<String>()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: "get_view_columns_by_oid schema lookup".to_string(),
                    error: "nspname column is NULL".to_string(),
                })
            })?;
            let table = row["relname"].value::<String>()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: "get_view_columns_by_oid table lookup".to_string(),
                    error: "relname column is NULL".to_string(),
                })
            })?;
            Ok((schema, table))
        } else {
            Err(spi::Error::from(crate::TViewError::SpiError {
                query: "get_view_columns_by_oid".to_string(),
                error: format!("No pg_class entry for oid: {rel_oid:?}"),
            }))
        }
    })?;

    let cols = get_view_columns(&schema_name, &table_name)?;
    VIEW_COLUMNS_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(oid_key, cols.clone());
    Ok(cols)
}

/// Quote a SQL identifier for safe use in queries.
///
/// Doubles any internal double-quotes and wraps the identifier in double-quotes.
/// This is safe for identifiers that are already constrained by `PostgreSQL`
/// (entity names, column names, etc. which match `\w+`).
///
/// # Examples
///
/// ```
/// # use crate::utils::quote_identifier;
/// assert_eq!(quote_identifier("post"), "\"post\"");
/// assert_eq!(quote_identifier("Post"), "\"Post\"");
/// assert_eq!(quote_identifier("pk_user"), "\"pk_user\"");
/// assert_eq!(quote_identifier("test\"col"), "\"test\"\"col\"");
/// ```
#[must_use]
pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
    let mut cut = MAX_IDENTIFIER_BYTES - tag.len();
    while !full.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{tag}", &full[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_identifier_normal() {
        assert_eq!(quote_identifier("post"), "\"post\"");
    }

    #[test]
    fn test_quote_identifier_uppercase() {
        assert_eq!(quote_identifier("Post"), "\"Post\"");
    }

    #[test]
    fn test_quote_identifier_with_underscore() {
        assert_eq!(quote_identifier("pk_user"), "\"pk_user\"");
    }

    #[test]
    fn test_quote_identifier_with_internal_quotes() {
        assert_eq!(quote_identifier("test\"col"), "\"test\"\"col\"");
    }

    #[test]
    fn test_oid_relname_cache_invalidation() {
        use pg_sys::Oid;

        // Clear cache first
        invalidate_oid_relname_cache();

        // Populate cache with a test entry
        {
            let mut cache = OID_QUALIFIED_RELNAME_CACHE.lock().unwrap();
            cache.insert(Oid::from(123), "test_table".to_string());
        }

        // Verify it's there
        {
            let cache = OID_QUALIFIED_RELNAME_CACHE.lock().unwrap();
            assert!(cache.get(&Oid::from(123)).is_some());
        }

        // Invalidate cache
        invalidate_oid_relname_cache();

        // Verify it's gone
        {
            let cache = OID_QUALIFIED_RELNAME_CACHE.lock().unwrap();
            assert!(cache.is_empty());
        }
    }

    #[test]
    fn test_view_columns_cache_invalidation() {
        // Clear cache first
        invalidate_view_columns_cache();

        // Populate cache with test entries using schema-qualified keys
        {
            let mut cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            cache.insert(
                "public.v_user".to_string(),
                vec!["id".to_string(), "name".to_string()],
            );
            cache.insert(
                "public.v_post".to_string(),
                vec!["id".to_string(), "title".to_string(), "user_id".to_string()],
            );
            // Test that same view name in different schema creates separate cache entries
            cache.insert(
                "app.v_user".to_string(),
                vec!["id".to_string(), "name".to_string(), "org_id".to_string()],
            );
        }

        // Verify entries are there
        {
            let cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            assert_eq!(cache.len(), 3);
            assert!(cache.contains_key("public.v_user"));
            assert!(cache.contains_key("public.v_post"));
            assert!(cache.contains_key("app.v_user"));
        }

        // Verify different schemas have different column lists
        {
            let cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            let public_user = cache.get("public.v_user").unwrap();
            let app_user = cache.get("app.v_user").unwrap();
            assert_eq!(public_user.len(), 2);
            assert_eq!(app_user.len(), 3);
        }

        // Invalidate cache
        invalidate_view_columns_cache();

        // Verify it's gone
        {
            let cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            assert!(cache.is_empty());
        }
    }
}
