//! Cascade refresh: propagating base-table changes to dependent TVIEWs.

use crate::catalog;
use crate::queue;
use crate::utils::{self, quote_identifier};
use pgrx::prelude::*;

/// Cascade refresh when a base table row changes
/// Called by trigger handler when INSERT/UPDATE/DELETE occurs on base tables
///
/// Arguments:
/// - `base_table_oid`: OID of the base table that changed
/// - `pk_value`: Primary key value of the changed row
#[pg_extern]
fn pg_tviews_cascade(base_table_oid: pg_sys::Oid, pk_value: i64) {
    crate::revision::check();
    let dependent_tviews = match find_dependent_tviews(base_table_oid) {
        Ok(tv) => tv,
        Err(e) => error!("Failed to find dependent TVIEWs: {}", e),
    };

    if dependent_tviews.is_empty() {
        return;
    }

    for tview_meta in dependent_tviews {
        let affected_rows = match find_affected_tview_rows(&tview_meta, base_table_oid, pk_value) {
            Ok(rows) => rows,
            Err(e) => {
                warning!(
                    "Failed to find affected rows in {}: {:?}",
                    tview_meta.entity_name,
                    e
                );
                continue;
            }
        };

        if affected_rows.is_empty() {
            continue;
        }

        // The rows are found by pk_<entity>; a TVIEW keyed on another column
        // (ADR 0169) is refreshed whole.
        if !tview_meta.identity.is_pk(&tview_meta.entity_name) {
            queue::enqueue_refresh_all(&tview_meta.entity_name);
            continue;
        }
        for affected_pk in affected_rows {
            queue::enqueue_refresh(
                &tview_meta.entity_name,
                queue::key::KeyValue::Int(affected_pk),
            );
        }
    }

    // Outside a transaction block no flush trigger follows this statement and the
    // commit drops what is queued: refresh now. Inside one, the work stays queued
    // for the next flush (a later statement, COMMIT or PREPARE TRANSACTION).
    // SAFETY: reads the backend's transaction state.
    if !unsafe { pg_sys::IsTransactionBlock() }
        && let Err(e) = queue::flush_refresh_queue()
    {
        error!("TVIEW refresh failed in pg_tviews_cascade: {e}");
    }
}

/// Handle INSERT operations on base tables
/// Called by trigger handler when rows are inserted
#[pg_extern]
fn pg_tviews_insert(base_table_oid: pg_sys::Oid, pk_value: i64) {
    crate::revision::check();
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Handle DELETE operations on base tables
/// Called by trigger handler when rows are deleted
#[pg_extern]
fn pg_tviews_delete(base_table_oid: pg_sys::Oid, pk_value: i64) {
    crate::revision::check();
    pg_tviews_cascade(base_table_oid, pk_value);
}

/// Find all TVIEWs that have the given base table as a dependency
fn find_dependent_tviews(
    base_table_oid: pg_sys::Oid,
) -> crate::TViewResult<Vec<catalog::TviewMeta>> {
    // The shared column list keeps this loader in step with `TviewMeta::from_spi_row`.
    let query = format!(
        "{} WHERE $1 IN (SELECT (cp::jsonb->>'source_oid')::oid FROM unnest(cascade_paths) AS cp) \
            OR $1 IN (SELECT (e->>'relid')::oid FROM jsonb_array_elements(key_mappings) AS e \
                      WHERE e->>'kind' IN ('local', 'mapped'))",
        catalog::meta_select()
    );
    Spi::connect(|client| -> crate::TViewResult<_> {
        // SAFETY: the oid datum is passed by value for the duration of the select.
        let args = [unsafe {
            pgrx::datum::DatumWithOid::new(
                base_table_oid,
                PgOid::BuiltIn(PgBuiltInOids::OIDOID).value(),
            )
        }];
        let rows = client.select(&query, None, &args)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(catalog::TviewMeta::from_spi_row(&row)?);
        }
        Ok(result)
    })
}

/// Find rows in a TVIEW that reference a specific base table row
fn find_affected_tview_rows(
    tview_meta: &catalog::TviewMeta,
    base_table_oid: pg_sys::Oid,
    base_pk: i64,
) -> crate::TViewResult<Vec<i64>> {
    let base_table_name = crate::utils::spi_get_string(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {base_table_oid:?}"
    ))?
    .ok_or(spi::Error::InvalidPosition)?;

    let base_entity = base_table_name.trim_start_matches("tb_");

    let qi_view = utils::qualified_relname_from_oid(tview_meta.view_oid)?;
    let tview_pk_col = format!("pk_{}", tview_meta.entity_name);

    // The backing view may call its owner's functions: read it as the owner.
    let _owner = crate::owner::AsOwner::of_table(tview_meta.tview_oid)?;
    let collect_pks = |query: &str| -> crate::TViewResult<Vec<i64>> {
        let col = tview_pk_col.clone();
        Spi::connect(|client| {
            let rows = client.select(query, None, &[])?;
            let mut pks = Vec::new();
            for row in rows {
                if let Some(pk) = row[col.as_str()].value::<i64>()? {
                    pks.push(pk);
                }
            }
            Ok::<_, crate::TViewError>(pks)
        })
    };

    let qi_pk_col = quote_identifier(&tview_pk_col);

    // Case 1: Direct match
    if tview_meta.entity_name == base_entity {
        let query = format!("SELECT {qi_pk_col} FROM {qi_view} WHERE {qi_pk_col} = {base_pk}");
        return collect_pks(&query);
    }

    // Case 2: Scalar FK
    let fk_col = format!("fk_{base_entity}");
    if tview_meta.fk_columns.contains(&fk_col) {
        let qi_fk = quote_identifier(&fk_col);
        let query = format!("SELECT {qi_pk_col} FROM {qi_view} WHERE {qi_fk} = {base_pk}");
        return collect_pks(&query);
    }

    // Case 3: Array aggregation
    let fk_in_base = format!("fk_{}", tview_meta.entity_name);
    let pk_in_base = format!("pk_{base_entity}");

    let lookup_query = format!(
        "SELECT DISTINCT {} AS {qi_pk_col} \
         FROM {} \
         WHERE {} = {base_pk}",
        quote_identifier(&fk_in_base),
        utils::qualified_relname_from_oid(base_table_oid)?,
        quote_identifier(&pk_in_base),
    );
    let pks = collect_pks(&lookup_query)?;
    if !pks.is_empty() {
        return Ok(pks);
    }

    // DELETE fallback: refresh all rows in the materialized TVIEW
    let fallback_query = format!(
        "SELECT {qi_pk_col} FROM {}",
        utils::qualified_relname_from_oid(tview_meta.tview_oid)?
    );
    collect_pks(&fallback_query)
}
