//! What the row trigger needs about the TVIEW whose root table it fires on.

use pgrx::prelude::*;
use std::collections::HashMap;

/// Cached information for a table managed by `pg_tviews`.
///
/// Beyond the entity name, this carries everything the issue #56 direct-patch
/// eligibility check needs, so the row trigger can decide the fast
/// path from a single cached lookup with **no SPI in the hot path** (populated once
/// per session on cache miss, invalidated on DDL).
#[derive(Clone, Debug)]
pub struct CachedEntityInfo {
    pub name: String,
    /// The TVIEW is keyed on a DISTINCT ON key: a row's own values may not be its
    /// group's, so the fast path declines.
    pub distinct_on: bool,
    /// Registered before the root table had a cascade path of its own (ADR
    /// 0169): how the trigger finds its key until it is re-registered.
    pub legacy_root: Option<LegacyRoot>,

    /// Direct-patch column→key map (issue #56): base column name → JSONB key it
    /// feeds in the entity's own `data`. Empty ⇒ the fast path never engages.
    pub direct_map: HashMap<String, String>,

    /// Integer FK columns of this entity's base table. A changed FK is a
    /// membership change ⇒ the fast path must decline (issue #56 eligibility).
    pub fk_columns: Vec<String>,

    /// UUID FK columns of this entity's base table (same membership rule).
    pub uuid_fk_columns: Vec<String>,

    /// Output columns the `tv_<entity>` table materialises (`pk_<entity>`, `id`,
    /// `data`, and any column projected outside `data`). A changed base column that
    /// is also a projected output column would go stale under a data-only patch ⇒
    /// the fast path declines (issue #56 eligibility).
    pub output_columns: Vec<String>,

    /// `true` when the backing view is a UNION / UNION ALL — different refresh
    /// machinery, so the fast path declines (issue #56 eligibility).
    pub is_union: bool,
}

/// How the row trigger keys a TVIEW registered before its root table had a
/// cascade path of its own (ADR 0169).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegacyRoot {
    /// `pk_<entity>`, read by name off `tb_<entity>`.
    Pk,
    /// A DISTINCT ON TVIEW: refreshed in full.
    DistinctOn,
}

/// The TVIEW `table_oid` is the root table of, if any (cached per backend).
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn entity_info(table_oid: pg_sys::Oid) -> crate::TViewResult<Option<CachedEntityInfo>> {
    if !crate::config::table_cache_enabled() {
        return load_entity_info_uncached(table_oid);
    }
    if let Some(info) = super::ENTITIES.with(|m| m.get(&table_oid)) {
        crate::metrics::metrics_api::record_table_cache_hit();
        return Ok(info);
    }
    crate::metrics::metrics_api::record_table_cache_miss();
    let info = load_entity_info_uncached(table_oid)?;
    super::ENTITIES.with(|m| m.insert(table_oid, info.clone()));
    Ok(info)
}

/// Load entity info from the database on a cache miss.
///
/// Loads the full `TviewMeta` for the entity plus the `tv_<entity>` output
/// columns, so a single cached record answers every issue #56 eligibility
/// question without further SPI in the trigger hot path.
fn load_entity_info_uncached(
    table_oid: pg_sys::Oid,
) -> crate::TViewResult<Option<CachedEntityInfo>> {
    let Some(name) = crate::catalog::entity_for_table_uncached(table_oid)? else {
        return Ok(None);
    };

    // Name resolved but no meta row (shouldn't happen) → minimal safe info
    // with the fast path disabled (empty direct_map).
    let Some(meta) = crate::catalog::TviewMeta::load_by_entity(&name)? else {
        return Ok(Some(CachedEntityInfo {
            name,
            distinct_on: false,
            legacy_root: Some(LegacyRoot::Pk),
            direct_map: HashMap::new(),
            fk_columns: Vec::new(),
            uuid_fk_columns: Vec::new(),
            output_columns: Vec::new(),
            is_union: false,
        }));
    };

    let mut direct_map: HashMap<String, String> = meta
        .direct_map_columns
        .iter()
        .cloned()
        .zip(meta.direct_map_keys.iter().cloned())
        .collect();

    // Output columns the tv_<entity> table materialises. If they can't be
    // determined we can't verify the "projected column" eligibility rule, so
    // disable the fast path for this entity rather than risk a stale column.
    let output_columns = if let Ok(cols) = crate::utils::get_view_columns_by_oid(meta.tview_oid) {
        cols
    } else {
        direct_map.clear();
        Vec::new()
    };

    // A column also read outside `data` (projected, joined on, filtered) is never
    // in the map: registration keeps only columns read nowhere else (#98).

    Ok(Some(CachedEntityInfo {
        name,
        distinct_on: meta.identity.kind == crate::lineage::IdentityKind::DistinctOn,
        legacy_root: if !meta.identity.legacy {
            None
        } else if meta.identity.legacy_distinct_on {
            Some(LegacyRoot::DistinctOn)
        } else {
            Some(LegacyRoot::Pk)
        },
        direct_map,
        fk_columns: meta.fk_columns,
        uuid_fk_columns: meta.uuid_fk_columns,
        output_columns,
        is_union: meta.is_union,
    }))
}
