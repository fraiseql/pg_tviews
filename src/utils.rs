use pgrx::datum::DatumWithOid;
use pgrx::heap_tuple::PgHeapTuple;
use pgrx::pg_sys;
use pgrx::prelude::*;
use pgrx::AllocatedByPostgres;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

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

    let c_sql = CString::new(sql).map_err(|e| format!("DDL SQL contains null byte: {e}"))?;

    unsafe {
        // SPI_OPT_NONATOMIC allows DDL in SPI context without triggering the
        // "attempted to execute DDL in atomic SPI context" assertion in PG18.
        #[allow(clippy::cast_possible_wrap)] // PostgreSQL SPI constants are u32, API takes i32
        let connect_result = pg_sys::SPI_connect_ext(pg_sys::SPI_OPT_NONATOMIC as i32);
        #[allow(clippy::cast_possible_wrap)]
        // Reason: PostgreSQL SPI constants are u32, API takes i32
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

        let execute_result =
            pg_sys::SPI_execute_extended(c_sql.as_ptr(), std::ptr::from_ref(&opts));

        // Always finish even on error
        pg_sys::SPI_finish();

        if execute_result < 0 {
            return Err(format!(
                "SPI_execute_extended returned error code {execute_result} for DDL: {sql}"
            ));
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

/// Extract an integer column value as i64, supporting both INTEGER and BIGINT columns.
///
/// Tries BIGINT (i64) first, then falls back to INTEGER (i32) with promotion.
/// This allows triggers to work regardless of whether the PK/FK column is
/// `INTEGER`/`SERIAL` or `BIGINT`/`BIGSERIAL`.
pub fn tuple_get_i64(tuple: &PgHeapTuple<'_, AllocatedByPostgres>, col: &str) -> Option<i64> {
    // Try BIGINT (i64) first — most common for pk_*/fk_* columns
    if let Ok(Some(v)) = tuple.get_by_name::<i64>(col) {
        return Some(v);
    }
    // Fall back to INTEGER (i32) and promote
    if let Ok(Some(v)) = tuple.get_by_name::<i32>(col) {
        return Some(i64::from(v));
    }
    None
}

/// Extracts a `pk_*` integer from `NEW` or `OLD` tuple by convention.
///
/// Derives the PK column name dynamically from the triggering table OID.
/// Convention: `tb_<entity>` → `pk_<entity>` (e.g. `tb_user` → `pk_user`).
pub fn extract_pk(trigger: &PgTrigger) -> spi::Result<i64> {
    let tuple = trigger
        .new()
        .or_else(|| trigger.old())
        .expect("Row must exist for AFTER trigger");

    let table_oid = trigger
        .relation()
        .map_err(|_| crate::TViewError::SpiError {
            query: "get trigger relation".to_string(),
            error: "Failed to get trigger relation".to_string(),
        })?
        .oid();

    let entity = crate::catalog::entity_for_table(table_oid)?.ok_or_else(|| {
        crate::TViewError::SpiError {
            query: "entity_for_table".to_string(),
            error: format!("Table OID {table_oid:?} not managed by pg_tviews"),
        }
    })?;

    let pk_column = format!("pk_{entity}");

    tuple_get_i64(&tuple, &pk_column).ok_or_else(|| {
        crate::TViewError::SpiError {
            query: pk_column.clone(),
            error: format!("{pk_column} must not be null"),
        }
        .into()
    })
}

/// Look up the view name from an OID
/// Used to find the backing view (`v_entity`) for a TVIEW
pub fn lookup_view_for_source(view_oid: Oid) -> spi::Result<String> {
    // Simply get the relation name from pg_class
    relname_from_oid(view_oid)
}

/// Global cache for OID → relname mappings
/// OID→relname mappings are stable within a session (only change on DDL)
static OID_RELNAME_CACHE: LazyLock<Mutex<HashMap<Oid, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the OID→relname cache
/// Called when DDL creates/drops tables
pub fn invalidate_oid_relname_cache() {
    let mut cache = OID_RELNAME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.clear();
}

/// Global cache for view column names (view_name → column names)
/// View column lists are stable within a session (only change on DDL)
pub static VIEW_COLUMNS_CACHE: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the view columns cache
/// Called when DDL creates/drops/alters tables with columns
pub fn invalidate_view_columns_cache() {
    let mut cache = VIEW_COLUMNS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.clear();
}

/// DML components for dedup key refresh: (col_list, do_update_clause)
/// Precomputed once per TVIEW to avoid repeated string building
pub type DedupDmlCache = HashMap<String, (String, String)>;

/// Global cache for dedup key DML strings (view_name → (col_list, do_update))
/// DML strings are stable within a session (only change on DDL)
pub static DEDUP_DML_CACHE: LazyLock<Mutex<DedupDmlCache>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Invalidate the dedup key DML cache
/// Called when DDL creates/drops/alters tables with columns
pub fn invalidate_dedup_dml_cache() {
    let mut cache = DEDUP_DML_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.clear();
}

/// Look up the TVIEW table name given its OID (from `pg_tview_meta`).
/// Results are cached per session to avoid repeated pg_class queries.
pub fn relname_from_oid(oid: Oid) -> spi::Result<String> {
    // Fast path: check cache
    {
        let cache = OID_RELNAME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(name) = cache.get(&oid) {
            return Ok(name.clone());
        }
    }

    // Slow path: query and cache
    let name: String = Spi::connect(|client| {
        let args =
            vec![unsafe { DatumWithOid::new(oid, PgOid::BuiltIn(PgBuiltInOids::OIDOID).value()) }];
        let mut rows = client.select(
            "SELECT relname::text AS relname FROM pg_class WHERE oid = $1",
            None,
            &args,
        )?;

        if let Some(row) = rows.next() {
            row["relname"].value::<String>()?.ok_or_else(|| {
                spi::Error::from(crate::TViewError::SpiError {
                    query: "SELECT relname::text AS relname FROM pg_class WHERE oid = $1"
                        .to_string(),
                    error: "relname column is NULL".to_string(),
                })
            })
        } else {
            Err(spi::Error::from(crate::TViewError::SpiError {
                query: "SELECT relname::text AS relname FROM pg_class WHERE oid = $1".to_string(),
                error: format!("No pg_class entry for oid: {oid:?}"),
            }))
        }
    })?;

    // Cache the result
    OID_RELNAME_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(oid, name.clone());
    Ok(name)
}

/// Get the list of column names for a view/table by name. Results are cached per session.
/// Used for UPSERT column lists to avoid repeated pg_attribute queries.
pub fn get_view_columns(view_name: &str) -> spi::Result<Vec<String>> {
    // Fast path: check cache
    {
        let cache = VIEW_COLUMNS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cols) = cache.get(view_name) {
            return Ok(cols.clone());
        }
    }

    // Slow path: query and cache
    let cols: Vec<String> = Spi::connect(|client| -> spi::Result<Vec<String>> {
        let args = vec![unsafe {
            DatumWithOid::new(view_name, PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value())
        }];
        let rows = client.select(
            "SELECT a.attname::text \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             WHERE c.relname = $1 AND a.attnum > 0 AND NOT a.attisdropped \
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

    // Cache the result
    VIEW_COLUMNS_CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(view_name.to_string(), cols.clone());
    Ok(cols)
}

/// Get column names for a relation by OID. Resolves name via relname_from_oid,
/// then delegates to get_view_columns for caching.
pub fn get_view_columns_by_oid(rel_oid: Oid) -> spi::Result<Vec<String>> {
    let name = relname_from_oid(rel_oid)?;
    get_view_columns(&name)
}

/// Quote a SQL identifier for safe use in queries.
///
/// Doubles any internal double-quotes and wraps the identifier in double-quotes.
/// This is safe for identifiers that are already constrained by PostgreSQL
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
            let mut cache = OID_RELNAME_CACHE.lock().unwrap();
            cache.insert(Oid::from(123), "test_table".to_string());
        }

        // Verify it's there
        {
            let cache = OID_RELNAME_CACHE.lock().unwrap();
            assert!(cache.get(&Oid::from(123)).is_some());
        }

        // Invalidate cache
        invalidate_oid_relname_cache();

        // Verify it's gone
        {
            let cache = OID_RELNAME_CACHE.lock().unwrap();
            assert!(cache.is_empty());
        }
    }

    #[test]
    fn test_view_columns_cache_invalidation() {
        // Clear cache first
        invalidate_view_columns_cache();

        // Populate cache with test entries
        {
            let mut cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            cache.insert(
                "v_user".to_string(),
                vec!["id".to_string(), "name".to_string()],
            );
            cache.insert(
                "v_post".to_string(),
                vec!["id".to_string(), "title".to_string(), "user_id".to_string()],
            );
        }

        // Verify entries are there
        {
            let cache = VIEW_COLUMNS_CACHE.lock().unwrap();
            assert_eq!(cache.len(), 2);
            assert!(cache.contains_key("v_user"));
            assert!(cache.contains_key("v_post"));
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
