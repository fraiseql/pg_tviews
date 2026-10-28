//! The flush loop: one entity per pass, in dependency order, until neither the
//! flush nor the triggers its writes fire left any work.

use super::apply::Flush;
use crate::TViewResult;
use crate::queue::RefreshKey;
use crate::queue::patch::{self, PatchState};
use crate::queue::state;

pub(super) fn flush_pending() -> TViewResult<()> {
    // Take what the triggers queued. The direct patches come with their queue
    // entries: keys carrying a usable chain are patched straight into
    // tv_<entity>; everything else recomputes.
    let state::Pending {
        queue,
        patches,
        fanout,
    } = state::drain();
    if queue.is_empty() && fanout.is_empty() {
        return Ok(());
    }
    crate::revision::check();
    crate::queue::affected::begin_flush();
    let timer = crate::metrics::metrics_api::record_refresh_start();

    let mut flush = Flush::new(crate::cache::graph()?, queue, patches);
    // Write each parent change into all its children at once; the
    // parents of every changed child join the queue.
    flush.apply_fanouts(fanout)?;

    while let Some((entity, keys)) = flush.next_entity() {
        let started = std::time::Instant::now();
        flush.apply_entity(&entity, keys)?;
        crate::stats::add(
            &entity,
            crate::stats::Counter::RefreshMicros,
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        );
        flush.take_late()?;
        flush.iteration += 1;
        let max_depth = crate::config::max_propagation_depth();
        if flush.iteration > max_depth {
            return Err(crate::TViewError::DepthExceeded {
                what: "propagation",
                depth: flush.iteration,
                max_depth,
            });
        }
    }

    // One audit entry per entity, written by the COMMIT hook.
    let mut counts: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
    for key in &flush.processed {
        *counts.entry(&key.entity).or_insert(0) += 1;
    }
    for (entity, count) in counts {
        crate::audit::log_refresh(entity, count);
    }
    crate::metrics::metrics_api::record_refresh_complete(
        flush.processed.len(),
        flush.iteration - 1,
        &timer,
    );
    Ok(())
}

impl Flush {
    /// Take the work queued by triggers the last pass's writes fired (a TVIEW's
    /// table read by another, a user trigger writing a base table), with the
    /// patches they captured, into this flush's pending keys: each entity is then
    /// refreshed once, in dependency order, its rows locked in key order, as two
    /// concurrent flushes must (#263). A key refreshed earlier in this flush is
    /// refreshed again: its source changed after that refresh read it. The
    /// propagation depth bounds a trigger that keeps writing.
    fn take_late(&mut self) -> TViewResult<()> {
        let late = state::drain();
        if late.queue.is_empty() && late.fanout.is_empty() {
            return Ok(());
        }
        let mut late_patches = late.patches;
        for key in late.queue {
            // A key refreshed earlier in this flush starts over with what the late
            // write captured; one still pending keeps what it had and adds that:
            // a patch merged into its chain, or a recompute when the late write
            // captured none.
            let refreshed = self.processed.remove(&key);
            let state = late_patches.remove(&key);
            if refreshed || !self.pending.contains(&key) {
                match state {
                    Some(state) => self.patches.insert(key.clone(), state),
                    None => self.patches.remove(&key),
                };
            } else {
                match state {
                    Some(PatchState::Direct(chain)) => {
                        patch::merge_chain_into(&mut self.patches, key.clone(), chain);
                    }
                    Some(PatchState::Poisoned) | None => {
                        patch::poison_into(&mut self.patches, key.clone());
                    }
                }
            }
            self.pending.insert(key);
        }
        self.apply_fanouts(late.fanout)
    }

    /// The keys of the first pending entity in dependency order not refreshed in
    /// this flush yet; the other entities' keys stay pending.
    fn next_entity(&mut self) -> Option<(String, Vec<RefreshKey>)> {
        if self.pending.is_empty() {
            return None;
        }
        // The parents a refresh discovers come later in this order, so every key
        // is refreshed after everything it reads, and only once.
        let sorted = self
            .graph
            .in_order(self.pending.drain().collect(), |k| &k.entity);
        let entity = sorted[0].entity.clone();
        let mut keys = Vec::new();
        for key in sorted {
            if key.entity != entity {
                self.pending.insert(key);
            } else if self.processed.insert(key.clone()) {
                keys.push(key);
            }
        }
        Some((entity, keys))
    }
}
