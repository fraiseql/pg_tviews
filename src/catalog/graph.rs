use crate::TViewResult;
use std::collections::{HashMap, HashSet, VecDeque};

/// The TVIEWs' dependencies on each other, read from their plans (ADR 0203).
///
/// Example: `tv_post` embeds `tv_user`, `tv_feed` embeds `tv_post`; the
/// topological order is `["user", "post", "feed"]`.
#[derive(Debug)]
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

    /// Entity → its place in [`Self::topo_order`].
    ranks: HashMap<String, usize>,
}

impl EntityDepGraph {
    /// Build the graph from every TVIEW's plan.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read, or a cycle among TVIEWs.
    pub fn load() -> TViewResult<Self> {
        Self::from_plans(&crate::catalog::TviewMeta::load_plans()?)
    }

    /// Build the graph from the TVIEWs' plans.
    ///
    /// # Errors
    /// A [`crate::TViewError::DependencyCycle`] when the TVIEWs read each other.
    pub fn from_plans(metas: &[crate::catalog::PlanEntry]) -> TViewResult<Self> {
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_entities: HashSet<String> = HashSet::new();
        let mut document_edges: HashSet<(String, String)> = HashSet::new();
        let mut lookup_columns: HashMap<(String, String), Vec<String>> = HashMap::new();

        for meta in metas {
            let entity = &meta.entity;
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
            ranks: ranks(&topo_order),
            topo_order,
            lookup_columns,
        })
    }

    /// A graph of no edges whose entities come in `topo_order`.
    #[cfg(test)]
    pub fn ordered(topo_order: Vec<String>) -> Self {
        Self {
            parents: HashMap::new(),
            children: HashMap::new(),
            document_edges: HashSet::new(),
            ranks: ranks(&topo_order),
            topo_order,
            lookup_columns: HashMap::new(),
        }
    }

    /// `entity`'s place in the topological order; `usize::MAX` for an entity the
    /// graph does not know, which comes after every known one.
    #[must_use]
    pub fn rank(&self, entity: &str) -> usize {
        self.ranks.get(entity).copied().unwrap_or(usize::MAX)
    }

    /// The columns of `tv_<parent>` holding the key of a `child` row (none when
    /// `parent` does not embed `child`).
    #[must_use]
    pub fn lookup_columns(&self, child: &str, parent: &str) -> &[String] {
        self.lookup_columns
            .get(&(child.to_string(), parent.to_string()))
            .map_or(&[], Vec::as_slice)
    }
}

/// Each entity of `topo_order` with its place in it.
fn ranks(topo_order: &[String]) -> HashMap<String, usize> {
    topo_order
        .iter()
        .enumerate()
        .map(|(i, entity)| (entity.clone(), i))
        .collect()
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

    fn meta(
        entity: &str,
        embeds: &[(&str, crate::lineage::EmbedKind, &str)],
    ) -> crate::catalog::PlanEntry {
        let mut meta = crate::catalog::PlanEntry {
            entity: entity.to_string(),
            ..crate::catalog::PlanEntry::default()
        };
        for (child, kind, lookup) in embeds {
            meta.plan.embeds.push(crate::lineage::plan::PlanEmbed {
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
        let graph = EntityDepGraph::from_plans(&[
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
        let result = EntityDepGraph::from_plans(&[
            meta("a", &[("b", EmbedKind::Scalar, "b_pk")]),
            meta("b", &[("a", EmbedKind::Scalar, "a_pk")]),
        ]);
        assert!(matches!(
            result,
            Err(crate::TViewError::DependencyCycle { .. })
        ));
    }
}
