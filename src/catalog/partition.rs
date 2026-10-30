//! Partition trees, as the catalog records them.

use crate::TViewResult;
use pgrx::pg_sys::Oid;

/// The root of `table_oid`'s partition tree, `table_oid` itself if it is not a
/// partition (cached).
pub fn partition_root(table_oid: Oid) -> TViewResult<Oid> {
    if let Some(root) = crate::cache::PARTITION_ROOTS.with(|m| m.get(&table_oid)) {
        return Ok(root);
    }
    let root = crate::utils::spi::catalog_one::<Oid>(
        "SELECT COALESCE(pg_catalog.pg_partition_root($1)::pg_catalog.oid, $1)",
        &[crate::utils::spi::regclass(table_oid)],
    )?
    .unwrap_or(table_oid);
    crate::cache::PARTITION_ROOTS.with(|m| m.insert(table_oid, root));
    Ok(root)
}
