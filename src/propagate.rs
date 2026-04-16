use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;
use std::collections::HashMap;

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
/// let key = RefreshKey::pk("user", 1);
/// let parents = find_parents_for(&key)?;
/// // Returns: [
/// //   RefreshKey::pk("post", 10),
/// //   RefreshKey::pk("post", 20),
/// //   RefreshKey::pk("comment", 5),
/// // ]
/// // These are all the tv_post and tv_comment rows where fk_user = 1
/// ```
pub fn find_parents_for(key: &RefreshKey, graph: &crate::queue::EntityDepGraph) -> crate::TViewResult<Vec<RefreshKey>> {

    // Find all parent entities that depend on this entity (from cached graph)
    let parent_entities = find_parent_entities(&key.entity, graph)?;

    if parent_entities.is_empty() {
        return Ok(Vec::new());
    }

    let mut parent_keys = Vec::new();

    // Propagation only applies to PK-based keys; DISTINCT ON dedup keys
    // do not carry a FK value that parent TVIEWs can use for lookup.
    if key.is_dedup() {
        return Ok(Vec::new());
    }

    // For each parent entity, find affected rows
    for parent_entity in parent_entities {
        let affected_pks = find_affected_pks(&parent_entity, &key.entity, key.pk)?;

        // Convert to RefreshKeys
        for pk in affected_pks {
            parent_keys.push(RefreshKey::pk(&parent_entity, pk));
        }
    }

    Ok(parent_keys)
}

/// Find parent keys for multiple child keys in a single batched operation (P-07).
///
/// This is the **optimized version of find_parents_for** that batches multiple keys
/// into fewer SPI queries. Instead of calling find_affected_pks once per key,
/// it groups keys by (parent_entity, child_entity) and issues ONE query per group
/// using PostgreSQL's `= ANY($1)` operator.
///
/// # Performance
/// - **Before**: N keys × M parent entities = N*M SPI queries
/// - **After**: N keys × M parent entities = M SPI queries (batched)
/// - **Example**: 3 changed user PKs with 2 parent entities (post, comment)
///   - Before: 6 queries (3 × 2)
///   - After: 2 queries (1 per parent entity)
///
/// # Arguments
/// * `keys` - Multiple RefreshKey values to propagate
/// * `graph` - Cached entity dependency graph
///
/// # Returns
/// Map from each input key to its discovered parent keys
pub fn find_parents_batch(
    keys: &[RefreshKey],
    graph: &crate::queue::EntityDepGraph,
) -> crate::TViewResult<HashMap<RefreshKey, Vec<RefreshKey>>> {
    let mut result: HashMap<RefreshKey, Vec<RefreshKey>> = HashMap::new();

    // Filter to PK-only keys (dedup keys don't propagate)
    let pk_keys: Vec<_> = keys.iter().filter(|k| !k.is_dedup()).cloned().collect();

    if pk_keys.is_empty() {
        return Ok(result);
    }

    // Build batch groups: (parent_entity, child_entity) -> Vec<child_pk>
    let batch_groups = build_batch_groups(&pk_keys, graph)?;

    // Execute one query per group
    for ((parent_entity, child_entity), child_pks) in batch_groups {
        let affected_pk_map = find_affected_pks_batch(&parent_entity, &child_entity, &child_pks)?;

        // Map results back to original keys
        for key in &pk_keys {
            if key.entity == child_entity && let Some(affected_pks) = affected_pk_map.get(&key.pk) {
                for pk in affected_pks {
                    result
                        .entry(key.clone())
                        .or_default()
                        .push(RefreshKey::pk(&parent_entity, *pk));
                }
            }
        }
    }

    Ok(result)
}

/// Build batch groups for propagation query (unit-testable logic).
///
/// Groups keys by (parent_entity, child_entity) to minimize queries.
/// Returns: Map<(parent, child), Vec<child_pk>>
fn build_batch_groups(
    keys: &[RefreshKey],
    graph: &crate::queue::EntityDepGraph,
) -> crate::TViewResult<HashMap<(String, String), Vec<i64>>> {
    let mut groups: HashMap<(String, String), Vec<i64>> = HashMap::new();

    for key in keys {
        // Get parent entities for this child from the cached graph
        let parent_entities = graph.parents.get(&key.entity).cloned().unwrap_or_default();

        for parent_entity in parent_entities {
            groups
                .entry((parent_entity, key.entity.clone()))
                .or_default()
                .push(key.pk);
        }
    }

    Ok(groups)
}

/// Find all PKs in a parent TVIEW that reference any of the given child PKs (batched).
///
/// This uses PostgreSQL's `= ANY($1)` to check multiple FKs in one query.
/// Returns a map from child_pk to the list of parent PKs referencing it.
fn find_affected_pks_batch(
    parent_entity: &str,
    child_entity: &str,
    child_pks: &[i64],
) -> spi::Result<HashMap<i64, Vec<i64>>> {
    let fk_col = format!("fk_{child_entity}");
    let parent_table = format!("tv_{parent_entity}");
    let parent_pk_col = format!("pk_{parent_entity}");

    // Use = ANY($1) to batch multiple child PKs into one query
    let query = format!(
        "SELECT {fk_col}, {parent_pk_col} FROM {parent_table} WHERE {fk_col} = ANY($1)"
    );

    Spi::connect(|client| {
        // Convert child_pks to a PostgreSQL array datum
        let pks_array = child_pks.to_vec();
        let args = vec![unsafe {
            DatumWithOid::new(
                pks_array,
                PgOid::BuiltIn(PgBuiltInOids::INT8ARRAYOID).value(),
            )
        }];

        let rows = client.select(&query, None, &args)?;
        let mut result: HashMap<i64, Vec<i64>> = HashMap::new();

        for row in rows {
            if let (Some(child_pk), Some(parent_pk)) = (
                row[fk_col.as_str()].value::<i64>()?,
                row[parent_pk_col.as_str()].value::<i64>()?,
            ) {
                result.entry(child_pk).or_default().push(parent_pk);
            }
        }

        Ok(result)
    })
}

/// Find all parent entities that depend on the given entity (from cached graph).
///
/// Example: `find_parent_entities`("user") -> `["post", "comment"]`
/// This means `tv_post` and `tv_comment` both have FK references to `tv_user`
fn find_parent_entities(child_entity: &str, graph: &crate::queue::EntityDepGraph) -> spi::Result<Vec<String>> {
    // Look up parent entities from cached graph (no SPI query)
    Ok(graph.parents.get(child_entity).cloned().unwrap_or_default())
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


