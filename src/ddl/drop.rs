use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

/// Drop a TVIEW and all its associated objects
///
/// This function handles the removal of:
/// - The materialized table (tv_<entity>)
/// - The backing view (v_<entity>)
/// - The metadata record in pg_tview_meta
///
/// If `if_exists` is true, no error is raised if the TVIEW doesn't exist.
/// PostgreSQL's transaction system provides automatic atomicity.
pub fn drop_tview(
    tview_name: &str,
    if_exists: bool,
) -> TViewResult<()> {
    let entity_name = tview_name.trim_start_matches("tv_");
    let view_name = format!("v_{}", entity_name);

    // Step 1: Check if TVIEW exists
    let exists = tview_exists_in_metadata(entity_name)?;

    if !exists && !if_exists {
        return Err(TViewError::MetadataNotFound {
            entity: entity_name.to_string(),
        });
    }

    if !exists {
        // IF EXISTS was specified and TVIEW doesn't exist - this is OK
        info!("TVIEW {} does not exist, skipping DROP", tview_name);
        return Ok(());
    }

    // Step 2: Find and remove triggers from base tables
    match crate::dependency::find_base_tables(&view_name) {
        Ok(dep_graph) => {
            if !dep_graph.base_tables.is_empty() {
                crate::dependency::remove_triggers(&dep_graph.base_tables, entity_name)?;
                info!("Removed triggers from {} base tables", dep_graph.base_tables.len());
            }
        }
        Err(e) => {
            warning!("Could not find dependencies for cleanup: {}", e);
            // Continue with drop - triggers will be orphaned but not harmful
        }
    }

    // Step 3: Drop the materialized table
    drop_materialized_table(tview_name)?;

    // Step 4: Drop the backing view
    drop_backing_view(&view_name)?;

    // Step 5: Drop metadata record
    drop_metadata(entity_name)?;

    // Invalidate caches since TVIEW was dropped
    crate::queue::cache::invalidate_all_caches();

    info!("TVIEW {} dropped successfully", tview_name);

    Ok(())
}

/// Check if a TVIEW exists in metadata
fn tview_exists_in_metadata(entity_name: &str) -> TViewResult<bool> {
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = '{}'",
        entity_name.replace("'", "''")
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW metadata: {}", entity_name),
        pg_error: format!("{:?}", e),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Drop the materialized table (tv_<entity>)
fn drop_materialized_table(tview_name: &str) -> TViewResult<()> {
    let drop_table_sql = format!(
        "DROP TABLE IF EXISTS public.{}",
        tview_name
    );

    Spi::run(&drop_table_sql).map_err(|e| TViewError::SpiError {
        query: drop_table_sql,
        error: e.to_string(),
    })?;

    info!("Dropped materialized table: {}", tview_name);
    Ok(())
}

/// Drop the backing view (v_<entity>)
fn drop_backing_view(view_name: &str) -> TViewResult<()> {
    let drop_view_sql = format!(
        "DROP VIEW IF EXISTS public.{}",
        view_name
    );

    Spi::run(&drop_view_sql).map_err(|e| TViewError::SpiError {
        query: drop_view_sql,
        error: e.to_string(),
    })?;

    info!("Dropped backing view: {}", view_name);
    Ok(())
}

/// Drop metadata record from pg_tview_meta
fn drop_metadata(entity_name: &str) -> TViewResult<()> {
    let delete_meta_sql = format!(
        "DELETE FROM public.pg_tview_meta WHERE entity = '{}'",
        entity_name.replace("'", "''")
    );

    Spi::run(&delete_meta_sql).map_err(|e| TViewError::SpiError {
        query: delete_meta_sql,
        error: e.to_string(),
    })?;

    info!("Dropped TVIEW metadata for: {}", entity_name);
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_tview_exists_in_metadata_non_existent() {
        // This test would require a database context
        // For now, we just verify the function signature compiles
    }
}
