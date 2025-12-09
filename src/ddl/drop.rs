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
pub fn drop_tview(
    tview_name: &str,
    if_exists: bool,
) -> TViewResult<()> {
    // Use subtransaction for atomic rollback on error
    Spi::run("SAVEPOINT tview_drop").map_err(|e| TViewError::SpiError {
        query: "SAVEPOINT tview_drop".to_string(),
        error: e.to_string(),
    })?;

    match drop_tview_impl(tview_name, if_exists) {
        Ok(()) => {
            Spi::run("RELEASE SAVEPOINT tview_drop").map_err(|e| TViewError::SpiError {
                query: "RELEASE SAVEPOINT tview_drop".to_string(),
                error: e.to_string(),
            })?;
            Ok(())
        }
        Err(e) => {
            // Rollback all changes on error
            let _ = Spi::run("ROLLBACK TO SAVEPOINT tview_drop");
            Err(e)
        }
    }
}

/// Internal implementation of TVIEW dropping
fn drop_tview_impl(
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

    // Step 2: Drop the materialized table
    drop_materialized_table(tview_name)?;

    // Step 3: Drop the backing view
    drop_backing_view(&view_name)?;

    // Step 4: Drop metadata record
    drop_metadata(entity_name)?;

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
    use super::*;

    #[test]
    fn test_tview_exists_in_metadata_non_existent() {
        // This test would require a database context
        // For now, we just verify the function signature compiles
    }
}
