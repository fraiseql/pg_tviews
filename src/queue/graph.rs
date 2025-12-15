use std::collections::{HashMap, HashSet, VecDeque};
use pgrx::prelude::*;
use crate::TViewResult;
use crate::catalog::TviewMeta;

/// Entity dependency graph for refresh ordering
///
/// Example:
/// - `tv_company` (no dependencies)
/// - `tv_user` (depends on `tv_company` via `fk_company`)
/// - `tv_post` (depends on `tv_user` via `fk_user`)
/// - `tv_feed` (depends on `tv_post` via `fk_post`)
///
/// Topological order: ["company", "user", "post", "feed"]
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// Parent relationships: entity -> list of entities that depend on it
    /// Example: "user" -> ["post", "feed"]
    #[allow(dead_code)]
    pub parents: HashMap<String, Vec<String>>,

    /// Child relationships: entity -> list of entities it depends on
    /// Example: "post" -> ["user"]
    #[allow(dead_code)]
    pub children: HashMap<String, Vec<String>>,

    /// Topological order (refresh from low to high dependency)
    /// Example: ["company", "user", "post", "feed"]
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
                        error: format!("Failed to get entity: {}", e),
                    })?
                    .ok_or_else(|| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: "entity column is NULL".to_string(),
                    })?;
                let fk_columns: Option<Vec<String>> = row["fk_columns"].value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get fk_columns: {}", e),
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
            .iter().map(|s| s.to_string()).collect();

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

    #[test]
    fn test_batch_analysis() {
        let graph = EntityDepGraph::load().unwrap();

        let mut estimated_rows = HashMap::new();
        estimated_rows.insert("post".to_string(), 100);
        estimated_rows.insert("comment".to_string(), 500);

        let analysis = graph.analyze_batch_potential(
            "user",
            &["post".to_string(), "comment".to_string()],
            &estimated_rows,
        );

        // Should detect large row counts as batch candidates
        assert!(analysis.total_entities >= 2);
        assert!(analysis.total_estimated_rows >= 600);
    }
}

/// Batch cascade detection and optimization
///
/// This module provides utilities for detecting when batch operations
/// would provide performance benefits for cascade scenarios.
impl EntityDepGraph {
    /// Analyze cascade scenario to determine if batch operations are beneficial
    ///
    /// # Arguments
    /// * `source_entity` - Entity that triggered the cascade
    /// * `affected_entities` - Entities that need refreshing
    /// * `estimated_rows` - Estimated number of rows per entity
    ///
    /// # Returns
    /// Batch optimization recommendations
    #[allow(dead_code)]  // Phase 3: Analysis method for future optimization
    pub fn analyze_batch_potential(
        &self,
        _source_entity: &str,
        affected_entities: &[String],
        estimated_rows: &HashMap<String, usize>,
    ) -> BatchAnalysis {
        let mut analysis = BatchAnalysis::default();

        for entity in affected_entities {
            let row_count = estimated_rows.get(entity).copied().unwrap_or(0);

            // Check if entity has array dependencies that could benefit from batch updates
            if let Ok(Some(meta)) = TviewMeta::load_by_entity(entity) {
                let has_array_deps = meta.dependency_types.iter().any(|dt| dt == &crate::catalog::DependencyType::Array);

                if has_array_deps && row_count > 10 {
                    analysis.batch_candidates.push(entity.clone());
                    analysis.estimated_savings += row_count * 3; // Rough estimate: 3x speedup
                }
            }

            // Check for large row counts that benefit from batch refresh
            if row_count >= 50 {
                analysis.large_refresh_candidates.push(entity.clone());
                analysis.estimated_savings += row_count * 4; // Rough estimate: 4x speedup for large batches
            }
        }

        analysis.total_entities = affected_entities.len();
        analysis.total_estimated_rows = estimated_rows.values().sum();

        analysis
    }
}

/// Analysis result for batch optimization potential
#[derive(Debug, Clone, Default)]
pub struct BatchAnalysis {
    /// Entities that would benefit from batch array updates
    pub batch_candidates: Vec<String>,
    /// Entities that would benefit from batch row refresh
    pub large_refresh_candidates: Vec<String>,
    /// Total entities in cascade
    #[allow(dead_code)]  // Phase 3: Populated for future use
    pub total_entities: usize,
    /// Total estimated rows across all entities
    #[allow(dead_code)]  // Phase 3: Populated for future use
    pub total_estimated_rows: usize,
    /// Estimated performance improvement (operations saved)
    #[allow(dead_code)]  // Phase 3: Populated for future use
    pub estimated_savings: usize,
}

impl BatchAnalysis {
    /// Determine if batch operations are recommended
    #[allow(dead_code)]  // Phase 3: Analysis methods for future optimization
    pub const fn should_use_batch(&self) -> bool {
        !self.batch_candidates.is_empty() || !self.large_refresh_candidates.is_empty()
    }

    /// Get recommended batch strategy
    #[allow(dead_code)]  // Phase 3: Analysis methods for future optimization
    pub const fn recommended_strategy(&self) -> BatchStrategy {
        if !self.batch_candidates.is_empty() && !self.large_refresh_candidates.is_empty() {
            BatchStrategy::Hybrid
        } else if !self.batch_candidates.is_empty() {
            BatchStrategy::ArrayBatch
        } else if !self.large_refresh_candidates.is_empty() {
            BatchStrategy::RowBatch
        } else {
            BatchStrategy::Individual
        }
    }
}

/// Recommended batch strategy for cascade operations
#[allow(dead_code)]  // Phase 3: Strategy enum for future optimization
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BatchStrategy {
    /// Use individual operations (default for small cascades)
    Individual,
    /// Use batch array updates for array dependencies
    ArrayBatch,
    /// Use batch row refresh for large row counts
    RowBatch,
    /// Use both array batch and row batch optimizations
    Hybrid,
}