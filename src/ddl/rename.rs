//! Keep TVIEW metadata in step with `ALTER … RENAME COLUMN` on a relation a
//! backing view reads from.
//!
//! `PostgreSQL` rewrites the backing view to follow a column rename, but the text in
//! `pg_tview_meta.definition` and everything derived from it at creation (cascade
//! paths, the column-aware refresh set, the direct-patch map, DISTINCT ON keys)
//! would still name the old column: propagation from the renamed column silently
//! stops. After a rename, every affected TVIEW's definition becomes the backing
//! view's own text (`pg_get_viewdef`), which names the new column, and its metadata
//! is re-derived from it with the same code that creation uses. The schema files
//! that created the TVIEW stay the source of its text.

use crate::error::{TViewError, TViewResult};
use pgrx::pg_sys::Oid;

/// Re-derive the metadata of every TVIEW whose backing view reads column
/// `new_name` (formerly `old_name`) of relation `relid`.
///
/// # Errors
/// Returns an error if a catalog query fails or the re-derived definition cannot
/// be analyzed; the caller aborts the `RENAME` so no TVIEW is left stale.
pub fn handle_column_rename(relid: Oid, old_name: &str, new_name: &str) -> TViewResult<()> {
    let affected = crate::catalog::queries::reading_column(relid, new_name)?;
    if !affected.is_empty() {
        crate::revision::check();
    }
    let relname = relation_name(relid)?;
    for (entity, schema_name, view_oid) in affected {
        let viewdef: String = crate::utils::spi::one(
            "SELECT pg_get_viewdef($1)",
            &[crate::utils::spi::oid(view_oid)],
        )
        .map_err(|e| crate::utils::spi::catalog_error("Read backing view definition", &e))?
        .unwrap_or_default();
        let definition = viewdef.trim().trim_end_matches(';').trim_end();

        // An aggregate TVIEW names its group key columns by name. The
        // rename was authorized by PostgreSQL; the catalog is written as the
        // extension's owner.
        crate::catalog::queries::rename_group_key(&entity, &relname, old_name, new_name)?;

        crate::ddl::create::reregister_metadata(&entity, &schema_name, definition)?;
    }
    Ok(())
}

fn relation_name(relid: Oid) -> TViewResult<String> {
    crate::utils::spi::one::<String>(
        "SELECT relname::text FROM pg_class WHERE oid = $1",
        &[crate::utils::spi::oid(relid)],
    )
    .map_err(|e| crate::utils::spi::catalog_error("Resolve renamed relation", &e))?
    .ok_or_else(|| TViewError::CatalogError {
        operation: "Resolve renamed relation".to_string(),
        pg_error: format!("relation {relid:?} not found"),
    })
}
