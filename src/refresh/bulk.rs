//! Bulk Refresh API
//!
//! Provides efficient refresh of multiple rows in a single operation.
//! Reduces query count from N queries to 2 queries for N rows.

use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::utils::lookup_view_for_source;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

/// Refresh multiple rows of the same entity in a single operation
///
/// This is the bulk refresh API that replaces individual `refresh_pk()` calls
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
/// refresh_bulk("post", &[1, 2, 3, 4, 5])?;
/// ```
pub fn refresh_bulk(entity: &str, pks: &[i64]) -> TViewResult<()> {
    if pks.is_empty() {
        return Ok(());
    }

    // Load metadata once
    let meta =
        TviewMeta::load_by_entity(entity)?.ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;

    // Resolve backing view + tview names and the authoritative data-column list.
    let view_name = lookup_view_for_source(meta.view_oid)?;
    let tv_name = crate::utils::relname_from_oid(meta.tview_oid)?;
    let pk_col = format!("pk_{entity}");

    let col_names = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    if col_names.is_empty() {
        return Ok(());
    }
    let col_list = col_names.join(", ");

    // DO UPDATE SET: every non-pk column tracks the backing view; refresh timestamp.
    let do_update: String = {
        let mut parts = Vec::with_capacity(col_names.len());
        for c in &col_names {
            if c.as_str() != pk_col.as_str() {
                parts.push(format!("{c} = EXCLUDED.{c}"));
            }
        }
        parts.push("updated_at = NOW()".to_string());
        parts.join(", ")
    };

    // UPSERT every requested pk that still resolves in the backing view. Rows not
    // yet materialized are inserted rather than silently skipped — the old
    // `UPDATE … FROM unnest()` path dropped every not-yet-present row (issue #48).
    let upsert_sql = format!(
        "INSERT INTO {tv_name} ({col_list}) \
         SELECT {col_list} FROM {view_name} WHERE {pk_col} = ANY($1) \
         ON CONFLICT ({pk_col}) DO UPDATE SET {do_update}"
    );

    // DELETE tview rows whose backing-view row has disappeared (deleted base rows).
    // The old UPDATE-only path left these stale (issue #48).
    let delete_sql = format!(
        "DELETE FROM {tv_name} t \
         WHERE t.{pk_col} = ANY($1) \
           AND NOT EXISTS (SELECT 1 FROM {view_name} v WHERE v.{pk_col} = t.{pk_col})"
    );

    // Chunk very large multi-row changes into batches (pg_tviews.batch_size) so a
    // single statement never carries an unbounded pk array. Each batch's UPSERT and
    // stale-DELETE are keyed on that batch's pks; the DELETE only affects tview rows
    // whose pk is in the batch, so per-batch execution stays correct. SAFETY:
    // DatumWithOid wraps the validated BIGINT[] for SPI parameter passing.
    let batch = crate::config::batch_size();
    for chunk in pks.chunks(batch) {
        Spi::run_with_args(
            &upsert_sql,
            &[unsafe {
                DatumWithOid::new(
                    chunk.to_vec(),
                    PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
                )
            }],
        )?;
        Spi::run_with_args(
            &delete_sql,
            &[unsafe {
                DatumWithOid::new(
                    chunk.to_vec(),
                    PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
                )
            }],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_bulk_empty() {
        // Empty PK list should succeed without doing anything
        assert!(refresh_bulk("test", &[]).is_ok());
    }
}
