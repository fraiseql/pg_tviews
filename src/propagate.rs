use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;

/// Propagation Engine: Parent Discovery for Dependent Views
///
/// This module provides parent discovery for the transaction-level queue:
/// - **Parent Discovery**: Finds views that depend on changed entities
/// - **Affected Row Identification**: Locates rows impacted by changes
///
/// Used by the `PRE_COMMIT` handler (`src/queue/`) to iteratively discover
/// and enqueue parent TVIEWs for refresh.
use crate::queue::RefreshKey;

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

    // Table/column names are from pg_tview_meta (internal); child_pk is parameterized
    let query = format!(
        "SELECT {parent_pk_col} FROM {parent_table} WHERE {fk_col} = $1"
    );
    let args = vec![unsafe {
        DatumWithOid::new(child_pk, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value())
    }];

    Spi::connect(|client| {
        let rows = client.select(&query, None, &args)?;
        let mut pks = Vec::new();

        for row in rows {
            if let Some(pk) = row[parent_pk_col.as_str()].value::<i64>()? {
                pks.push(pk);
            }
        }

        Ok(pks)
    })
}

