use pgrx::prelude::*;

use crate::refresh::ViewRow;

/// Discover parents (entities that depend on this entity) and refresh them.
pub fn propagate_from_row(row: &ViewRow) -> spi::Result<()> {
    // TODO:
    // 1. Use pg_depend and pg_tview_meta to find which v_* depend on row.entity_name.
    // 2. For each parent entity, determine which fk_* points to row.pk.
    // 3. Call refresh_pk(parent_source_oid, fk_value).
    //
    // For now, this is a no-op stub.
    Ok(())
}

/// Example helper: find all parent entity names for a given entity.
/// Likely implemented via pg_depend → dependency on view oids.
pub fn parent_entities_of(_entity_name: &str) -> spi::Result<Vec<String>> {
    // TODO: actual catalog queries.
    Ok(Vec::new())
}

