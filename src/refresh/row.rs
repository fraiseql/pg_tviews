//! Recompute one TVIEW row from its backing view.
//!
//! A queued key is refreshed by an upsert from the backing view: the row the view
//! produces for that key is written whole (its document included), and a row the
//! view no longer produces is deleted. The upsert's guard skips a row whose
//! columns are all unchanged, so a no-op refresh writes nothing.
//!
//! Patching a document in place, without recomputing it, happens elsewhere and
//! needs `jsonb_delta`: the direct patch of a TVIEW's own columns
//! (`refresh/direct.rs`) and the fan-out patch of a mapped table's columns
//! (`delta.rs`). Parents are found and refreshed by the flush (`src/flush/`).

use pgrx::prelude::*;

use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;

use crate::jsonb_delta::jsonb_delta_schema;
use crate::utils::{qualified_relname_from_oid, quote_identifier};

/// Refresh a single TVIEW row when its source data changes.
///
/// Recomputes the row from the backing view and writes it to the TVIEW's table. Does **not** propagate to parent TVIEWs;
/// propagation is handled by the transaction-level queue (`src/queue/`).
///
/// # Workflow
///
/// 1. **Lock**: wait for a concurrent writer of the row (READ COMMITTED)
/// 2. **Recompute**: read the row from the backing view
/// 3. **Write**: upsert it, or delete the row when the view has none
///
/// # Arguments
///
/// * `meta` - The TVIEW
/// * `key` - Identity value of the row (ADR 0169)
///
/// # Returns
///
/// The rows refreshed, as parents look them up.
///
/// # Errors
///
/// - Update to `tv_entity` table failed
pub fn refresh_key(meta: &TviewMeta, key: &KeyValue) -> crate::TViewResult<super::Touched> {
    let keys = std::slice::from_ref(key);
    // Wait for a concurrent writer of this row before reading the view.
    let before = super::lock_rows(
        meta,
        &qualified_relname_from_oid(meta.tview_oid)?,
        keys,
        !meta.identity.is_pk(&meta.entity_name),
    )?;

    // A UNION view can return several rows for one key: read it first so the
    // union_duplicate_policy applies before the upsert.
    let (written, deleted) = if meta.plan.set_operation && !view_row_exists(meta, key)? {
        (super::Written::default(), delete_tview_row(meta, key)?)
    } else {
        // Upsert straight from v_entity: the view is evaluated once. No
        // source row means the base row was deleted, so remove the tview row
        // instead of erroring, which would leave the deleted row stale.
        crate::metrics::metrics_api::record_view_recomputes(1);
        let (produced, written) = write_row(meta, key)?;
        let deleted = if produced == 0 {
            delete_tview_row(meta, key)?
        } else {
            Vec::new()
        };
        (written, deleted)
    };
    Ok(super::touched(meta, keys, before, written, deleted))
}

/// Delete the tview row of a key whose backing-view row has disappeared, and
/// return its `pk_<entity>`.
///
/// Removing the row here is what makes DELETE propagate to the tview instead of
/// leaving a stale row.
fn delete_tview_row(meta: &TviewMeta, key: &KeyValue) -> crate::TViewResult<Vec<i64>> {
    let key_type = meta.key_type()?;
    let qi_tv = qualified_relname_from_oid(meta.tview_oid)?;
    let qi_key = quote_identifier(&meta.identity.column);
    let qi_pk = quote_identifier(&format!("pk_{}", meta.entity_name));
    let sql = format!(
        "DELETE FROM {qi_tv} WHERE {qi_key} = {} \
         RETURNING {qi_pk}::text, to_jsonb({qi_tv}.*)->>'id'",
        super::key_cast(&key_type, "$1", false)
    );
    super::run_journaled_delete(
        &meta.entity_name,
        &sql,
        &[super::key_scalar(&key_type, key)?],
    )
}

/// Whether the backing view still has a row for `key`, applying the
/// `union_duplicate_policy` when a UNION view returns several.
///
/// # Example Query
///
/// ```sql
/// SELECT 1 FROM v_post WHERE pk_post = $1 LIMIT 2
/// ```
fn view_row_exists(meta: &TviewMeta, key: &KeyValue) -> crate::TViewResult<bool> {
    let key_type = meta.key_type()?;
    let qi_view = qualified_relname_from_oid(meta.view_oid)?;

    let sql = format!(
        "SELECT 1 FROM {qi_view} WHERE {} = {} LIMIT 2",
        quote_identifier(&meta.identity.column),
        super::key_cast(&key_type, "$1", false)
    );

    Spi::connect(|client| {
        let args = [super::key_scalar(&key_type, key)?];
        let mut rows = client.select(&sql, None, &args)?;

        // No backing-view row for this key means the base row was deleted (or now
        // fails the view's WHERE/branch conditions). This is not an error: the
        // caller removes the corresponding tview row.
        if rows.next().is_none() {
            return Ok(false);
        }

        // For UNION ALL TVIEWs, check for duplicate rows (non-mutually-exclusive branches)
        if meta.plan.set_operation && rows.next().is_some() {
            union_duplicate(meta, &key.to_string());
        }

        Ok(true)
    })
}

/// Apply `pg_tviews.union_duplicate_policy` to a UNION TVIEW whose backing view
/// returned several rows for the key `key`: an ERROR that aborts the write,
/// or under `first` a note, once per backend, that the first row is kept.
pub(super) fn union_duplicate(meta: &TviewMeta, key: &str) {
    if crate::config::union_duplicate_policy() == "first" {
        crate::utils::log_once(
            &format!("union_duplicate:{}", meta.entity_name),
            &format!(
                "TVIEW '{}': UNION ALL backing view returned multiple rows for {}={key}; \
                 taking the first row (union_duplicate_policy=first). Reported once \
                 per backend.",
                meta.entity_name, meta.identity.column
            ),
        );
        return;
    }
    // Raised, not returned: the flush trigger turns returned errors into warnings,
    // and a write that gives two rows one key must fail.
    pgrx::pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_CARDINALITY_VIOLATION,
        format!(
            "TVIEW '{}': UNION ALL backing view returned multiple rows for {}={key}",
            meta.entity_name, meta.identity.column
        ),
        function_name!(),
    )
    .set_hint(
        "Make the UNION branches' keys disjoint (a sign or an offset per branch), or set \
         pg_tviews.union_duplicate_policy = 'first' to keep the first row.",
    )
    .report(PgLogLevel::ERROR);
}

/// Write the row `key` of `meta`'s TVIEW from its backing view, its document
/// replaced whole: the view computed all of it, and a merge into the stored
/// document would keep keys the view no longer produces, or a NULL it replaced.
/// Returns the rows the view produced and what was written.
///
/// Without `jsonb_delta`, writes still refresh every TVIEW, but none is patched
/// in place: said once per backend, in the server log.
///
/// # Errors
/// What the upsert returns.
fn write_row(meta: &TviewMeta, key: &KeyValue) -> crate::TViewResult<(i64, super::Written)> {
    if jsonb_delta_schema().is_none() {
        crate::utils::log_once(
            crate::jsonb_delta::JSONB_DELTA_MISSING,
            "jsonb_delta is not installed: writes recompute TVIEW rows instead of patching \
             them in place. CREATE EXTENSION jsonb_delta to enable the direct and fan-out \
             patches.",
        );
    }
    let key_type = meta.key_type()?;
    let qi_tv = qualified_relname_from_oid(meta.tview_oid)?;
    let key_col = &meta.identity.column;
    let qi_key = quote_identifier(key_col);

    // Schema-qualified backing view, so the refresh works under any search_path
    let qi_view = qualified_relname_from_oid(meta.view_oid)?;

    // Get view column names (authoritative list of data columns; excludes timestamps)
    let col_names = crate::utils::get_view_columns_by_oid(meta.view_oid)?;

    let col_list = super::column_list(&col_names);

    // UPSERT: INSERT from view (timestamps use DEFAULT NOW()), or UPDATE on conflict
    // when a column actually changed. This handles both new rows (inserted into
    // the base table after TVIEW creation) and existing rows that need refreshing.
    super::run_counted_upsert(
        &meta.entity_name,
        &qi_tv,
        &col_list,
        &format!(
            "SELECT {col_list} FROM {qi_view} WHERE {qi_key} = {}",
            super::key_cast(&key_type, "$1", false)
        ),
        &format!(
            "ON CONFLICT ({qi_key}) {}",
            super::upsert_conflict_action(&qi_tv, &col_names, key_col)
        ),
        &[super::key_scalar(&key_type, key)?],
    )
}
