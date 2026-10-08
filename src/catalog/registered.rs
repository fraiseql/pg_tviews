//! Typed reads of every registered TVIEW at once.

use crate::error::TViewResult;
use pgrx::pg_sys::Oid;
use std::collections::HashSet;

/// What another TVIEW's registration says to the analysis of a new one.
#[derive(Debug, Clone)]
pub struct Registered {
    pub entity: String,
    pub table_oid: Oid,
    pub view_oid: Oid,
    /// The base tables a write to which is mapped to its keys (not `all_keys`).
    pub mapped: HashSet<u32>,
    /// The base tables a write to which refreshes it in full.
    pub full_refresh: HashSet<u32>,
    /// An aggregate TVIEW (declared group keys).
    pub aggregate: bool,
    /// Its rows are named by a column other than `pk_<entity>` (DISTINCT ON).
    pub keyed_otherwise: bool,
}

/// Every registered TVIEW, by entity.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn all() -> TViewResult<Vec<Registered>> {
    let sql = format!(
        "SELECT entity::text, table_oid::oid, view_oid::oid, key_mappings, \
                ARRAY(SELECT u::pg_catalog.oid FROM pg_catalog.unnest(uncascaded_oids) u \
                      WHERE COALESCE(uncascaded_table_policies[pg_catalog.array_position( \
                                uncascaded_table_oids, u)], uncascaded_policy) \
                            = 'full_refresh'), \
                group_keys IS NOT NULL, \
                COALESCE(identity->>'kind' = 'distinct_on', false) \
                    OR distinct_on_keys <> '{{}}' \
         FROM {} ORDER BY entity",
        crate::utils::meta_table()
    );
    let rows = crate::utils::spi::rows(&sql, &[], |row| {
        let (Some(entity), Some(table_oid), Some(view_oid)) = (
            row.get::<String>(1)?,
            row.get::<Oid>(2)?,
            row.get::<Oid>(3)?,
        ) else {
            return Ok(None);
        };
        let mapped = row
            .get::<pgrx::JsonB>(4)?
            .and_then(|j| j.0.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter(|e| e["kind"] != "all_keys")
            .filter_map(|e| e["relid"].as_u64().and_then(|r| u32::try_from(r).ok()))
            .collect();
        let full_refresh = row
            .get::<Vec<Oid>>(5)?
            .unwrap_or_default()
            .iter()
            .map(|oid| oid.to_u32())
            .collect();
        Ok(Some(Registered {
            entity,
            table_oid,
            view_oid,
            mapped,
            full_refresh,
            aggregate: row.get::<bool>(6)?.unwrap_or(false),
            keyed_otherwise: row.get::<bool>(7)?.unwrap_or(false),
        }))
    })?;
    Ok(rows.into_iter().flatten().collect())
}

/// The definition of `entity` and the output columns holding the keys of the
/// aggregate TVIEWs it embeds.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn definition_and_embed_columns(entity: &str) -> TViewResult<(String, Vec<String>)> {
    let sql = format!(
        "SELECT definition, aggregate_embeds FROM {} WHERE entity = $1",
        crate::utils::meta_table()
    );
    let mut rows = crate::utils::spi::rows(&sql, &[crate::utils::spi::text(entity)], |row| {
        Ok((row.get::<String>(1)?, row.get::<pgrx::JsonB>(2)?))
    })?;
    let (definition, embeds) = rows.pop().unwrap_or_default();
    let columns = embeds
        .and_then(|j| {
            serde_json::from_value::<std::collections::BTreeMap<String, String>>(j.0).ok()
        })
        .map(|m| m.into_values().collect())
        .unwrap_or_default();
    Ok((definition.unwrap_or_default(), columns))
}

/// SQL over a `pg_tview_meta` row aliased `meta`: whether it predates per-table
/// key mappings (registered before ADR 0157), and the mapping kind of table
/// `relid` (`local`, `mapped`, `propagated`, `all_keys`; NULL when unmapped).
#[must_use]
pub fn mapping_kind_sql(meta: &str, relid: &str) -> (String, String) {
    (
        format!("pg_catalog.jsonb_array_length({meta}.key_mappings) = 0"),
        format!(
            "(SELECT e->>'kind' FROM pg_catalog.jsonb_array_elements({meta}.key_mappings) e \
              WHERE (e->>'relid')::pg_catalog.oid = {relid} LIMIT 1)"
        ),
    )
}
