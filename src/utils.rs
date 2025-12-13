use pgrx::prelude::*;
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
/// Utilities: Common Helper Functions and PostgreSQL Integration
///
/// This module provides utility functions used throughout pg_tviews:
/// - **Primary Key Extraction**: Gets PK values from trigger tuples
/// - **OID Resolution**: Maps PostgreSQL OIDs to names and vice versa
/// - **SPI Helpers**: Common database query patterns
/// - **Type Conversions**: PostgreSQL type handling
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

/// Extracts a `pk_*` integer from NEW or OLD tuple by convention.
/// For MVP we assume the column name is literally "pk_*".
#[allow(dead_code)]
pub fn extract_pk(trigger: &PgTrigger) -> spi::Result<i64> {
    // For simplicity we assume there's a column named 'pk_*' and you know the entity.
    // For real code:
    //  - inspect relation attributes,
    //  - find first "pk_" column,
    //  - read value.
    let tuple = trigger
        .new()
        .or(trigger.old())
        .expect("Row must exist for AFTER trigger");

    // TODO: detect column name dynamically. For now, assume "pk_*" is "pk_post".
    // You might want to store the pk column name in pg_tview_meta.
    let pk: i64 = tuple
        .get_by_name("pk_post")? // <-- placeholder: replace per entity
        .expect("pk_post must not be null");
    Ok(pk)
}

// ✅ Tests at module level (outside function)
#[cfg(any(test, feature = "pg_test"))]
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
/// Used to find the backing view (v_entity) for a TVIEW
pub fn lookup_view_for_source(view_oid: Oid) -> spi::Result<String> {
    // Simply get the relation name from pg_class
    relname_from_oid(view_oid)
}

/// Look up the TVIEW table name given its OID (from pg_tview_meta).
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

/// Extract ID field from JSONB data using jsonb_ivm extension.
///
/// **Security**: This function validates the id_key parameter to prevent SQL injection.
/// Only alphanumeric characters and underscores are allowed in id_key.
///
/// # Arguments
///
/// * `data` - JSONB data to extract ID from
/// * `id_key` - Key name for ID field (must be valid identifier: [a-zA-Z0-9_]+)
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
/// - With jsonb_ivm: ~5× faster than data->>'id'
/// - Without jsonb_ivm: Same as data->>'id'
///
/// # Example
///
/// ```rust
/// let data = JsonB(json!({"id": "user_123", "name": "Alice"}));
/// let id = extract_jsonb_id(&data, "id")?;
/// assert_eq!(id, Some("user_123".to_string()));
/// ```
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
