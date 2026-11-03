//! Typed reads of every registered TVIEW at once.

use crate::error::TViewResult;
use crate::lineage::Registered;
use pgrx::pg_sys::Oid;

/// Every registered TVIEW, by entity.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn all() -> TViewResult<Vec<Registered>> {
    let sql = format!(
        "SELECT entity::text, table_oid::oid, view_oid::oid, plan, \
                ARRAY(SELECT u::pg_catalog.oid FROM pg_catalog.unnest(uncascaded_oids) u \
                      WHERE COALESCE(uncascaded_table_policies[pg_catalog.array_position( \
                                uncascaded_table_oids, u)], uncascaded_policy) \
                            = 'full_refresh'), \
                group_keys IS NOT NULL, \
                COALESCE(identity->>'kind' = 'distinct_on', false) \
         FROM {} ORDER BY entity",
        crate::catalog::meta_table()
    );
    let rows = crate::utils::spi::rows(&sql, &[], |row| {
        let (Some(entity), Some(table_oid), Some(view_oid)) = (
            row.get::<String>(1)?,
            row.get::<Oid>(2)?,
            row.get::<Oid>(3)?,
        ) else {
            return Ok(None);
        };
        // A plan that does not decode maps nothing here (the health check reports
        // it; re-registering that TVIEW derives it again).
        let mapped = row
            .get::<pgrx::JsonB>(4)?
            .and_then(|j| crate::lineage::plan::TviewPlan::decode(&entity, j.0).ok())
            .map(|plan| {
                plan.tables
                    .iter()
                    .filter(|t| t.kind != crate::lineage::MappingKind::AllKeys)
                    .map(|t| t.relid)
                    .collect()
            })
            .unwrap_or_default();
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

/// SQL over a `pg_tview_meta` row aliased `meta`: the mapping kind of table
/// `relid` (`local`, `mapped`, `propagated`, `all_keys`; NULL when unmapped).
#[must_use]
pub fn mapping_kind_sql(meta: &str, relid: &str) -> String {
    format!(
        "(SELECT e.kind FROM {schema}.pg_tviews_plan_tables({meta}.plan) e \
          WHERE e.relid = {relid} LIMIT 1)",
        schema = crate::utils::ext_schema(),
    )
}

/// SQL selecting, for each TVIEW, the ordinary and partitioned tables it reads
/// (`tviews.reads`) as `(entity, relid, relkind, tview, kind)`: `tview` when
/// the table is a TVIEW's, `kind` its mapping kind ([`mapping_kind_sql`]).
#[must_use]
pub fn reads_with_kind_sql() -> String {
    let kind = mapping_kind_sql("m", "r.relid");
    format!(
        "SELECT DISTINCT r.entity, r.relid, c.relkind, \
                    r.relid IN (SELECT table_oid::oid FROM {meta}) AS tview, \
                    {kind} AS kind \
             FROM {schema}.reads r \
             JOIN {meta} m ON m.entity = r.entity \
             JOIN pg_catalog.pg_class c ON c.oid = r.relid AND c.relkind IN ('r', 'p')",
        meta = crate::catalog::meta_table(),
        schema = crate::utils::ext_schema(),
    )
}
