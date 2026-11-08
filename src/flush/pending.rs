//! The keys a flush has yet to refresh, held per entity, the entities in
//! dependency order: the next entity is the first one, found without sorting
//! the keys (#298).

use crate::catalog::EntityDepGraph;
use crate::queue::RefreshKey;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

/// Keys waiting for their refresh: by `(rank, entity)`, the rank an entity's
/// place in the graph's topological order (entities the graph does not know
/// after every known one, by name).
pub(super) struct Pending {
    graph: Rc<EntityDepGraph>,
    by_entity: BTreeMap<(usize, String), HashSet<RefreshKey>>,
}

impl Pending {
    pub(super) const fn new(graph: Rc<EntityDepGraph>) -> Self {
        Self {
            graph,
            by_entity: BTreeMap::new(),
        }
    }

    /// Queue `key`; a key queued already is held once.
    pub(super) fn insert(&mut self, key: RefreshKey) {
        let rank = self.graph.rank(&key.entity);
        self.by_entity
            .entry((rank, key.entity.clone()))
            .or_default()
            .insert(key);
    }

    pub(super) fn contains(&self, key: &RefreshKey) -> bool {
        self.by_entity
            .get(&(self.graph.rank(&key.entity), key.entity.clone()))
            .is_some_and(|keys| keys.contains(key))
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.by_entity.is_empty()
    }

    /// The first entity in dependency order, with its keys; the others stay.
    pub(super) fn pop_first(&mut self) -> Option<(String, HashSet<RefreshKey>)> {
        self.by_entity
            .pop_first()
            .map(|((_, entity), keys)| (entity, keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(order: &[&str]) -> Rc<EntityDepGraph> {
        Rc::new(EntityDepGraph::ordered(
            order.iter().map(|e| (*e).to_string()).collect(),
        ))
    }

    fn drain(pending: &mut Pending) -> Vec<(String, Vec<i64>)> {
        let mut out = Vec::new();
        while let Some((entity, keys)) = pending.pop_first() {
            let mut pks: Vec<i64> = keys.iter().filter_map(|k| k.key.as_int()).collect();
            pks.sort_unstable();
            out.push((entity, pks));
        }
        out
    }

    #[test]
    fn entities_come_in_topological_order() {
        let mut pending = Pending::new(graph(&["company", "user", "post"]));
        for (entity, pk) in [
            ("post", 10),
            ("user", 2),
            ("company", 1),
            ("user", 42),
            ("post", 3),
        ] {
            pending.insert(RefreshKey::pk(entity, pk));
        }
        assert_eq!(
            drain(&mut pending),
            vec![
                ("company".to_string(), vec![1]),
                ("user".to_string(), vec![2, 42]),
                ("post".to_string(), vec![3, 10]),
            ]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn entities_missing_from_the_graph_come_last_by_name() {
        let mut pending = Pending::new(graph(&["user"]));
        for (entity, pk) in [("zeta", 1), ("alpha", 2), ("user", 3)] {
            pending.insert(RefreshKey::pk(entity, pk));
        }
        let order: Vec<String> = drain(&mut pending).into_iter().map(|(e, _)| e).collect();
        assert_eq!(order, vec!["user", "alpha", "zeta"]);
    }

    #[test]
    fn a_key_queued_twice_is_held_once_and_contains_finds_it() {
        let mut pending = Pending::new(graph(&["user", "post"]));
        pending.insert(RefreshKey::pk("post", 1));
        pending.insert(RefreshKey::pk("post", 1));
        assert!(pending.contains(&RefreshKey::pk("post", 1)));
        assert!(!pending.contains(&RefreshKey::pk("post", 2)));
        assert!(!pending.contains(&RefreshKey::pk("user", 1)));
        assert_eq!(drain(&mut pending), vec![("post".to_string(), vec![1])]);
    }

    #[test]
    fn a_key_queued_after_its_entity_was_taken_comes_again_in_order() {
        // A late write re-queues an earlier entity: it is next, before later ones.
        let mut pending = Pending::new(graph(&["user", "post", "feed"]));
        pending.insert(RefreshKey::pk("user", 1));
        pending.insert(RefreshKey::pk("feed", 9));
        assert_eq!(pending.pop_first().map(|(e, _)| e).as_deref(), Some("user"));
        pending.insert(RefreshKey::pk("post", 5));
        pending.insert(RefreshKey::pk("user", 1));
        let order: Vec<String> = drain(&mut pending).into_iter().map(|(e, _)| e).collect();
        assert_eq!(order, vec!["user", "post", "feed"]);
    }
}
