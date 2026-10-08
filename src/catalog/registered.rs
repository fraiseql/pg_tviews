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
        "SELECT entity::text, table_oid::oid, view_oid::oid, plan->'tables', \
                ARRAY(SELECT u::pg_catalog.oid FROM pg_catalog.unnest(uncascaded_oids) u \
                      WHERE COALESCE(uncascaded_table_policies[pg_catalog.array_position( \
                                uncascaded_table_oids, u)], uncascaded_policy) \
                            = 'full_refresh'), \
                group_keys IS NOT NULL, \
                COALESCE(identity->>'kind' = 'distinct_on', false) \
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

/// The output columns of `entity` holding the keys of the TVIEWs it embeds.
///
/// # Errors
/// The catalog cannot be read, or `entity` is not registered.
pub fn embed_columns(entity: &str) -> TViewResult<Vec<String>> {
    let sql = format!(
        "SELECT ARRAY(SELECT pg_catalog.jsonb_array_elements_text(e->'lookups') \
                      FROM pg_catalog.jsonb_array_elements(plan->'embeds') e) \
         FROM {} WHERE entity = $1",
        crate::utils::meta_table()
    );
    let mut rows = crate::utils::spi::rows(&sql, &[crate::utils::spi::text(entity)], |row| {
        Ok(row.get::<Vec<String>>(1)?)
    })?;
    let Some(columns) = rows.pop() else {
        return Err(crate::TViewError::MetadataNotFound {
            entity: entity.to_string(),
        });
    };
    Ok(columns.unwrap_or_default())
}

/// SQL over a `pg_tview_meta` row aliased `meta`: the mapping kind of table
/// `relid` (`local`, `mapped`, `propagated`, `all_keys`; NULL when unmapped).
#[must_use]
pub fn mapping_kind_sql(meta: &str, relid: &str) -> String {
    format!(
        "(SELECT e->>'kind' FROM pg_catalog.jsonb_array_elements({meta}.plan->'tables') e \
          WHERE (e->>'relid')::pg_catalog.oid = {relid} LIMIT 1)"
    )
}
