use std::collections::{HashMap, HashSet, VecDeque};
use pgrx::prelude::*;
use crate::TViewResult;

/// Entity dependency graph for refresh ordering
///
/// Example:
/// - `tv_company` (no dependencies)
/// - `tv_user` (depends on `tv_company` via `fk_company`)
/// - `tv_post` (depends on `tv_user` via `fk_user`)
/// - `tv_feed` (depends on `tv_post` via `fk_post`)
///
/// Topological order: `["company", "user", "post", "feed"]`
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// Parent relationships: entity -> list of entities that depend on it
    /// Example: "user" -> `["post", "feed"]`
    #[allow(dead_code)]
    pub parents: HashMap<String, Vec<String>>,

    /// Child relationships: entity -> list of entities it depends on
    /// Example: "post" -> `["user"]`
    #[allow(dead_code)]
    pub children: HashMap<String, Vec<String>>,

    /// Topological order (refresh from low to high dependency)
    /// Example: `["company", "user", "post", "feed"]`
    pub topo_order: Vec<String>,
}

impl EntityDepGraph {
    /// Build dependency graph from `pg_tview_meta`
    pub fn load() -> TViewResult<Self> {
        // Query pg_tview_meta for all entities and their FK columns
        let query = "SELECT entity, fk_columns FROM pg_tview_meta";

        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_entities: HashSet<String> = HashSet::new();

        Spi::connect(|client| {
            let rows = client.select(query, None, &[])?;

            for row in rows {
                let entity: String = row["entity"].value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get entity: {e}"),
                    })?
                    .ok_or_else(|| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: "entity column is NULL".to_string(),
                    })?;
                let fk_columns: Option<Vec<String>> = row["fk_columns"].value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get fk_columns: {e}"),
                    })?;

                all_entities.insert(entity.clone());

                if let Some(fk_cols) = fk_columns {
                    for fk_col in fk_cols {
                        // FK column format: "fk_<entity>"
                        // Example: "fk_user" -> "user"
                        if let Some(parent_entity) = fk_col.strip_prefix("fk_") {
            // Register parent relationship
            parents.entry(parent_entity.to_string())
                .or_default()
                .push(entity.clone());

            // Register child relationship
            children.entry(entity.clone())
                .or_default()
                .push(parent_entity.to_string());
                        }
                    }
                }
            }

            Ok::<_, spi::SpiError>(())
        })?;

        // Compute topological order
        let topo_order = topological_sort(&all_entities, &children)?;

        Ok(Self {
            parents,
            children,
            topo_order,
        })
    }

    /// Sort refresh keys by dependency order
    ///
    /// Keys are grouped by entity, then sorted by `topo_order`.
    /// Within each entity group, PK order is preserved.
    pub fn sort_keys(&self, keys: Vec<super::key::RefreshKey>) -> Vec<super::key::RefreshKey> {
        // Group by entity
        let mut groups: HashMap<String, Vec<i64>> = HashMap::new();
        for key in keys {
            groups.entry(key.entity.clone())
                .or_default()
                .push(key.pk);
        }

        // Sort entities by topo_order
        let mut sorted_keys = Vec::new();
        for entity in &self.topo_order {
            if let Some(pks) = groups.get(entity) {
                for pk in pks {
                    sorted_keys.push(super::key::RefreshKey {
                        entity: entity.clone(),
                        pk: *pk,
                    });
                }
            }
        }

        sorted_keys
    }
}

/// Topological sort using Kahn's algorithm
fn topological_sort(
    entities: &HashSet<String>,
    children: &HashMap<String, Vec<String>>,
) -> TViewResult<Vec<String>> {
    // Calculate in-degree for each entity
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for entity in entities {
        in_degree.insert(entity.clone(), 0);
    }

    for deps in children.values() {
        for dep in deps {
            *in_degree.entry(dep.clone()).or_insert(0) += 1;
        }
    }

    // Start with entities that have no dependencies
    let mut queue: VecDeque<String> = VecDeque::new();
        for (entity, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(entity.clone());
            }
        }

        let mut result = Vec::new();

    while let Some(entity) = queue.pop_front() {
        result.push(entity.clone());

        // Find entities that depend on this one
        if let Some(parents) = children.get(&entity) {
            for parent in parents {
                if let Some(degree) = in_degree.get_mut(parent) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(parent.clone());
                    }
                }
            }
        }
    }

    // Check for cycles
    if result.len() != entities.len() {
        return Err(crate::TViewError::DependencyCycle {
            entities: entities.iter().cloned().collect(),
        });
    }

        Ok(result)
    }



    #[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort() {
        // Entity graph:
        // company (no deps)
        // user -> company
        // post -> user
        // feed -> post

        let entities: HashSet<String> = ["company", "user", "post", "feed"]
            .iter().map(|&s| s.to_string()).collect();

        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        children.insert("user".to_string(), vec!["company".to_string()]);
        children.insert("post".to_string(), vec!["user".to_string()]);
        children.insert("feed".to_string(), vec!["post".to_string()]);

        let topo = topological_sort(&entities, &children).unwrap();

        // Valid topological orders:
        // ["company", "user", "post", "feed"]
        // Check that company comes before user, user before post, etc.
        let company_idx = topo.iter().position(|e| e == "company").unwrap();
        let user_idx = topo.iter().position(|e| e == "user").unwrap();
        let post_idx = topo.iter().position(|e| e == "post").unwrap();
        let feed_idx = topo.iter().position(|e| e == "feed").unwrap();

        assert!(company_idx < user_idx);
        assert!(user_idx < post_idx);
        assert!(post_idx < feed_idx);
    }
}