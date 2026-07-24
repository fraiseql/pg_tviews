use crate::TViewResult;
use crate::cascade_path::CascadePath;
use pgrx::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};

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
    /// Propagation parents: entity -> list of entities that must be *entity-level*
    /// refreshed (via [`crate::propagate`]) when it changes during flush.
    ///
    /// An edge is kept when the parent embeds part of this entity's *computed*
    /// document that a base-table cascade path cannot cover on its own — i.e. a
    /// `nested_object`/`array` embed, or a scalar embed that follows one of this
    /// entity's foreign keys to reach a deeper relationship.
    ///
    /// **A scalar embed that reads only the child's own (non-FK) columns is excluded.**
    /// Such an embed (e.g. `jsonb_build_object('title', p.title)`) can change only when
    /// a child base-table column changes, which already fires the child's trigger and
    /// cascades through the column-aware `tb_<child>` path — entity-level propagation is
    /// then pure redundancy, and worse, it fires even when the child was recomputed for
    /// an *unrelated* deeper embed (a post recomputed for an author-bio change would
    /// otherwise recompute every comment that embeds only the post's `{id, title}`).
    /// See [`EntityDepGraph::load`].
    pub parents: HashMap<String, Vec<String>>,

    /// Child relationships: entity -> list of entities it depends on
    /// Example: "post" -> `["user"]`
    #[allow(dead_code)] // Reason: public API for graph introspection; populated during load()
    pub children: HashMap<String, Vec<String>>,

    /// Topological order (refresh from low to high dependency)
    /// Example: `["company", "user", "post", "feed"]`
    pub topo_order: Vec<String>,
}

impl EntityDepGraph {
    /// Build dependency graph from `pg_tview_meta`
    pub fn load() -> TViewResult<Self> {
        // `fk_columns[i]` names the relationship this entity embeds; `dependency_types[i]`
        // classifies that embed (scalar / nested_object / array); `cascade_paths` records,
        // per source table, the exact source columns the embed reads. All three feed the
        // decision below: `parents` (which drives flush-time entity propagation) drops a
        // scalar embed reading only the child's own columns, while `children` (which drives
        // topological refresh ordering) keeps every edge.
        let query = "SELECT entity, fk_columns, dependency_types, cascade_paths FROM pg_tview_meta";

        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_entities: HashSet<String> = HashSet::new();

        Spi::connect(|client| {
            let rows = client.select(query, None, &[])?;

            for row in rows {
                let entity: String = row["entity"]
                    .value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get entity: {e}"),
                    })?
                    .ok_or_else(|| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: "entity column is NULL".to_string(),
                    })?;
                let fk_columns: Option<Vec<String>> =
                    row["fk_columns"]
                        .value()
                        .map_err(|e| crate::TViewError::SpiError {
                            query: query.to_string(),
                            error: format!("Failed to get fk_columns: {e}"),
                        })?;
                // `dependency_types[i]` is positionally aligned with `fk_columns[i]`
                // (same contract as `TviewMeta::parse_dependencies`).
                let dependency_types: Vec<String> = row["dependency_types"]
                    .value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get dependency_types: {e}"),
                    })?
                    .unwrap_or_default();
                let cascade_paths_raw: Vec<String> = row["cascade_paths"]
                    .value()
                    .map_err(|e| crate::TViewError::SpiError {
                        query: query.to_string(),
                        error: format!("Failed to get cascade_paths: {e}"),
                    })?
                    .unwrap_or_default();

                // Deserialize with the same typed struct the trigger path consumes. A
                // path we cannot parse simply doesn't contribute to `reads_by_fk`, which
                // keeps the corresponding edge — the safe default. (The runtime cascade
                // path has its own deserialization, so a real parse failure surfaces
                // there, not silently here.)
                let cascade_paths: Vec<CascadePath> = cascade_paths_raw
                    .iter()
                    .filter_map(|s| serde_json::from_str(s).ok())
                    .collect();
                let reads_by_fk = source_columns_by_fk(&cascade_paths);

                all_entities.insert(entity.clone());

                if let Some(fk_cols) = fk_columns {
                    for (i, fk_col) in fk_cols.iter().enumerate() {
                        // FK column format: "fk_<entity>"
                        // Example: "fk_user" -> "user"
                        if let Some(parent_entity) = fk_col.strip_prefix("fk_") {
                            // Topological ordering must respect every dependency, so
                            // `children` records the edge unconditionally.
                            children
                                .entry(entity.clone())
                                .or_default()
                                .push(parent_entity.to_string());

                            // Skip flush-time entity propagation for a scalar embed that
                            // reads ONLY the child's own (non-FK) columns: it is already
                            // covered by the column-aware `tb_<child>` cascade path, so
                            // propagating here is redundant and over-refreshes. Keep the
                            // edge for nested_object/array embeds, for scalar embeds that
                            // follow a child FK (a deeper relationship no `tb_<child>` path
                            // covers), and whenever the classification or columns are
                            // unknown — the safe default.
                            let is_scalar =
                                dependency_types.get(i).is_some_and(|t| t.as_str() == "scalar");
                            let reads_only_own_columns =
                                reads_by_fk.get(fk_col).is_some_and(|cols| {
                                    !cols.is_empty() && !cols.iter().any(|c| c.starts_with("fk_"))
                                });
                            if !(is_scalar && reads_only_own_columns) {
                                parents
                                    .entry(parent_entity.to_string())
                                    .or_default()
                                    .push(entity.clone());
                            }
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
    /// Within each entity group, insertion order is preserved.
    /// Both PK and dedup keys are retained as-is.
    pub fn sort_keys(&self, keys: Vec<super::key::RefreshKey>) -> Vec<super::key::RefreshKey> {
        // Group by entity, preserving full RefreshKey values
        let mut groups: HashMap<String, Vec<super::key::RefreshKey>> = HashMap::new();
        for key in keys {
            groups.entry(key.entity.clone()).or_default().push(key);
        }

        // Emit groups in topological order
        let mut sorted_keys = Vec::new();
        for entity in &self.topo_order {
            if let Some(ks) = groups.remove(entity) {
                sorted_keys.extend(ks);
            }
        }

        sorted_keys
    }
}

/// Map each embedded relationship's FK column to the base-table source columns its
/// embed reads, taken from this entity's cascade paths.
///
/// A cascade path refreshes this entity when a source table changes; its final step
/// into this entity's own base table matches `lookup_col == fk_<child>` — the last
/// hop's `lookup_col`, or `initial_col` when the path has no hops. That FK column keys
/// the path's `source_columns`. Paths landing on the same FK are unioned, so a later
/// empty (multi-hop) path never masks a direct path's FK reference.
fn source_columns_by_fk(paths: &[CascadePath]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for path in paths {
        let fk = path
            .hops
            .last()
            .map_or(path.initial_col.as_str(), |h| h.lookup_col.as_str());
        out.entry(fk.to_string())
            .or_default()
            .extend(path.source_columns.iter().cloned());
    }
    out
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

    // Check for cycles (only count entities in the original set;
    // FK references to non-TVIEW entities like "user" are external and shouldn't
    // cause cycle detection failures)
    let result_in_set = result.iter().filter(|e| entities.contains(*e)).count();
    if result_in_set != entities.len() {
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
    fn test_sort_keys_preserves_dedup_keys() {
        // Build a simple graph: company -> user -> post
        let graph = EntityDepGraph {
            parents: HashMap::new(),
            children: HashMap::new(),
            topo_order: vec!["company".into(), "user".into(), "post".into()],
        };

        let keys = vec![
            super::super::key::RefreshKey::pk("post", 10),
            super::super::key::RefreshKey::dedup("user", "some-uuid"),
            super::super::key::RefreshKey::pk("company", 1),
            super::super::key::RefreshKey::pk("user", 42),
            super::super::key::RefreshKey::dedup("post", "dedup-val"),
        ];

        let sorted = graph.sort_keys(keys);

        // All 5 keys must be present
        assert_eq!(sorted.len(), 5);

        // Dedup keys must survive with their dedup_key field intact
        let dedup_keys: Vec<_> = sorted.iter().filter(|k| k.is_dedup()).collect();
        assert_eq!(dedup_keys.len(), 2);

        // Verify specific dedup keys are present with correct fields
        assert!(sorted.contains(&super::super::key::RefreshKey::dedup("user", "some-uuid")));
        assert!(sorted.contains(&super::super::key::RefreshKey::dedup("post", "dedup-val")));

        // Verify topological order: company entities before user, user before post
        let first_company = sorted.iter().position(|k| k.entity == "company").unwrap();
        let first_user = sorted.iter().position(|k| k.entity == "user").unwrap();
        let first_post = sorted.iter().position(|k| k.entity == "post").unwrap();
        assert!(first_company < first_user);
        assert!(first_user < first_post);
    }

    #[test]
    fn test_topological_sort() {
        // Entity graph:
        // company (no deps)
        // user -> company
        // post -> user
        // feed -> post

        let entities: HashSet<String> = ["company", "user", "post", "feed"]
            .iter()
            .map(|&s| s.to_string())
            .collect();

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

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod pg_tests {
    use super::*;
    use pgrx::prelude::Spi;

    /// Column-aware multi-hop propagation graph shape:
    /// - `nested_object` embed (post embeds `v_user.data`) → propagation parent (user → post).
    /// - `scalar` embed reading only the child's own columns (shallow embeds only
    ///   `post.title`) → NOT a propagation parent; the `tb_post` cascade path covers it.
    /// - `scalar` embed following a child FK (deep embeds `post.author`, reading the
    ///   post's `fk_author`) → still a propagation parent; no `tb_post` path covers the
    ///   two-level author change.
    ///
    /// All three edges stay in `children`, so topological ordering is unchanged.
    #[pg_test]
    fn test_scalar_embed_propagation_is_fk_aware() {
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run(
            "CREATE TABLE tb_post (pk_post BIGSERIAL PRIMARY KEY, \
             fk_author BIGINT REFERENCES tb_user(pk_user), title TEXT)",
        )
        .unwrap();
        // shallow: embeds only the post's own title (scalar, no child FK read)
        Spi::run(
            "CREATE TABLE tb_shallow (pk_shallow BIGSERIAL PRIMARY KEY, \
             fk_post BIGINT REFERENCES tb_post(pk_post), body TEXT)",
        )
        .unwrap();
        // deep: embeds the post AND the post's author (scalar, reads post.fk_author)
        Spi::run(
            "CREATE TABLE tb_deep (pk_deep BIGSERIAL PRIMARY KEY, \
             fk_post BIGINT REFERENCES tb_post(pk_post), body TEXT)",
        )
        .unwrap();

        Spi::run(
            "SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data FROM tb_user
            $$)",
        )
        .unwrap();
        // post embeds the whole computed user document → nested_object dependency
        Spi::run(
            "SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_author,
                       jsonb_build_object('title', title, 'author', v_user.data) AS data
                FROM tb_post LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_author
            $$)",
        )
        .unwrap();
        // shallow: scalar embed of only the post's title → base path covers it
        Spi::run(
            "SELECT pg_tviews_create('shallow', $$
                SELECT pk_shallow, fk_post,
                       jsonb_build_object('body', body, 'post',
                           jsonb_build_object('title', tb_post.title)) AS data
                FROM tb_shallow JOIN tb_post ON tb_post.pk_post = tb_shallow.fk_post
            $$)",
        )
        .unwrap();
        // deep: scalar embed that follows post.fk_author to embed post.author
        Spi::run(
            "SELECT pg_tviews_create('deep', $$
                SELECT pk_deep, fk_post,
                       jsonb_build_object('body', body, 'post',
                           jsonb_build_object('title', p.title,
                               'author', jsonb_build_object('name', pu.name))) AS data
                FROM tb_deep d
                JOIN tb_post p  ON p.pk_post = d.fk_post
                JOIN tb_user pu ON pu.pk_user = p.fk_author
            $$)",
        )
        .unwrap();

        let graph = EntityDepGraph::load().unwrap();
        let post_parents = graph.parents.get("post").cloned().unwrap_or_default();
        let user_parents = graph.parents.get("user").cloned().unwrap_or_default();

        // nested_object: user -> post propagates
        assert!(
            user_parents.contains(&"post".to_string()),
            "nested_object embed (post←user) must propagate; got {user_parents:?}"
        );
        // scalar own-columns: post -> shallow must NOT propagate
        assert!(
            !post_parents.contains(&"shallow".to_string()),
            "scalar own-column embed (shallow←post) must be excluded; got {post_parents:?}"
        );
        // scalar following a child FK: post -> deep MUST propagate
        assert!(
            post_parents.contains(&"deep".to_string()),
            "scalar embed reading post.fk_author (deep←post) must propagate; got {post_parents:?}"
        );

        // Every edge stays in `children` so topo ordering is preserved.
        for child in ["shallow", "deep"] {
            assert!(
                graph
                    .children
                    .get(child)
                    .is_some_and(|c| c.contains(&"post".to_string())),
                "topo children must retain edge ({child}→post)"
            );
            assert!(
                graph.topo_order.iter().position(|e| e == "post")
                    < graph.topo_order.iter().position(|e| e == child),
                "topo order must refresh post before {child}"
            );
        }
    }
}
