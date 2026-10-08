use crate::TViewResult;
use std::collections::{HashMap, HashSet, VecDeque};

/// The TVIEWs' dependencies on each other, read from their plans (ADR 0203).
///
/// Example: `tv_post` embeds `tv_user`, `tv_feed` embeds `tv_post`; the
/// topological order is `["user", "post", "feed"]`.
#[derive(Debug, Clone)]
pub struct EntityDepGraph {
    /// Propagation parents: entity -> the entities embedding it, refreshed (via
    /// [`crate::propagate`]) when its rows change during a flush. A TVIEW reading
    /// another one's *base tables* is no parent: its own triggers see those writes.
    pub parents: HashMap<String, Vec<String>>,

    /// Child relationships: entity -> the entities it reads (embeds, and TVIEW
    /// tables it maps like base tables), refreshed before it.
    pub children: HashMap<String, Vec<String>>,

    /// `(child, parent)` edges where the parent embeds the child's computed document
    /// (a nested or array embed). Along these, a child row whose refresh changed
    /// nothing cannot change the parent.
    pub document_edges: HashSet<(String, String)>,

    /// Topological order (refresh from low to high dependency)
    /// Example: `["company", "user", "post", "feed"]`
    pub topo_order: Vec<String>,

    /// `(child, parent)` → the parent's output columns equal to the child's key:
    /// the parents of a refreshed child row are the rows holding its key in one.
    pub lookup_columns: HashMap<(String, String), Vec<String>>,
}

impl EntityDepGraph {
    /// Build the graph from every TVIEW's plan.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read, or a cycle among TVIEWs.
    pub fn load() -> TViewResult<Self> {
        Self::from_metas(&crate::catalog::TviewMeta::load_all()?)
    }

    /// Build the graph from `metas`.
    ///
    /// # Errors
    /// A [`crate::TViewError::DependencyCycle`] when the TVIEWs read each other.
    pub fn from_metas(metas: &[crate::catalog::TviewMeta]) -> TViewResult<Self> {
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_entities: HashSet<String> = HashSet::new();
        let mut document_edges: HashSet<(String, String)> = HashSet::new();
        let mut lookup_columns: HashMap<(String, String), Vec<String>> = HashMap::new();

        for meta in metas {
            let entity = &meta.entity_name;
            all_entities.insert(entity.clone());
            for embed in &meta.plan.embeds {
                children
                    .entry(entity.clone())
                    .or_default()
                    .push(embed.entity.clone());
                parents
                    .entry(embed.entity.clone())
                    .or_default()
                    .push(entity.clone());
                if embed.kind != crate::lineage::EmbedKind::Scalar {
                    document_edges.insert((embed.entity.clone(), entity.clone()));
                }
                lookup_columns.insert(
                    (embed.entity.clone(), entity.clone()),
                    embed.lookups.clone(),
                );
            }
            // A TVIEW whose refreshes this one maps like writes is
            // refreshed first.
            for table in &meta.plan.tables {
                if let Some(inner) = &table.tview
                    && table.kind != crate::lineage::MappingKind::Propagated
                {
                    children
                        .entry(entity.clone())
                        .or_default()
                        .push(inner.clone());
                }
            }
        }

        let topo_order = topological_sort(&all_entities, &children)?;
        Ok(Self {
            parents,
            children,
            document_edges,
            topo_order,
            lookup_columns,
        })
    }

    /// The columns of `tv_<parent>` holding the key of a `child` row (none when
    /// `parent` does not embed `child`).
    #[must_use]
    pub fn lookup_columns(&self, child: &str, parent: &str) -> &[String] {
        self.lookup_columns
            .get(&(child.to_string(), parent.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Sort refresh keys by dependency order
    ///
    /// Keys are grouped by entity, then sorted by `topo_order`.
    /// Within each entity group, insertion order is preserved.
    /// Integer and text keys are retained as-is.
    pub fn sort_keys(
        &self,
        keys: Vec<crate::queue::key::RefreshKey>,
    ) -> Vec<crate::queue::key::RefreshKey> {
        // Group by entity, preserving full RefreshKey values
        let mut groups: HashMap<String, Vec<crate::queue::key::RefreshKey>> = HashMap::new();
        for key in keys {
            groups.entry(key.entity.clone()).or_default().push(key);
        }

        // Emit groups in topological order, then any entity the graph does not
        // know (never drop a key)
        let mut sorted_keys = Vec::new();
        for entity in &self.topo_order {
            if let Some(ks) = groups.remove(entity) {
                sorted_keys.extend(ks);
            }
        }
        let mut rest: Vec<_> = groups.into_iter().collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        sorted_keys.extend(rest.into_iter().flat_map(|(_, ks)| ks));

        sorted_keys
    }
}

/// Topological sort using Kahn's algorithm, dependencies first.
///
/// `depends_on[x]` lists the entities `x` reads; `x` is emitted after all of them.
/// Dependencies that are not TVIEWs (one dropped meanwhile) are ignored.
fn topological_sort(
    entities: &HashSet<String>,
    depends_on: &HashMap<String, Vec<String>>,
) -> TViewResult<Vec<String>> {
    // In-degree: how many TVIEW dependencies each entity still waits for.
    // `dependents[d]` lists the entities waiting on `d`.
    let mut in_degree: HashMap<&str, usize> = entities.iter().map(|e| (e.as_str(), 0)).collect();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for entity in entities {
        let deps: HashSet<&str> = depends_on
            .get(entity)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .filter(|d| entities.contains(*d))
            .collect();
        for dep in deps {
            *in_degree.entry(entity.as_str()).or_insert(0) += 1;
            dependents.entry(dep).or_default().push(entity.as_str());
        }
    }

    // Start with entities that have no dependencies (sorted: a deterministic order)
    let mut ready: Vec<&str> = in_degree
        .iter()
        .filter(|&(_, &degree)| degree == 0)
        .map(|(&e, _)| e)
        .collect();
    ready.sort_unstable();
    let mut queue: VecDeque<&str> = ready.into();

    let mut result = Vec::with_capacity(entities.len());
    while let Some(entity) = queue.pop_front() {
        result.push(entity.to_string());

        let mut unblocked = Vec::new();
        for &dependent in dependents.get(entity).into_iter().flatten() {
            if let Some(degree) = in_degree.get_mut(dependent) {
                *degree -= 1;
                if *degree == 0 {
                    unblocked.push(dependent);
                }
            }
        }
        unblocked.sort_unstable();
        queue.extend(unblocked);
    }

    if result.len() != entities.len() {
        // What never became ready: the cycle, and whatever waits on it.
        let mut stuck: Vec<String> = in_degree
            .into_iter()
            .filter(|&(_, degree)| degree > 0)
            .map(|(e, _)| e.to_string())
            .collect();
        stuck.sort_unstable();
        return Err(crate::TViewError::DependencyCycle { entities: stuck });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_key(entity: &str, value: &str) -> crate::queue::key::RefreshKey {
        crate::queue::key::RefreshKey::new(
            entity,
            crate::queue::key::KeyValue::Text(value.to_string()),
        )
    }

    #[test]
    fn test_sort_keys_preserves_text_keys() {
        // Build a simple graph: company -> user -> post
        let graph = EntityDepGraph {
            parents: HashMap::new(),
            children: HashMap::new(),
            document_edges: HashSet::new(),
            topo_order: vec!["company".into(), "user".into(), "post".into()],
            lookup_columns: HashMap::new(),
        };

        let keys = vec![
            crate::queue::key::RefreshKey::pk("post", 10),
            text_key("user", "some-uuid"),
            crate::queue::key::RefreshKey::pk("company", 1),
            crate::queue::key::RefreshKey::pk("user", 42),
            text_key("post", "text-val"),
        ];

        let sorted = graph.sort_keys(keys);

        // All 5 keys must be present
        assert_eq!(sorted.len(), 5);

        // Text keys must survive with their value intact
        assert!(sorted.contains(&text_key("user", "some-uuid")));
        assert!(sorted.contains(&text_key("post", "text-val")));

        // Verify topological order: company entities before user, user before post
        let first_company = sorted.iter().position(|k| k.entity == "company").unwrap();
        let first_user = sorted.iter().position(|k| k.entity == "user").unwrap();
        let first_post = sorted.iter().position(|k| k.entity == "post").unwrap();
        assert!(first_company < first_user);
        assert!(first_user < first_post);
    }

    fn depends_on(edges: &[(&str, &str)]) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (entity, dep) in edges {
            map.entry((*entity).to_string())
                .or_default()
                .push((*dep).to_string());
        }
        map
    }

    fn entity_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|&s| s.to_string()).collect()
    }

    #[test]
    fn test_topological_sort() {
        // feed reads post, post reads user, user reads company — the orientation
        // `EntityDepGraph::load` records (`children[x]` = what `x` depends on).
        let entities = entity_set(&["company", "user", "post", "feed"]);
        let edges = depends_on(&[("user", "company"), ("post", "user"), ("feed", "post")]);

        let topo = topological_sort(&entities, &edges).unwrap();

        assert_eq!(topo, ["company", "user", "post", "feed"]);
    }

    #[test]
    fn topological_sort_diamond_puts_shared_dependency_first() {
        // report reads post and comment; both read user.
        let entities = entity_set(&["report", "post", "comment", "user"]);
        let edges = depends_on(&[
            ("report", "post"),
            ("report", "comment"),
            ("post", "user"),
            ("comment", "user"),
        ]);

        let topo = topological_sort(&entities, &edges).unwrap();

        assert_eq!(topo, ["user", "comment", "post", "report"]);
    }

    #[test]
    fn topological_sort_ignores_dependencies_that_are_not_tviews() {
        // post has fk_user but there is no user TVIEW.
        let entities = entity_set(&["post", "comment"]);
        let edges = depends_on(&[("post", "user"), ("comment", "post")]);

        let topo = topological_sort(&entities, &edges).unwrap();

        assert_eq!(topo, ["post", "comment"]);
    }

    #[test]
    fn topological_sort_rejects_cycles() {
        let entities = entity_set(&["a", "b"]);
        let edges = depends_on(&[("a", "b"), ("b", "a")]);

        assert!(topological_sort(&entities, &edges).is_err());
    }

    #[test]
    fn sort_keys_keeps_entities_missing_from_the_graph() {
        let graph = EntityDepGraph {
            parents: HashMap::new(),
            children: HashMap::new(),
            document_edges: HashSet::new(),
            topo_order: vec!["user".into()],
            lookup_columns: HashMap::new(),
        };
        let keys = vec![
            crate::queue::key::RefreshKey::pk("unknown", 1),
            crate::queue::key::RefreshKey::pk("user", 2),
        ];

        let sorted = graph.sort_keys(keys);

        assert_eq!(
            sorted,
            [
                crate::queue::key::RefreshKey::pk("user", 2),
                crate::queue::key::RefreshKey::pk("unknown", 1),
            ]
        );
    }

    fn meta(
        entity: &str,
        embeds: &[(&str, crate::lineage::EmbedKind, &str)],
    ) -> crate::catalog::TviewMeta {
        let mut meta = crate::catalog::TviewMeta {
            entity_name: entity.to_string(),
            ..crate::catalog::TviewMeta::default()
        };
        for (child, kind, lookup) in embeds {
            meta.plan.embeds.push(crate::catalog::plan::PlanEmbed {
                entity: (*child).to_string(),
                lookups: vec![(*lookup).to_string()],
                kind: *kind,
                path: Vec::new(),
            });
        }
        meta
    }

    /// Edges come from the plans' embeds only: a TVIEW reading another one's base
    /// tables (no embed) is no parent of it, whatever its columns are called.
    #[test]
    fn graph_edges_are_the_plans_embeds() {
        use crate::lineage::EmbedKind;
        let graph = EntityDepGraph::from_metas(&[
            meta("user", &[]),
            meta("post", &[("user", EmbedKind::Nested, "author_pk")]),
            meta("feed", &[("post", EmbedKind::Scalar, "fk_post")]),
            meta("shallow", &[]),
        ])
        .unwrap();
        assert_eq!(graph.parents["user"], vec!["post".to_string()]);
        assert_eq!(graph.parents["post"], vec!["feed".to_string()]);
        assert!(!graph.parents.values().flatten().any(|p| p == "shallow"));
        assert_eq!(
            graph.lookup_columns("user", "post"),
            ["author_pk".to_string()]
        );
        assert!(graph.lookup_columns("user", "feed").is_empty());
        assert!(
            graph
                .document_edges
                .contains(&("user".into(), "post".into()))
        );
        assert!(
            !graph
                .document_edges
                .contains(&("post".into(), "feed".into()))
        );
        let at = |e: &str| graph.topo_order.iter().position(|x| x == e).unwrap();
        assert!(at("user") < at("post") && at("post") < at("feed"));
    }

    #[test]
    fn embeds_in_a_cycle_are_refused() {
        use crate::lineage::EmbedKind;
        let result = EntityDepGraph::from_metas(&[
            meta("a", &[("b", EmbedKind::Scalar, "b_pk")]),
            meta("b", &[("a", EmbedKind::Scalar, "a_pk")]),
        ]);
        assert!(matches!(
            result,
            Err(crate::TViewError::DependencyCycle { .. })
        ));
    }
}
