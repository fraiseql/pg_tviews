//! Phase 9B: Bulk Refresh API
//!
//! Provides efficient refresh of multiple rows in a single operation.
//! Reduces query count from N queries to 2 queries for N rows.

use pgrx::prelude::*;
use pgrx::spi;
use pgrx::JsonB;
use crate::catalog::TviewMeta;
use crate::utils::lookup_view_for_source;
use crate::TViewResult;

/// Refresh multiple rows of the same entity in a single operation
///
/// This is the bulk refresh API that replaces individual refresh_pk() calls
/// for statement-level triggers and other bulk operations.
///
/// # Arguments
///
/// * `entity` - Entity name (e.g., "post", "user")
/// * `pks` - Vector of primary key values to refresh
///
/// # Returns
///
/// Result indicating success or failure
///
/// # Performance
///
/// - **Individual refresh**: N queries (1 SELECT + 1 UPDATE per row)
/// - **Bulk refresh**: 2 queries (1 SELECT + 1 UPDATE for all rows)
/// - **Speedup**: 100-500× fewer queries (workload-dependent)
///
/// # Example
///
/// ```rust
/// // Instead of:
/// for pk in &[1, 2, 3, 4, 5] {
///     refresh_pk(source_oid, *pk)?;
/// }
///
/// // Use:
/// refresh_bulk("post", vec![1, 2, 3, 4, 5])?;
/// ```
pub fn refresh_bulk(entity: &str, pks: Vec<i64>) -> TViewResult<()> {
    if pks.is_empty() {
        return Ok(());
    }

    // Load metadata once
    let meta = TviewMeta::load_by_entity(entity)?
        .ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;

    // Recompute ALL rows in a single query using parameterized ANY($1)
    let view_name = lookup_view_for_source(meta.view_oid)?;
    let pk_col = format!("pk_{}", entity);

    // SAFE: Use ANY($1) with array parameter (prevents SQL injection)
    let query = format!(
        "SELECT * FROM {} WHERE {} = ANY($1)",
        quote_identifier(&view_name),
        quote_identifier(&pk_col)
    );

    Spi::connect(|mut client| {
        // Create PostgreSQL BIGINT[] array from Vec<i64>
        let rows = client.select(
            &query,
            None,
            Some(vec![(
                PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID),
                pks.clone().into_datum()
            )]),
        )?;

        // Batch update using UPDATE ... FROM unnest()
        let tv_name = relname_from_oid(meta.tview_oid)?;

        // Collect data for update
        let mut update_pks: Vec<i64> = Vec::new();
        let mut update_data: Vec<JsonB> = Vec::new();

        for row in rows {
            let pk: i64 = row[&pk_col as &str].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "".to_string(),
                    error: format!("{} column is NULL", pk_col),
                }))?;
            let data: JsonB = row["data"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: "".to_string(),
                    error: "data column is NULL".to_string(),
                }))?;
            update_pks.push(pk);
            update_data.push(data);
        }

        if update_pks.is_empty() {
            return Ok(()); // No rows to update
        }

        // SAFE: Single UPDATE with unnest() (parameterized)
        let update_query = format!(
            "UPDATE {}
             SET data = v.data, updated_at = now()
             FROM (
                 SELECT unnest($1::bigint[]) as pk,
                        unnest($2::jsonb[]) as data
             ) AS v
             WHERE {}.{} = v.pk",
            quote_identifier(&tv_name),
            quote_identifier(&tv_name),
            quote_identifier(&pk_col)
        );

        // Execute batch update with parameters
        client.update(
            &update_query,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID), update_pks.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::JSONBARRAYOID), update_data.into_datum()),
            ]),
        )?;

        Ok(())
    })
}

/// Helper: Quote identifier safely
pub fn quote_identifier(name: &str) -> String {
    // Use PostgreSQL's quote_ident() for safety
    match Spi::get_one_with_args::<String>(
        "SELECT quote_ident($1)",
        vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), name.into_datum())],
    ) {
        Ok(Some(quoted)) => quoted,
        _ => format!("\"{}\"", name.replace("\"", "\"\"")),
    }
}

/// Helper: Look up TVIEW table name given its OID
fn relname_from_oid(oid: pg_sys::Oid) -> spi::Result<String> {
    Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {oid:?}"
    ))?
    .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
        query: format!("SELECT relname FROM pg_class WHERE oid = {oid:?}"),
        error: "No pg_class entry found".to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_bulk_empty() {
        // Empty PK list should succeed without doing anything
        assert!(refresh_bulk("test", vec![]).is_ok());
    }

    #[test]
    fn test_quote_identifier() {
        // Test basic identifier quoting
        let result = quote_identifier("test_table");
        assert_eq!(result, "\"test_table\"");

        // Test identifier with special characters
        let result = quote_identifier("test-table");
        assert_eq!(result, "\"test-table\"");
    }
}