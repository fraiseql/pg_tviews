//! How one entity's pending keys are applied, and their parents found.

use super::EntityDepGraph;
use crate::TViewResult;
use crate::catalog::TviewMeta;
use crate::queue::key::KeyValue;
use crate::queue::patch::{FanoutMap, PatchEntry, PatchState};
use crate::queue::{RefreshKey, patch};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The state of one flush.
pub(super) struct Flush {
    pub(super) graph: EntityDepGraph,
    /// Keys still to refresh.
    pub(super) pending: HashSet<RefreshKey>,
    /// The direct-patch chains of the pending keys.
    pub(super) patches: HashMap<RefreshKey, PatchState>,
    /// Keys refreshed in this flush.
    pub(super) processed: HashSet<RefreshKey>,
    /// Passes of the flush loop so far, starting at 1.
    pub(super) iteration: usize,
    /// The catalog rows read in this flush, by entity (`None`: not a TVIEW).
    metas: HashMap<String, Option<TviewMeta>>,
}

impl Flush {
    pub(super) fn new(
        graph: EntityDepGraph,
        pending: HashSet<RefreshKey>,
        patches: HashMap<RefreshKey, PatchState>,
    ) -> Self {
        let processed = HashSet::with_capacity(pending.len().max(16));
        Self {
            graph,
            pending,
            patches,
            processed,
            iteration: 1,
            metas: HashMap::new(),
        }
    }

    /// Bring `entity`'s rows named by `keys` to what its view computes, then queue
    /// the rows of the TVIEWs embedding the rows that changed.
    pub(super) fn apply_entity(&mut self, entity: &str, keys: Vec<RefreshKey>) -> TViewResult<()> {
        // The entity is read and written as the owner of its tv_* table, whoever
        // wrote to the base table.
        let _owner = crate::owner::AsOwner::of_entity(entity)?;
        if !crate::queue::ops::is_crash_recovery_checked(entity) {
            crate::queue::mark_crash_recovery_checked(entity);
            if crate::lifecycle::detect_post_crash_truncation(entity)? {
                // The TVIEW is empty but its view is not: fill it. No TRUNCATE, so
                // no ACCESS EXCLUSIVE lock held until the transaction ends.
                crate::admin::fill_empty_tview(entity)?;
            }
        }
        if keys.iter().any(RefreshKey::is_all) {
            return self.refresh_all(entity);
        }

        // Keys carrying a usable direct patch are written straight into
        // tv_<entity>; the others recompute. The setting is read again
        // here, so turning it off before the commit forces a recompute.
        let apply_enabled = crate::config::direct_patch_enabled();
        let mut patched: Vec<(i64, Vec<PatchEntry>)> = Vec::new();
        let mut recompute: Vec<RefreshKey> = Vec::new();
        for key in keys {
            if apply_enabled
                && let Some(pk) = key.key.as_int()
                && let Some(PatchState::Direct(chain)) = self.patches.get(&key)
            {
                patched.push((pk, chain.clone()));
                continue;
            }
            recompute.push(key);
        }
        let mut applied: HashSet<i64> = patched.iter().map(|(pk, _)| *pk).collect();
        if !patched.is_empty() {
            // A row not materialised yet cannot be patched: it is recomputed.
            let meta = self.meta(entity)?;
            for pk in crate::refresh::direct::apply_entity_patches(&meta, patched)? {
                applied.remove(&pk);
                recompute.push(RefreshKey::pk(entity, pk));
            }
        }
        if !recompute.is_empty() {
            self.recompute(entity, recompute)?;
        }
        if !applied.is_empty() {
            self.derive_parent_patches(entity, &applied.into_iter().collect::<Vec<_>>())?;
        }
        Ok(())
    }

    /// A write to a table no cascade maps (`full_refresh` policy): bring the whole TVIEW to its view once, which covers every other key
    /// of the entity, and queue the parents of the rows that changed.
    fn refresh_all(&mut self, entity: &str) -> TViewResult<()> {
        let meta = self.meta(entity)?;
        // Parents are found by integer keys: a text key has none to find.
        let changed: Vec<i64> = crate::ddl::replace::reconcile(entity, &meta)?
            .into_iter()
            .filter_map(|k| KeyValue::Text(k).to_int())
            .collect();
        // A full refresh may bring rows back: look their parents up in the
        // parents' views too.
        self.queue_parents_to_recompute(entity, &changed, &changed)?;
        if meta.identity.is_pk(entity) {
            self.processed
                .extend(changed.into_iter().map(|pk| RefreshKey::pk(entity, pk)));
        }
        Ok(())
    }

    /// Recompute `keys` from the view, one row with a smart patch or several in
    /// bulk, and queue the parents of the rows touched to recompute too: their
    /// child's whole document changed.
    fn recompute(&mut self, entity: &str, keys: Vec<RefreshKey>) -> TViewResult<()> {
        let keys: Vec<KeyValue> = keys.into_iter().map(|k| k.key).collect();
        let touched = if let [key] = keys.as_slice() {
            crate::refresh::refresh_key(&self.meta(entity)?, key)?
        } else {
            crate::refresh::refresh_bulk(entity, &keys)?
        };
        self.queue_parents_to_recompute(entity, &touched.pks, &touched.appeared)
    }

    /// Queue the parents of `entity`'s rows `pks` (their `pk_<entity>`, ADR 0169),
    /// each to recompute; parents of the rows that `appeared` are looked up in
    /// their views too.
    fn queue_parents_to_recompute(
        &mut self,
        entity: &str,
        pks: &[i64],
        appeared: &[i64],
    ) -> TViewResult<()> {
        for parent in crate::propagate::find_parents_batch(entity, pks, appeared, &self.graph)?
            .into_values()
            .flatten()
        {
            patch::poison_into(&mut self.patches, parent.clone());
            self.queue(parent);
        }
        Ok(())
    }

    /// For the parents of the rows a direct patch `applied`: the child's chain
    /// under the path a parent embeds its document at, or a recompute where no
    /// patch can be derived (an array or scalar embed, a gated parent). A parent
    /// reached by a patched and by a recomputed child recomputes: poison sticks.
    fn derive_parent_patches(&mut self, entity: &str, applied: &[i64]) -> TViewResult<()> {
        let parents = crate::propagate::find_parents_batch(entity, applied, &[], &self.graph)?;
        for (child_pk, parent_keys) in parents {
            let child = RefreshKey::pk(entity, child_pk);
            let chain = match self.patches.get(&child) {
                Some(PatchState::Direct(chain)) => Some(chain.clone()),
                _ => None,
            };
            for parent in parent_keys {
                let derived = match &chain {
                    Some(chain) => self.cached_meta(&parent.entity)?.and_then(|m| {
                        crate::refresh::direct::derive_parent_chain(&m, entity, chain)
                    }),
                    None => None,
                };
                match derived {
                    Some(chain) => {
                        patch::merge_chain_into(&mut self.patches, parent.clone(), chain);
                    }
                    None => patch::poison_into(&mut self.patches, parent.clone()),
                }
                self.queue(parent);
            }
        }
        Ok(())
    }

    /// Apply fan-out patches: one UPDATE per child entity and lookup
    /// column, writing each parent's changed fields into all its children. Every
    /// child row that changed is journaled, and its own parents are queued (to
    /// recompute: they embed the child's document).
    pub(super) fn apply_fanouts(&mut self, fanouts: FanoutMap) -> TViewResult<()> {
        let mut groups: BTreeMap<(String, String), Vec<_>> = BTreeMap::new();
        for ((entity, lookup_col, key), fields) in fanouts {
            groups
                .entry((entity, lookup_col))
                .or_default()
                .push((key, fields));
        }
        for ((entity, lookup_col), rows) in groups {
            let meta = self.meta(&entity)?;
            let owner = crate::owner::AsOwner::of_table(meta.tview_oid)?;
            let changed = crate::refresh::direct::apply_fanout_patch(&meta, &lookup_col, &rows)?;
            drop(owner);
            self.queue_parents_to_recompute(&entity, &changed, &[])?;
        }
        Ok(())
    }

    /// Queue `key` unless this flush refreshed it already.
    fn queue(&mut self, key: RefreshKey) {
        if !self.processed.contains(&key) {
            self.pending.insert(key);
        }
    }

    /// `entity`'s catalog row.
    fn meta(&mut self, entity: &str) -> TViewResult<TviewMeta> {
        self.cached_meta(entity)?
            .ok_or_else(|| crate::TViewError::MetadataNotFound {
                entity: entity.to_string(),
            })
    }

    /// `entity`'s catalog row, read once per flush (`None`: not a TVIEW).
    fn cached_meta(&mut self, entity: &str) -> TViewResult<Option<TviewMeta>> {
        if let Some(meta) = self.metas.get(entity) {
            return Ok(meta.clone());
        }
        let meta = TviewMeta::load_by_entity(entity)?;
        self.metas.insert(entity.to_string(), meta.clone());
        Ok(meta)
    }
}
