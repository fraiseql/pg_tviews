//! Bulk Refresh API
//!
//! Provides efficient refresh of multiple rows in a single operation.
//! Reduces query count from N queries to 2 queries for N rows.

use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;

/// Refresh multiple rows of the same entity in a single operation
///
/// This is the bulk refresh API that replaces individual `refresh_key()` calls
/// for statement-level triggers and other bulk operations. Rows are named by the
/// TVIEW's identity (ADR 0169).
///
/// # Arguments
///
/// * `entity` - Entity name (e.g., "post", "user")
/// * `keys` - Identity values of the rows to refresh
///
/// # Returns
///
/// The rows refreshed, as parents look them up.
///
/// # Performance
///
/// - **Individual refresh**: N queries (1 SELECT + 1 UPDATE per row)
/// - **Bulk refresh**: 2 queries (1 SELECT + 1 UPDATE for all rows)
/// - **Speedup**: 100-500× fewer queries (workload-dependent)
pub fn refresh_bulk(entity: &str, keys: &[KeyValue]) -> TViewResult<super::Touched> {
    if keys.is_empty() {
        return Ok(super::Touched::default());
    }

    // Count the backing-view recompute of these rows.
    crate::metrics::metrics_api::record_view_recomputes(keys.len() as u64);

    // Load metadata once
    let meta =
        TviewMeta::load_by_entity(entity)?.ok_or_else(|| crate::TViewError::MetadataNotFound {
            entity: entity.to_string(),
        })?;

    // Resolve the schema-qualified backing view + tview and the authoritative
    // data-column list.
    let qi_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let key_col = &meta.identity.column;
    let key_type = meta.key_type()?;

    let col_names = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    if col_names.is_empty() {
        return Ok(super::Touched::default());
    }
    let col_list = super::column_list(&col_names);

    // UPSERT every requested row that still resolves in the backing view. Rows not
    // yet materialized are inserted rather than silently skipped — the old
    // `UPDATE … FROM unnest()` path dropped every not-yet-present row.
    // Rows whose recomputed columns equal the stored ones are left alone.
    // The filter is on the identity, bound with its type, so it reaches the base
    // tables' indexes through the view.
    let qi_key = crate::utils::quote_identifier(key_col);
    let qi_pk = crate::utils::quote_identifier(&format!("pk_{entity}"));
    let any_key = format!("ANY({})", super::key_cast(&key_type, "$1", true));
    let source_sql = format!("SELECT {col_list} FROM {qi_view} WHERE {qi_key} = {any_key}");
    let conflict = format!(
        "ON CONFLICT ({qi_key}) {}",
        super::upsert_conflict_action(&qi_tv, &col_names, key_col)
    );

    // DELETE tview rows whose backing-view row has disappeared (deleted base rows).
    // The old UPDATE-only path left these stale.
    let delete_sql = format!(
        "DELETE FROM {qi_tv} t \
         WHERE t.{qi_key} = {any_key} \
           AND NOT EXISTS (SELECT 1 FROM {qi_view} v WHERE v.{qi_key} = t.{qi_key}) \
         RETURNING t.{qi_pk}::text, to_jsonb(t.*)->>'id'"
    );

    // Chunk very large multi-row changes into batches (pg_tviews.batch_size) so a
    // single statement never carries an unbounded key array. Each batch's UPSERT and
    // stale-DELETE are keyed on that batch's keys; the DELETE only affects tview rows
    // whose key is in the batch, so per-batch execution stays correct.
    let with_pks = !meta.identity.is_pk(entity);
    let mut touched = super::Touched::default();
    for chunk in keys.chunks(crate::config::batch_size()) {
        // Wait for concurrent writers of these rows before reading the view.
        let before = super::lock_rows(&meta, &qi_tv, chunk, with_pks)?;
        // A key names one row: a UNION view returning several for one is refused.
        super::refuse_duplicate_keys(
            &meta,
            &format!("{qi_key} = {any_key}"),
            &[super::key_array(&key_type, chunk)?],
        )?;
        let (_, written) = super::run_counted_upsert(
            entity,
            &qi_tv,
            &col_list,
            &source_sql,
            &conflict,
            &[super::key_array(&key_type, chunk)?],
        )?;
        let deleted = super::run_journaled_delete(
            entity,
            &delete_sql,
            &[super::key_array(&key_type, chunk)?],
        )?;
        touched.extend(super::touched(&meta, chunk, before, written, deleted));
    }

    Ok(touched)
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
