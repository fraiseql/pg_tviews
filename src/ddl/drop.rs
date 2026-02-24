use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

/// Drop a TVIEW and all its associated objects
///
/// This function handles the removal of:
/// - The materialized table (`tv_<entity>`)
/// - The backing view (`v_<entity>`)
/// - The metadata record in `pg_tview_meta`
///
/// If `if_exists` is true, no error is raised if the TVIEW doesn't exist.
/// `PostgreSQL's` transaction system provides automatic atomicity.
///
/// # Errors
/// Returns error if TVIEW doesn't exist (unless `if_exists` is true) or drop operation fails
pub fn drop_tview(
    tview_name: &str,
    if_exists: bool,
) -> TViewResult<()> {
    let entity_name = tview_name.trim_start_matches("tv_");
    let view_name = format!("v_{entity_name}");

    // Step 1: Check if TVIEW exists
    let exists = tview_exists_in_metadata(entity_name)?;

    if !exists && !if_exists {
        return Err(TViewError::MetadataNotFound {
            entity: entity_name.to_string(),
        });
    }

    if !exists {
        // IF EXISTS was specified and TVIEW doesn't exist - this is OK
        return Ok(());
    }

    // Load metadata to get OIDs for schema-safe drops
    let meta = crate::catalog::TviewMeta::load_by_entity(entity_name)
        .map_err(|e| TViewError::SpiError {
            query: "Load TviewMeta by entity".to_string(),
            error: e.to_string(),
        })?;

    // Step 2: Find and remove triggers from base tables
    match crate::dependency::find_base_tables(&view_name) {
        Ok(dep_graph) => {
            if !dep_graph.base_tables.is_empty() {
                crate::dependency::remove_triggers(&dep_graph.base_tables, entity_name)?;
            }
        }
        Err(e) => {
            warning!("Could not find dependencies for cleanup: {}", e);
            // Continue with drop - triggers will be orphaned but not harmful
        }
    }

    // Step 3: Drop the materialized table (schema-resolved via OID)
    if let Some(ref m) = meta {
        drop_by_oid(m.tview_oid, "TABLE")?;
    }

    // Step 4: Drop the backing view (schema-resolved via OID)
    if let Some(ref m) = meta {
        drop_by_oid(m.view_oid, "VIEW")?;
    }

    // Step 5: Drop metadata record
    drop_metadata(entity_name)?;

    // Invalidate caches since TVIEW was dropped
    crate::queue::cache::invalidate_all_caches();

    // Log the drop for audit trail
    if let Err(e) = crate::audit::log_drop(entity_name) {
        warning!("Failed to log TVIEW drop: {}", e);
    }


    Ok(())
}

/// Resolve a schema-qualified name from an object OID and drop it
///
/// Uses `pg_class JOIN pg_namespace` to find the object's schema at runtime,
/// so drops work regardless of which schema the TVIEW was created in.
fn drop_by_oid(oid: pg_sys::Oid, kind: &str) -> TViewResult<()> {
    let qualified = crate::utils::spi_get_string(&format!(
        "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname) \
         FROM pg_class c \
         JOIN pg_namespace n ON c.relnamespace = n.oid \
         WHERE c.oid = {}",
        oid.to_u32()
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Resolve qualified name for OID {}", oid.to_u32()),
        pg_error: e.to_string(),
    })?;

    if let Some(qname) = qualified {
        let sql = format!("DROP {kind} IF EXISTS {qname}");
        crate::utils::spi_run_ddl(&sql).map_err(|e| TViewError::SpiError {
            query: sql,
            error: e,
        })?;
    }

    Ok(())
}

/// Check if a TVIEW exists in metadata
fn tview_exists_in_metadata(entity_name: &str) -> TViewResult<bool> {
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta WHERE entity = '{}'",
        entity_name.replace('\'', "''")
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW metadata: {entity_name}"),
        pg_error: format!("{e:?}"),
    })
    .map(|opt| opt.unwrap_or(false))
}

/// Drop metadata record from `pg_tview_meta`
fn drop_metadata(entity_name: &str) -> TViewResult<()> {
    let delete_meta_sql = format!(
        "DELETE FROM pg_tview_meta WHERE entity = '{}'",
        entity_name.replace('\'', "''")
    );

    Spi::run(&delete_meta_sql).map_err(|e| TViewError::SpiError {
        query: delete_meta_sql,
        error: e.to_string(),
    })?;

    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;


    #[pg_test]
    fn test_drop_tview_nonexistent_if_exists() {
        // Dropping a non-existent TVIEW with IF EXISTS should not error
        let result = Spi::run("SELECT pg_tviews_drop('nonexistent', true)");
        assert!(result.is_ok(), "IF EXISTS drop of non-existent TVIEW should succeed");
    }

    #[pg_test]
    fn test_drop_tview_nonexistent_strict() {
        // Dropping a non-existent TVIEW without IF EXISTS should error
        let result = Spi::run("SELECT pg_tviews_drop('nonexistent', false)");
        assert!(result.is_err(), "Strict drop of non-existent TVIEW should fail");
    }
}
