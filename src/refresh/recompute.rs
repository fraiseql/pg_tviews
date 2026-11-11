//! Recompute TVIEW rows from their backing view, by identity (ADR 0169).
//!
//! A queued key is refreshed by an upsert from the backing view: the row the view
//! produces for it is written whole (its document included), and a row the view
//! no longer produces is deleted. The upsert's guard skips a row whose columns are
//! all unchanged, so a no-op refresh writes nothing. One key or many take the same
//! statements: the keys are looked up by their count's shape (`key_lookup`), a
//! single one by equality.
//!
//! Patching a document in place, without recomputing it, is `direct` (a TVIEW's
//! own columns) and the fan-out patch (`delta/mapping.rs`); every row at once is
//! `full`. Parents are found and refreshed by the flush (`src/flush/`).

use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;

/// Recompute the rows of `meta`'s TVIEW named by `keys` and write them: one
/// upsert, and a delete of the keys the view no longer produces, per batch of
/// [`super::BATCH_KEYS`] keys. Waits for concurrent writers of the rows first.
/// Does not propagate to parent TVIEWs: the flush does (`src/flush/`).
///
/// # Returns
///
/// The rows refreshed, as parents look them up.
///
/// # Errors
///
/// What the lock, the upsert or the delete returns.
pub fn recompute(
    frame: &crate::owner::OwnerFrame,
    meta: &TviewMeta,
    keys: &[KeyValue],
) -> TViewResult<super::Touched> {
    frame.require_owner_of(meta.tview_oid)?;
    if keys.is_empty() {
        return Ok(super::Touched::default());
    }

    // Count the backing-view recompute of these rows.
    crate::stats::record_view_recomputes(&meta.entity_name, keys.len() as u64);

    // Resolve the schema-qualified backing view + tview and the authoritative
    // data-column list.
    let qi_view = crate::utils::qualified_relname_from_oid(meta.view_oid)?;
    let qi_tv = crate::utils::qualified_relname_from_oid(meta.tview_oid)?;
    let key_col = &meta.identity.column;
    let key_type = &meta.key_type;

    let col_names = &meta.view_columns;
    if col_names.is_empty() {
        return Ok(super::Touched::default());
    }
    let col_list = super::column_list(col_names);

    // UPSERT every requested row that still resolves in the backing view. Rows not
    // yet materialized are inserted rather than silently skipped — the old
    // `UPDATE … FROM unnest()` path dropped every not-yet-present row.
    // Rows whose recomputed columns equal the stored ones are left alone.
    // The filter is on the identity, bound with its type, so it reaches the base
    // tables' indexes through the view.
    let qi_key = crate::utils::ident::quoted(key_col);
    let qi_pk = crate::utils::ident::quoted(meta.key_column());
    let any_key = format!("ANY({})", super::key_cast(key_type, "$1", true));
    let conflict = format!(
        "ON CONFLICT ({qi_key}) {}",
        super::upsert_conflict_action(&qi_tv, col_names, key_col)
    );

    // DELETE the rows of the keys the upsert's source no longer produced (deleted
    // base rows), by their identities: the view is not read again. Asking it
    // (`NOT EXISTS (SELECT 1 FROM view v WHERE v.key = t.key)`) evaluated it a
    // second time, and the correlation does not reach below a GROUP BY: an
    // aggregate TVIEW computed every group (#237). Nothing changes these keys'
    // rows in between: `lock_rows` locked the existing ones (FOR UPDATE) and the
    // missing keys, so another refresh of them waits for this transaction.
    let delete_sql = format!(
        "DELETE FROM {qi_tv} t \
         WHERE t.{qi_key} = {any_key} AND t.{qi_key} <> ALL({}) \
         RETURNING t.{qi_pk}::pg_catalog.int8, {}",
        super::key_cast(key_type, "$2", true),
        meta.returned("t", false)
    );

    // Chunk very large multi-row changes into batches (`BATCH_KEYS`) so a
    // single statement never carries an unbounded key array. Each batch's UPSERT and
    // stale-DELETE are keyed on that batch's keys; the DELETE only affects tview rows
    // whose key is in the batch, so per-batch execution stays correct.
    let mut touched = super::Touched::default();
    for chunk in keys.chunks(crate::refresh::BATCH_KEYS) {
        // Wait for concurrent writers of these rows before reading the view.
        let locked = super::lock_rows(frame, meta, &qi_tv, chunk)?;
        // The keys take their shape (`key_lookup`): the upsert is planned by
        // PostgreSQL's own rule, which settles on its generic plan, and a
        // generic plan of an array parameter reads the view for ten keys.
        let Some(lookup) = super::key_lookup(key_type, chunk)? else {
            continue;
        };
        let source_sql = format!(
            "SELECT {col_list} FROM {qi_view} WHERE {qi_key} = {}",
            lookup.rhs
        );
        let into = super::UpsertInto {
            table: &qi_tv,
            columns: &col_list,
            conflict: &conflict,
        };
        let produced = super::run_counted_upsert(
            frame,
            meta,
            &into,
            &source_sql,
            &lookup.args,
            locked.old.as_ref(),
        )?;
        // A key produces at most one row (the upsert refuses duplicates): as many
        // rows as keys means none is gone.
        let deleted = if produced.keys.len() == chunk.len() {
            Vec::new()
        } else {
            super::run_journaled_delete(
                frame,
                meta,
                &delete_sql,
                &[
                    super::key_array(key_type, chunk)?,
                    super::produced_keys(key_type, produced.keys)?,
                ],
            )?
        };
        touched.extend(super::touched(
            meta,
            chunk,
            locked.pks,
            produced.written,
            deleted,
        ));
    }

    Ok(touched)
}
