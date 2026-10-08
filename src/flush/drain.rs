//! The flush loop: one entity per pass, in dependency order, until neither the
//! flush nor the triggers its writes fire left any work.

use super::apply::Flush;
use crate::TViewResult;
use crate::queue::RefreshKey;
use crate::queue::state;

pub(super) fn flush_pending() -> TViewResult<()> {
    // Take what the triggers queued. The direct patches come with their queue
    // entries (issue #56): keys carrying a usable chain are patched straight into
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
    // Issue #120: write each parent change into all its children at once; the
    // parents of every changed child join the queue.
    flush.apply_fanouts(fanout)?;

    loop {
        while let Some((entity, keys)) = flush.next_entity() {
            flush.apply_entity(&entity, keys)?;
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
        // Work queued by triggers the flush's own writes fired (a TVIEW's table
        // read by another, a user trigger writing a base table), with the patches
        // they captured. Keys already refreshed are not refreshed again.
        let late = state::drain();
        if late.queue.is_empty() && late.fanout.is_empty() {
            break;
        }
        flush.patches.extend(late.patches);
        flush.pending = late.queue;
        flush.apply_fanouts(late.fanout)?;
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
    /// The keys of the first pending entity in dependency order not refreshed in
    /// this flush yet; the other entities' keys stay pending.
    fn next_entity(&mut self) -> Option<(String, Vec<RefreshKey>)> {
        if self.pending.is_empty() {
            return None;
        }
        // The parents a refresh discovers come later in this order, so every key
        // is refreshed after everything it reads, and only once.
        let sorted = self.graph.sort_keys(self.pending.drain().collect());
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
