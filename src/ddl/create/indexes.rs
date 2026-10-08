//! The indexes `pg_tviews` creates on a TVIEW's table.

use super::ViewColumns;
use crate::error::TViewError;
use crate::error::TViewResult;
use crate::utils::quote_identifier;

/// Deterministic index name `idx_<tview>_<suffix>`, fitted to 63 bytes by
/// [`crate::utils::fit_identifier`].
pub(crate) fn index_name(tview_name: &str, suffix: &str) -> String {
    crate::utils::fit_identifier(format!("idx_{tview_name}_{suffix}"))
}

/// DDL for the required propagation index `(fk, pk)` on a TVIEW.
///
/// Cascade propagation (`src/propagate.rs`) looks up parent rows with
/// `SELECT fk, pk FROM tv WHERE fk = ANY($1)`; this index makes that lookup
/// index-only instead of a scan of the whole TVIEW.
pub(crate) fn propagation_index_ddl(
    schema_name: &str,
    tview_name: &str,
    fk: &str,
    pk: &str,
) -> String {
    index_ddl(
        schema_name,
        tview_name,
        &format!("{fk}_{pk}"),
        "",
        &[fk, pk],
    )
}

/// `CREATE INDEX IF NOT EXISTS idx_<tview>_<suffix> ON schema.tview <method>(cols)`.
pub(crate) fn index_ddl(
    schema_name: &str,
    tview_name: &str,
    suffix: &str,
    method: &str,
    columns: &[&str],
) -> String {
    let cols = columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE INDEX IF NOT EXISTS {} ON {}.{} {method}({cols})",
        quote_identifier(&index_name(tview_name, suffix)),
        quote_identifier(schema_name),
        quote_identifier(tview_name),
    )
}

/// Create indexes on the materialized table for optimal query performance
pub(crate) fn create_tview_indexes(
    tview_name: &str,
    schema: &ViewColumns,
    schema_name: &str,
    data_gin: bool,
) -> TViewResult<()> {
    let ddl = tview_index_ddl(tview_name, schema, schema_name, data_gin);
    for create_idx in ddl {
        crate::utils::spi_run_ddl(&create_idx).map_err(|e| TViewError::SpiError {
            query: create_idx.clone(),
            error: e,
        })?;
    }
    Ok(())
}

/// Names of the indexes `pg_tviews` creates on `tview_name` for `schema` (the
/// `data` GIN index included) and for the columns joined to the aggregate TVIEWs it
/// embeds: the indexes a rebuild does not carry over as a user's (issue #134).
pub(crate) fn managed_index_names(
    tview_name: &str,
    schema: &ViewColumns,
    embed_columns: &[String],
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    if let Some(id) = &schema.id {
        names.insert(index_name(tview_name, id));
    }
    for uuid_fk in &schema.uuid_fk {
        names.insert(index_name(tview_name, uuid_fk));
    }
    if let Some(pk) = &schema.pk {
        for column in schema.fk.iter().chain(embed_columns) {
            if column != pk {
                names.insert(index_name(tview_name, &format!("{column}_{pk}")));
            }
        }
    }
    if let Some(data) = &schema.data {
        names.insert(index_name(tview_name, &format!("{data}_gin")));
    }
    names
}

/// DDL for every index a new TVIEW gets.
///
/// HOT invariant: refreshes rewrite `data` and `updated_at`, so neither is indexed
/// (the `data` GIN only when `data_gin` is explicitly requested). An index on a
/// rewritten column makes every refresh a non-HOT update: new entries in every
/// index, a dead tuple needing index cleanup, and a cleared visibility-map bit.
pub(crate) fn tview_index_ddl(
    tview_name: &str,
    schema: &ViewColumns,
    schema_name: &str,
    data_gin: bool,
) -> Vec<String> {
    let mut ddl = Vec::new();

    // Trinity identifier and UUID foreign keys (filtering by public id)
    if let Some(id) = &schema.id {
        ddl.push(index_ddl(schema_name, tview_name, id, "", &[id]));
    }
    for uuid_fk in &schema.uuid_fk {
        ddl.push(index_ddl(schema_name, tview_name, uuid_fk, "", &[uuid_fk]));
    }

    // Required propagation indexes (see `propagation_index_ddl`)
    if let Some(pk) = &schema.pk {
        for fk in schema.fk.iter().filter(|fk| *fk != pk) {
            ddl.push(propagation_index_ddl(schema_name, tview_name, fk, pk));
        }
    }

    // Opt-in (pg_tviews.data_gin_index): top-level containment queries on data
    if data_gin && let Some(data) = &schema.data {
        ddl.push(index_ddl(
            schema_name,
            tview_name,
            &format!("{data}_gin"),
            "USING GIN ",
            &[data],
        ));
    }

    ddl
}

/// `WITH (fillfactor = N)` for a new TVIEW table; empty at 100 (the heap default),
/// so opting out yields the same DDL as before the setting existed.
pub(crate) fn storage_clause(fillfactor: i32) -> String {
    if fillfactor < 100 {
        format!(" WITH (fillfactor = {fillfactor})")
    } else {
        String::new()
    }
}

/// Index each embed lookup column that is neither the TVIEW's primary
/// key nor already indexed as an `fk_*` propagation column, so propagation from
/// the embedded TVIEW does not scan the whole TVIEW.
pub(crate) fn create_embed_lookup_indexes(
    lookups: &[String],
    schema: &ViewColumns,
    tview_name: &str,
    schema_name: &str,
) -> TViewResult<()> {
    let Some(pk) = &schema.pk else {
        return Ok(());
    };
    let columns: std::collections::BTreeSet<&String> = lookups
        .iter()
        .filter(|c| *c != pk && !schema.fk.contains(c))
        .collect();
    for column in columns {
        let ddl = propagation_index_ddl(schema_name, tview_name, column, pk);
        crate::utils::spi_run_ddl(&ddl).map_err(|e| TViewError::SpiError {
            query: ddl.clone(),
            error: e,
        })?;
    }
    Ok(())
}
