use pgrx::prelude::*;

/// Propagation Engine: Cascade Refresh for Dependent Views
///
/// This module handles cascading refreshes when TVIEW data changes:
/// - **Parent Discovery**: Finds views that depend on changed entities
/// - **Affected Row Identification**: Locates rows impacted by changes
/// - **Cascade Refresh**: Updates dependent views automatically
/// - **Cycle Prevention**: Avoids infinite loops in dependency chains
///
/// ## Propagation Flow
///
/// 1. **Change Detection**: Trigger identifies changed base table row
/// 2. **Parent Analysis**: Find TVIEWs that reference this entity
/// 3. **Impact Assessment**: Identify which parent rows are affected
/// 4. **Cascade Refresh**: Update affected parent rows
/// 5. **Recursive Propagation**: Handle multi-level dependencies
///
/// ## Performance Optimizations
///
/// - Batch operations for multiple affected rows
/// - Early termination for unchanged data
/// - Dependency graph caching
/// - Configurable propagation depth limits
use crate::refresh::main::{ViewRow, refresh_pk};
use crate::refresh::batch;
use crate::catalog::TviewMeta;
use crate::queue::RefreshKey;

/// Discover parents (entities that depend on this entity) and refresh them.
///
/// Example: When `tv_user` row (pk=1) changes:
/// 1. Find parent entities (e.g., `tv_post` depends on `tv_user`)
/// 2. Find affected rows (all `tv_post` where `fk_user` = 1)
/// 3. Refresh each affected row
pub fn propagate_from_row(row: &ViewRow) -> spi::Result<()> {
    // Find all parent entities that depend on this entity
    let parent_entities = find_parent_entities(&row.entity_name)?;

    if parent_entities.is_empty() {
        // No parents to cascade to
        return Ok(());
    }


    // For each parent entity, find affected rows and refresh them
    for parent_entity in parent_entities {
        let affected_pks = find_affected_pks(&parent_entity, &row.entity_name, row.pk)?;

        if affected_pks.is_empty() {
            continue;
        }


        // Load parent TVIEW metadata to get view_oid for refresh
        let Some(parent_meta) = TviewMeta::load_by_entity(&parent_entity)? else {
            warning!("No metadata found for parent entity {}", parent_entity);
            continue;
        };

        // Use batch refresh for large cascades, individual refresh for small ones
        if affected_pks.len() >= 10 {
            batch::refresh_batch(&parent_entity, &affected_pks)?;
        } else {
            // Refresh each affected row individually
            for pk in affected_pks {
                refresh_pk(parent_meta.view_oid, pk)?;
            }
        }
    }

    Ok(())
}

/// Find parent keys that depend on the given entity+pk (without refreshing them)
///
/// Propagation that returns keys instead of
/// performing immediate recursive refreshes.
///
/// # Example
///
/// ```rust
/// let key = RefreshKey { entity: "user".into(), pk: 1 };
/// let parents = find_parents_for(&key)?;
/// // Returns: [
/// //   RefreshKey { entity: "post", pk: 10 },
/// //   RefreshKey { entity: "post", pk: 20 },
/// //   RefreshKey { entity: "comment", pk: 5 },
/// // ]
/// // These are all the tv_post and tv_comment rows where fk_user = 1
/// ```
pub fn find_parents_for(key: &RefreshKey) -> crate::TViewResult<Vec<RefreshKey>> {

    // Find all parent entities that depend on this entity
    let parent_entities = find_parent_entities(&key.entity)?;

    if parent_entities.is_empty() {
        return Ok(Vec::new());
    }

    let mut parent_keys = Vec::new();

    // For each parent entity, find affected rows
    for parent_entity in parent_entities {
        let affected_pks = find_affected_pks(&parent_entity, &key.entity, key.pk)?;

        // Convert to RefreshKeys
        for pk in affected_pks {
            parent_keys.push(RefreshKey {
                entity: parent_entity.clone(),
                pk,
            });
        }
    }

    Ok(parent_keys)
}

/// Find all parent entities that depend on the given entity.
///
/// Example: `find_parent_entities`("user") -> `["post", "comment"]`
/// This means `tv_post` and `tv_comment` both have FK references to `tv_user`
fn find_parent_entities(child_entity: &str) -> spi::Result<Vec<String>> {
    // Query pg_tview_meta to find entities whose fk_columns reference this entity
    // e.g., if child_entity = "user", look for entities with "fk_user" in fk_columns

    let fk_col = format!("fk_{child_entity}");

    let query = format!(
        "SELECT entity FROM public.pg_tview_meta
         WHERE '{fk_col}' = ANY(fk_columns)"
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut parents = Vec::new();

        for row in rows {
            if let Some(entity) = row["entity"].value::<String>()? {
                parents.push(entity);
            }
        }

        Ok(parents)
    })
}

/// Find all PKs in the parent TVIEW that reference the given child PK.
///
/// Example: `find_affected_pks`("post", "user", 1)
/// Returns all `pk_post` values where `fk_user` = 1
fn find_affected_pks(
    parent_entity: &str,
    child_entity: &str,
    child_pk: i64,
) -> spi::Result<Vec<i64>> {
    let fk_col = format!("fk_{child_entity}");
    let parent_table = format!("tv_{parent_entity}");
    let parent_pk_col = format!("pk_{parent_entity}");

    let query = format!(
        "SELECT {parent_pk_col} FROM {parent_table} WHERE {fk_col} = {child_pk}"
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, &[])?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[parent_pk_col.as_str()].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

