//! How one entity's pending keys are applied, and their parents found.

use crate::TViewResult;
use crate::catalog::EntityDepGraph;
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
    /// For a parent key patched from its children's patches: each `(child
    /// entity, child pk)` it was found embedding. The patch holds only while the
    /// parent still embeds every one.
    guards: HashMap<RefreshKey, Vec<(String, i64)>>,
    /// Keys refreshed in this flush.
    pub(super) processed: HashSet<RefreshKey>,
    /// Passes of the flush loop so far, starting at 1.
    pub(super) iteration: usize,
    /// The catalog rows read in this flush, by entity (`None`: not a TVIEW).
    metas: HashMap<String, Option<std::rc::Rc<TviewMeta>>>,
    /// The TVIEW rows this flush may write (`pg_tviews.max_refresh_rows`), and
    /// those charged so far: one flush is one statement's refresh (ADR 0233).
    max_rows: Option<u64>,
    charged: u64,
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
            guards: HashMap::new(),
            processed,
            iteration: 1,
            metas: HashMap::new(),
            max_rows: crate::config::max_refresh_rows(),
            charged: 0,
        }
    }

    /// Charge `rows` rows of `meta`'s TVIEW, written `how`, against the
    /// statement's limit, before they are written.
    ///
    /// # Errors
    /// `RefreshLimit` when the statement's refresh would then write more rows than
    /// `pg_tviews.max_refresh_rows`.
    fn charge(&mut self, meta: &TviewMeta, how: &str, rows: u64) -> TViewResult<()> {
        let Some(max) = self.max_rows else {
            return Ok(());
        };
        let count = self.charged + rows;
        if count > max {
            return Err(crate::TViewError::RefreshLimit {
                limit: crate::error::Limit::Rows {
                    tview: crate::utils::qualified_relname_from_oid(meta.tview_oid)?,
                    how: how.to_string(),
                    count,
                    max,
                },
            });
        }
        self.charged = count;
        Ok(())
    }

    /// Bring `entity`'s rows named by `keys` to what its view computes, then queue
    /// the rows of the TVIEWs embedding the rows that changed.
    pub(super) fn apply_entity(&mut self, entity: &str, keys: Vec<RefreshKey>) -> TViewResult<()> {
        // The entity is read and written as the owner of its tv_* table, whoever
        // wrote to the base table.
        let _owner = crate::owner::AsOwner::of_entity(entity)?;
        // An UNLOGGED TVIEW reset by a crash or a promotion is filled first.
        crate::lifecycle::validity::ensure(entity)?;
        if keys.iter().any(RefreshKey::is_all) {
            self.refresh_all(entity)?;
            crate::lifecycle::validity::refreshed_whole(entity);
        } else {
            self.apply_keys(entity, keys)?;
        }
        // One restored without its rows holds the write's rows only: filled.
        crate::lifecycle::validity::refill_if_restored_empty(entity)?;
        Ok(())
    }

    /// Bring `entity`'s rows named by `keys` to what its view computes, as
    /// [`Self::apply_entity`] does when no key names every row.
    fn apply_keys(&mut self, entity: &str, keys: Vec<RefreshKey>) -> TViewResult<()> {
        let meta = self.meta(entity)?;
        self.charge(&meta, "by key", keys.len() as u64)?;
        // Writers of what these rows read meet them on its values (ADR 0207):
        // locked before the rows are locked and computed.
        let values: Vec<KeyValue> = keys.iter().map(|k| k.key.clone()).collect();
        crate::concurrency::reads::lock_read_set(&*self.meta(entity)?, &values)?;

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
        if !patched.is_empty() {
            self.check_patches(entity, &values, &mut patched, &mut recompute)?;
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
        // REPEATABLE READ: the rows are what the latest snapshot computes.
        crate::concurrency::crosscheck::refreshed_rows(&*self.meta(entity)?, &values)?;
        if !applied.is_empty() {
            self.derive_parent_patches(entity, &applied.into_iter().collect::<Vec<_>>())?;
        }
        Ok(())
    }

    /// Before `entity`'s rows are patched: lock every row of the pass in key order
    /// (patches, one statement per shape, and recomputes would lock them in
    /// several statements, in a different order in each transaction), and move to
    /// `recompute` a parent patched for a child it no longer embeds. The parent was
    /// found holding the child under this statement's snapshot; a concurrent
    /// transaction may have re-pointed it since, and committed while this one
    /// waited for its row lock: the row locked is that latest version.
    fn check_patches(
        &mut self,
        entity: &str,
        values: &[KeyValue],
        patched: &mut Vec<(i64, Vec<PatchEntry>)>,
        recompute: &mut Vec<RefreshKey>,
    ) -> TViewResult<()> {
        let meta = self.meta(entity)?;
        let guarded = |pk: i64| self.guards.get(&RefreshKey::pk(entity, pk));
        let mut columns: Vec<String> = Vec::new();
        for (pk, _) in patched.iter() {
            for (child, _) in guarded(*pk).into_iter().flatten() {
                for column in self.graph.lookup_columns(child, entity) {
                    if !columns.contains(column) {
                        columns.push(column.clone());
                    }
                }
            }
        }
        if patched.len() + recompute.len() < 2 && columns.is_empty() {
            return Ok(());
        }
        let locked = crate::refresh::lock_in_key_order(&meta, values, &columns)?;
        let holds = |pk: i64| {
            guarded(pk).into_iter().flatten().all(|(child, child_pk)| {
                let Some(row) = locked.get(&pk) else {
                    // Not materialised: the patch falls back to a recompute.
                    return true;
                };
                self.graph
                    .lookup_columns(child, entity)
                    .iter()
                    .any(|column| {
                        columns
                            .iter()
                            .position(|c| c == column)
                            .and_then(|i| row.get(i).copied().flatten())
                            == Some(*child_pk)
                    })
            })
        };
        let (kept, moved): (Vec<_>, Vec<_>) = patched.drain(..).partition(|(pk, _)| holds(*pk));
        *patched = kept;
        recompute.extend(moved.into_iter().map(|(pk, _)| RefreshKey::pk(entity, pk)));
        Ok(())
    }

    /// A write to a table no cascade maps (`full_refresh` policy): bring the whole TVIEW to its view once, which covers every other key
    /// of the entity, and queue the parents of the rows that changed.
    fn refresh_all(&mut self, entity: &str) -> TViewResult<()> {
        let meta = self.meta(entity)?;
        if self.max_rows.is_some() {
            let rows = crate::utils::spi::count_rows(meta.tview_oid)?;
            self.charge(&meta, "by a full refresh", rows)?;
        }
        // Every row may change: refreshes of any of them wait, and are waited for.
        crate::concurrency::lock_relation(
            meta.tview_oid.to_u32(),
            crate::concurrency::Side::Writer,
        );
        let changed = crate::refresh::full::reconcile(entity, &meta)?;
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
            crate::refresh::refresh_key(&*self.meta(entity)?, key)?
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
                        self.guards
                            .entry(parent.clone())
                            .or_default()
                            .push((entity.to_string(), child_pk));
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
            if self.max_rows.is_some() {
                let keys = rows.iter().map(|(key, _)| *key).collect();
                let children = crate::refresh::direct::count_fanout(&meta, &lookup_col, keys)?;
                self.charge(&meta, &format!("via {lookup_col}"), children)?;
            }
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
    fn meta(&mut self, entity: &str) -> TViewResult<std::rc::Rc<TviewMeta>> {
        self.cached_meta(entity)?
            .ok_or_else(|| crate::TViewError::TviewNotFound {
                name: entity.to_string(),
            })
    }

    /// `entity`'s catalog row, read once per flush (`None`: not a TVIEW).
    fn cached_meta(&mut self, entity: &str) -> TViewResult<Option<std::rc::Rc<TviewMeta>>> {
        if let Some(meta) = self.metas.get(entity) {
            return Ok(meta.clone());
        }
        let meta = TviewMeta::load_by_entity(entity)?;
        self.metas.insert(entity.to_string(), meta.clone());
        Ok(meta)
    }
}
