//! # Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_delta` extension
//! for 1.5-3× performance improvement on cascade updates.
//!
//! ## Architecture
//!
//! 1. **Detect Change**: Trigger on source table → enqueues the row's identity value
//! 2. **Recompute Row**: Query `v_entity` to get fresh JSONB data
//! 3. **Smart Patch**: Use dependency metadata to apply surgical JSONB updates
//! 4. **Propagate**: Cascade to parent entities via FK relationships
//!
//! ## Smart Patching Strategy
//!
//! The `apply_patch()` function dispatches to different `jsonb_delta` functions based
//! on dependency type metadata:
//!
//! | Dependency Type | `jsonb_delta` Function | Use Case |
//! |-----------------|-------------------|----------|
//! | `nested_object` | `jsonb_smart_patch_nested(data, patch, path)` | Author/category objects |
//! | `array` | `jsonb_smart_patch_array(data, patch, path, key)` | Comments/tags arrays |
//! | `scalar` | `jsonb_smart_patch_scalar(data, patch)` | Unused FKs |
//!
//! ## Performance Impact
//!
//! - **Without `jsonb_delta`**: Full document replacement (~870ms for 100-row cascade)
//! - **With `jsonb_delta`**: Surgical updates (~400-600ms for 100-row cascade)
//! - **Speedup**: 1.45× to 2.2× faster
//!
//! ## Fallback Behavior
//!
//! If `jsonb_delta` is not installed, falls back to full replacement (slower but functional).
//!
//! ## Example
//!
//! ```sql
//! -- Create TVIEW with nested author
//! SELECT pg_tviews_create('post', $$
//!     SELECT pk_post, fk_user,
//!            jsonb_build_object('title', title, 'author', v_user.data) AS data
//!     FROM tb_post
//!     LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
//! $$);
//!
//! -- Update author name
//! UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
//!
//! -- Cascade uses jsonb_smart_patch_nested() to update only 'author' path
//! -- Original: UPDATE tv_post SET data = $1 (full replacement)
//! -- Optimized: UPDATE tv_post SET data = jsonb_smart_patch_nested(data, $1, '{author}')
//! ```

use pgrx::prelude::*;

use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;

use crate::jsonb_delta::jsonb_delta_schema;
use crate::utils::{qualified_relname_from_oid, quote_identifier};

/// Refresh a single TVIEW row when its source data changes.
///
/// Recomputes data from the backing view and applies smart JSONB patching
/// to the materialized table. Does **not** propagate to parent TVIEWs;
/// propagation is handled by the transaction-level queue (`src/queue/`).
///
/// # Workflow
///
/// 1. **Lock**: wait for a concurrent writer of the row (READ COMMITTED)
/// 2. **Recompute Row**: Query `v_entity` view for fresh JSONB data
/// 3. **Apply Patch**: Use smart JSONB patching to update `tv_entity` table
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
        let (produced, written) = apply_patch(meta, key)?;
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

/// Write the row `key` of `meta`'s TVIEW from its backing view, merging the new
/// document into the stored one with `jsonb_smart_patch_scalar` when that is
/// exact; returns the rows the view produced and what was written.
///
/// A TVIEW that embeds no other one, or embeds a document (nested or in an
/// array), has its document replaced whole ([`apply_full_replacement`]): a path
/// patch would leave the entity's own columns outside that path stale. Without
/// `jsonb_delta` every refresh replaces the document.
///
/// # Errors
/// What the upsert returns.
fn apply_patch(meta: &TviewMeta, key: &KeyValue) -> crate::TViewResult<(i64, super::Written)> {
    let key_type = meta.key_type()?;
    let key_col = &meta.identity.column;

    // Check if jsonb_delta is available (cached after first session query)
    let Some(delta_schema) = jsonb_delta_schema() else {
        crate::utils::log_once(
            crate::jsonb_delta::JSONB_DELTA_MISSING,
            "jsonb_delta is not installed: smart JSONB patching is disabled and cascades \
             replace whole documents (about 2x slower). CREATE EXTENSION jsonb_delta to enable it.",
        );
        return apply_full_replacement(meta, key);
    };

    // A TVIEW embedding no other one: its document is replaced whole.
    if meta.plan.embeds.is_empty() {
        return apply_full_replacement(meta, key);
    }

    // Array and nested-object embeds cannot be
    // surgically patched correctly. A path-level smart patch only touches the
    // embed's sub-path, so a change to one of the entity's OWN columns —
    // recomputed correctly into `$1` but living outside the patched path — is
    // silently dropped. Recompute the whole row instead. Only scalar embeds stay on
    // the smart-patch path below, where the shallow `jsonb_smart_patch_scalar`
    // merge already carries own columns along.
    if !meta.plan.only_scalar_embeds() {
        return apply_full_replacement(meta, key);
    }

    // UPSERT rather than UPDATE: a smart patch only makes sense for a
    // row that already exists, but an INSERT into the base table has no tview row
    // to patch yet, so an UPDATE-only statement silently drops it. Insert the full
    // row from the backing view, or smart-patch the existing row on conflict.
    let col_names = crate::utils::get_view_columns_by_oid(meta.view_oid)?;
    if col_names.is_empty() {
        return apply_full_replacement(meta, key);
    }
    let col_list = super::column_list(&col_names);
    // Qualify the target column as `{tv}.data`: the INSERT … SELECT source relation
    // is in scope inside ON CONFLICT DO UPDATE, so a bare `data` is ambiguous.
    let qi_tv = qualified_relname_from_oid(meta.tview_oid)?;
    let qi_view = qualified_relname_from_oid(meta.view_oid)?;
    let qi_key = quote_identifier(key_col);
    // The patch source is the freshly computed document the upsert already read
    // from the view (`EXCLUDED.data`), so the view is evaluated once.
    let patch_expr =
        build_smart_patch_expr(&delta_schema, &format!("{qi_tv}.data"), "EXCLUDED.\"data\"");

    // $1 = the row's identity (selects the row to insert from the backing view). In the
    // DO UPDATE clause, `data` is patched in place while every other projected
    // column takes the backing view's value; the guard skips the write when
    // nothing changed.
    let conflict = format!(
        "ON CONFLICT ({qi_key}) {}",
        super::upsert_conflict_action(&qi_tv, &col_names, key_col, Some(&patch_expr))
    );

    super::run_counted_upsert(
        &meta.entity_name,
        &qi_tv,
        &col_list,
        &format!(
            "SELECT {col_list} FROM {qi_view} WHERE {qi_key} = {}",
            super::key_cast(&key_type, "$1", false)
        ),
        &conflict,
        &[super::key_scalar(&key_type, key)?],
    )
}

/// The shallow merge of the freshly computed document `source` into the TVIEW's
/// `data` (`base_data_expr`, which the caller qualifies as `tv_<entity>.data`, so
/// it is unambiguous inside `INSERT … SELECT … ON CONFLICT DO UPDATE`), with the
/// quoted `jsonb_delta` `schema`. Only a TVIEW whose embeds are all scalar gets
/// here: nested and array embeds are recomputed in full, since a path-level patch
/// would miss a change to the row's own columns.
fn build_smart_patch_expr(schema: &str, base_data_expr: &str, source: &str) -> String {
    format!("{schema}.jsonb_smart_patch_scalar({base_data_expr}, {source})")
}

/// Check if `jsonb_delta` extension is installed in the current database.
///
/// Queries `pg_extension` system catalog to detect if the smart patching functions
/// are available. Used to determine whether to use optimized patching or fall back
/// to full replacement.
///
/// # Returns
///
/// - `Ok(true)` if `jsonb_delta` extension is installed
/// - `Ok(false)` if extension is not found
/// - `Err` if query fails
///
/// # Example
///
/// ```sql
/// Fallback: Full JSONB replacement (legacy behavior).
///
/// Performs a complete document replacement instead of surgical patching.
/// This is the slower but more compatible approach, used in these scenarios:
///
/// - **`jsonb_delta` not installed**: Extension unavailable
/// - **Metadata missing**: Legacy TVIEW without dependency info
/// - **No dependencies**: TVIEW has no FK relationships
///
/// # Performance
///
/// This approach is ~2× slower than smart patching for cascades but maintains
/// backward compatibility and serves as a safety fallback.
///
/// # Arguments
///
/// * `row` - `ViewRow` with fresh data to write
///
/// # Returns
///
/// `Ok(())` if replacement succeeded, `Err` if update failed.
///
/// # Generated SQL
///
/// ```sql
/// UPDATE tv_entity
/// SET data = $1, updated_at = now()
/// WHERE pk_entity = $2
/// ```
fn apply_full_replacement(
    meta: &TviewMeta,
    key: &KeyValue,
) -> crate::TViewResult<(i64, super::Written)> {
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
            super::upsert_conflict_action(&qi_tv, &col_names, key_col, None)
        ),
        &[super::key_scalar(&key_type, key)?],
    )
}

#[cfg(any(test, feature = "pg_test"))]
#[path = "row_tests.rs"]
mod row_tests;
