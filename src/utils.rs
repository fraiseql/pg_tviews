use pgrx::prelude::*;
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
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

/// Extracts primary key from NEW or OLD tuple using naming convention
///
/// Looks for column `pk_<entity>` in the tuple.
///
/// # Arguments
///
/// * `trigger` - The trigger context
/// * `entity` - Entity name (e.g., "user", "post")
///
/// # Returns
///
/// The primary key value as i64, or error if column not found/null.
///
/// # Example
///
/// For entity "user", looks for column "pk_user".
pub fn extract_pk(trigger: &PgTrigger, entity: &str) -> spi::Result<i64> {
    let tuple = trigger
        .new()
        .or(trigger.old())
        .expect("Row must exist for AFTER trigger");

    // Build column name from entity: "user" -> "pk_user"
    let pk_column = format!("pk_{}", entity);

    let pk: i64 = tuple
        .get_by_name(&pk_column)?
        .ok_or_else(|| {
            spi::Error::from(crate::TViewError::SpiError {
                query: format!("extract pk from column {}", pk_column),
                error: format!("Column '{}' is NULL or missing", pk_column),
            })
        })?;

    Ok(pk)
}

/// Derive entity name from table name using naming convention
///
/// Follows the pattern: `tb_<entity>` → `<entity>`
///
/// # Arguments
///
/// * `table_name` - Full table name (e.g., "tb_user")
///
/// # Returns
///
/// Entity name if table follows convention, None otherwise.
///
/// # Example
///
/// ```
/// derive_entity_from_table("tb_user") // => Some("user")
/// derive_entity_from_table("users")   // => None
/// ```
pub fn derive_entity_from_table(table_name: &str) -> Option<&str> {
    table_name.strip_prefix("tb_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_entity_from_table() {
        // Test valid cases
        assert_eq!(derive_entity_from_table("tb_user"), Some("user"));
        assert_eq!(derive_entity_from_table("tb_post"), Some("post"));
        assert_eq!(derive_entity_from_table("tb_category"), Some("category"));

        // Test invalid cases
        assert_eq!(derive_entity_from_table("user"), None);
        assert_eq!(derive_entity_from_table("users"), None);
        assert_eq!(derive_entity_from_table("tv_user"), None);
        assert_eq!(derive_entity_from_table(""), None);
        assert_eq!(derive_entity_from_table("tb"), None);
    }
}

// ✅ Tests at module level (outside function)
#[cfg(feature = "pg_test")]
#[pg_schema]
mod helper_tests {
    use super::*;

    #[pg_test]
    fn test_extract_jsonb_id_basic() {
        let data = JsonB(serde_json::json!({
            "id": "user_123",
            "name": "Alice"
        }));

        let id = extract_jsonb_id(&data, "id").unwrap();
        assert_eq!(id, Some("user_123".to_string()));
    }

    #[pg_test]
    fn test_extract_jsonb_id_custom_key() {
        let data = JsonB(serde_json::json!({
            "uuid": "abc-def-ghi",
            "name": "Bob"
        }));

        let uuid = extract_jsonb_id(&data, "uuid").unwrap();
        assert_eq!(uuid, Some("abc-def-ghi".to_string()));
    }

    #[pg_test]
    fn test_extract_jsonb_id_missing() {
        let data = JsonB(serde_json::json!({
            "name": "Charlie"
        }));

        let id = extract_jsonb_id(&data, "id").unwrap();
        assert_eq!(id, None);
    }

    #[pg_test]
    #[should_panic(expected = "Invalid identifier")]
    fn test_extract_jsonb_id_sql_injection() {
        let data = JsonB(serde_json::json!({"id": "test"}));

        // Should reject malicious input
        let _ = extract_jsonb_id(&data, "id'); DROP TABLE users; --").unwrap();
    }
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
                error: format!("No pg_class entry for oid: {:?}", oid),
            }))
        }
    })
}

/// Extract ID field from JSONB data using `jsonb_ivm` extension.
///
/// **Security**: This function validates the `id_key` parameter to prevent SQL injection.
/// Only alphanumeric characters and underscores are allowed in `id_key`.
///
/// # Arguments
///
/// * `data` - JSONB data to extract ID from
/// * `id_key` - Key name for ID field (must be valid identifier: `[a-zA-Z0-9_]+`)
///
/// # Returns
///
/// ID value as string, or None if not found
///
/// # Errors
///
/// Returns `TViewError` if:
/// - `id_key` contains invalid characters (security)
/// - Database query fails
///
/// # Performance
///
/// - With `jsonb_ivm`: ~5× faster than `data->>'id'`
/// - Without `jsonb_ivm`: Same as `data->>'id'`
///
/// # Example
///
/// ```rust
/// let data = JsonB(json!({"id": "user_123", "name": "Alice"}));
/// let id = extract_jsonb_id(&data, "id")?;
/// assert_eq!(id, Some("user_123".to_string()));
/// ```
#[allow(dead_code)]  // Phase 1: Will be integrated in Phase 2+
pub fn extract_jsonb_id(data: &JsonB, id_key: &str) -> spi::Result<Option<String>> {
    // Validate id_key to prevent SQL injection
    crate::validation::validate_sql_identifier(id_key, "id_key")?;

    // Check if jsonb_ivm is available
    let has_jsonb_ivm = Spi::get_one::<bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_proc WHERE proname = 'jsonb_extract_id')"
    )?.unwrap_or(false);

    if has_jsonb_ivm {
        // Use optimized jsonb_ivm function with parameterized id_key
        let sql = "SELECT jsonb_extract_id($1::jsonb, $2::text)";
        Spi::get_one_with_args::<String>(
            sql,
            &[
                unsafe { DatumWithOid::new(JsonB(data.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(id_key.to_string(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            ][..],
        )
    } else {
        // Fallback to standard operator (validated id_key is safe to interpolate)
        // Note: We still prefer parameterized but PostgreSQL doesn't support
        // parameterized identifiers in ->> operator, so we use validated string
        let sql = format!("SELECT $1::jsonb->>'{}'", id_key);
        Spi::get_one_with_args::<String>(
            &sql,
            &[unsafe { DatumWithOid::new(JsonB(data.0.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) }][..],
        )
    }
}
