//! Metrics Collection: Performance Monitoring and Statistics
//!
//! This module tracks performance metrics for TVIEW operations, read with
//! `pg_tviews_queue_stats()`:
//! - **Refresh Statistics**: count and timing of refreshes
//! - **Cache Performance**: hit rates of the graph and table caches
//! - **Propagation Metrics**: iterations per flush
//! - **Direct patches**: captured, applied, fallen back
//!
//! ## Architecture
//!
//! Metrics live in thread-local storage, so collecting them costs a counter
//! increment and needs no synchronization. They are always collected:
//! - the refresh and cache counters belong to the transaction and reset when it ends;
//! - the direct-patch counters are cumulative for the session.

use crate::queue::key::RefreshKey;

// Metrics tracking for TVIEW operations
// Thread-local storage to avoid contention between transactions
thread_local! {
    static METRICS: std::cell::RefCell<QueueMetrics> = const { std::cell::RefCell::new(QueueMetrics::new_const()) };

    /// Session-cumulative direct-patch counters.
    ///
    /// Unlike `METRICS`, these are **not** reset at transaction boundaries, so a
    /// counter set by a trigger during an auto-commit statement is still readable
    /// via `pg_tviews_queue_stats()` in a following statement. Tests assert on the
    /// delta across a mutation.
    static DIRECT_PATCH_METRICS: std::cell::RefCell<DirectPatchMetrics> =
        const { std::cell::RefCell::new(DirectPatchMetrics::new_const()) };
}

/// Session-cumulative counters for the direct-patch fast path.
#[derive(Debug, Default, Clone, Copy)]
struct DirectPatchMetrics {
    /// Eligible UPDATEs whose patch was captured by the row trigger.
    captured: u64,
    /// Tview rows updated directly by a patch (no backing-view query).
    applied: u64,
    /// Patched pks that fell back to recompute (row not yet materialised).
    fallbacks: u64,
    /// Tview rows recomputed from the backing view (the non-fast path).
    view_recomputes: u64,
    /// Refresh writes skipped because the row already held the result.
    noop_skipped: u64,
    /// Catalog queries the refresh path made on cache misses.
    catalog_lookups: u64,
    /// Parent lookups skipped because the child's row did not change.
    propagation_pruned: u64,
}

impl DirectPatchMetrics {
    const fn new_const() -> Self {
        Self {
            captured: 0,
            applied: 0,
            fallbacks: 0,
            view_recomputes: 0,
            noop_skipped: 0,
            catalog_lookups: 0,
            propagation_pruned: 0,
        }
    }
}

/// Structure holding current transaction metrics
#[derive(Debug, Default, Clone)]
struct QueueMetrics {
    /// Flushes that refreshed something in the current transaction
    flushes: u64,
    /// Total number of refreshes processed in current transaction
    total_refreshes: u64,
    /// Total propagation iterations in current transaction
    total_iterations: u64,
    /// Maximum iterations seen in any single propagation chain
    max_iterations: usize,
    /// Total timing for refresh operations (nanoseconds)
    total_timing_ns: u128,
    /// Graph cache hits
    graph_cache_hits: u64,
    /// Graph cache misses
    graph_cache_misses: u64,
    /// Table cache hits
    table_cache_hits: u64,
    /// Table cache misses
    table_cache_misses: u64,
    /// Value and key locks taken (ADR 0207)
    value_locks: u64,
    /// Relations locked in place of their values
    value_lock_escalations: u64,
    /// Locks that were not granted at once and were waited for
    value_lock_waits: u64,
    /// Time spent waiting for them (nanoseconds)
    value_lock_wait_ns: u128,
}

impl QueueMetrics {
    const fn new_const() -> Self {
        Self {
            flushes: 0,
            total_refreshes: 0,
            total_iterations: 0,
            max_iterations: 0,
            total_timing_ns: 0,
            graph_cache_hits: 0,
            graph_cache_misses: 0,
            table_cache_hits: 0,
            table_cache_misses: 0,
            value_locks: 0,
            value_lock_escalations: 0,
            value_lock_waits: 0,
            value_lock_wait_ns: 0,
        }
    }
}

/// Public interface for metrics tracking
pub mod metrics_api {
    #[allow(clippy::wildcard_imports)] // Reason: module-internal prelude import
    use super::*;

    /// Record the start of a refresh operation
    pub fn record_refresh_start() -> RefreshTimer {
        RefreshTimer::new()
    }

    /// Record completion of refresh operations
    pub fn record_refresh_complete(
        refresh_count: usize,
        iteration_count: usize,
        timer: &RefreshTimer,
    ) {
        METRICS.with(|m| {
            let mut metrics = m.borrow_mut();
            metrics.flushes += 1;
            metrics.total_refreshes += refresh_count as u64;
            metrics.total_iterations += iteration_count as u64;
            metrics.max_iterations = metrics.max_iterations.max(iteration_count);
            metrics.total_timing_ns += timer.elapsed_ns();
        });
    }

    /// Record graph cache hit
    pub fn record_graph_cache_hit() {
        METRICS.with(|m| {
            m.borrow_mut().graph_cache_hits += 1;
        });
    }

    /// Record graph cache miss
    pub fn record_graph_cache_miss() {
        METRICS.with(|m| {
            m.borrow_mut().graph_cache_misses += 1;
        });
    }

    /// Record table cache hit
    pub fn record_table_cache_hit() {
        METRICS.with(|m| {
            m.borrow_mut().table_cache_hits += 1;
        });
    }

    /// Record table cache miss
    pub fn record_table_cache_miss() {
        METRICS.with(|m| {
            m.borrow_mut().table_cache_misses += 1;
        });
    }

    /// Record an eligible direct-patch capture of `entity`'s TVIEW.
    /// Session-cumulative, and counted in `tviews.stats`.
    pub fn record_direct_patch_captured(entity: &str) {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().captured += 1;
        });
        crate::stats::add(entity, crate::stats::Counter::PatchCaptured, 1);
    }

    /// Record `n` rows of `entity`'s TVIEW updated directly by a patch.
    pub fn record_direct_patches_applied(entity: &str, n: u64) {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().applied += n;
        });
        crate::stats::add(entity, crate::stats::Counter::PatchApplied, n);
    }

    /// Record `n` patched keys of `entity`'s TVIEW that fell back to recompute.
    pub fn record_direct_patch_fallbacks(entity: &str, n: u64) {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().fallbacks += n;
        });
        crate::stats::add(entity, crate::stats::Counter::PatchFallbacks, n);
    }

    /// Record `n` rows of `entity`'s TVIEW recomputed from the backing view.
    pub fn record_view_recomputes(entity: &str, n: u64) {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().view_recomputes += n;
        });
        crate::stats::add(entity, crate::stats::Counter::ViewRecomputes, n);
    }

    /// Record `n` refresh writes to `entity`'s TVIEW skipped because nothing
    /// changed.
    pub fn record_noop_skipped(entity: &str, n: u64) {
        if n > 0 {
            DIRECT_PATCH_METRICS.with(|m| {
                m.borrow_mut().noop_skipped += n;
            });
            crate::stats::add(entity, crate::stats::Counter::NoopSkipped, n);
        }
    }

    /// Record one catalog query made by the refresh path on a cache miss.
    pub fn record_catalog_lookup() {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().catalog_lookups += 1;
        });
    }

    /// Record one propagation edge into `parent`'s TVIEW skipped at an unchanged
    /// child row.
    pub fn record_propagation_pruned(parent: &str) {
        DIRECT_PATCH_METRICS.with(|m| {
            m.borrow_mut().propagation_pruned += 1;
        });
        crate::stats::add(parent, crate::stats::Counter::PropagationPruned, 1);
    }

    /// A value or key lock taken (ADR 0207).
    pub fn record_value_lock() {
        METRICS.with(|m| m.borrow_mut().value_locks += 1);
    }

    /// A relation locked in place of its values.
    pub fn record_value_lock_escalation() {
        METRICS.with(|m| m.borrow_mut().value_lock_escalations += 1);
    }

    /// A lock waited for, `ns` long.
    pub fn record_value_lock_wait(ns: u128) {
        METRICS.with(|m| {
            let mut m = m.borrow_mut();
            m.value_lock_waits += 1;
            m.value_lock_wait_ns += ns;
        });
    }

    /// Get current queue statistics
    pub fn get_queue_stats() -> QueueStats {
        // Get current queue size from state
        let queue_size = crate::queue::get_queue_size();

        let dp = DIRECT_PATCH_METRICS.with(|m| *m.borrow());

        METRICS.with(|m| {
            let metrics = m.borrow();
            QueueStats {
                queue_size,
                flushes: metrics.flushes,
                total_refreshes: metrics.total_refreshes,
                total_iterations: metrics.total_iterations,
                max_iterations: metrics.max_iterations,
                total_timing_ns: metrics.total_timing_ns,
                graph_cache_hits: metrics.graph_cache_hits,
                graph_cache_misses: metrics.graph_cache_misses,
                table_cache_hits: metrics.table_cache_hits,
                table_cache_misses: metrics.table_cache_misses,
                value_locks: metrics.value_locks,
                value_lock_escalations: metrics.value_lock_escalations,
                value_lock_waits: metrics.value_lock_waits,
                value_lock_wait_ns: metrics.value_lock_wait_ns,
                direct_patch_captured: dp.captured,
                direct_patches_applied: dp.applied,
                direct_patch_fallbacks: dp.fallbacks,
                view_recomputes: dp.view_recomputes,
                refresh_noop_skipped: dp.noop_skipped,
                catalog_lookups: dp.catalog_lookups,
                propagation_pruned: dp.propagation_pruned,
            }
        })
    }

    /// Get current queue contents for debugging
    pub fn get_queue_contents() -> Vec<RefreshKey> {
        crate::queue::get_queue_contents()
    }

    /// Reset metrics (called after transaction completes)
    pub fn reset_metrics() {
        METRICS.with(|m| {
            *m.borrow_mut() = QueueMetrics::default();
        });
    }
}

/// Timer for measuring refresh operation duration
pub struct RefreshTimer {
    start: std::time::Instant,
}

impl RefreshTimer {
    fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }

    fn elapsed_ns(&self) -> u128 {
        self.start.elapsed().as_nanos()
    }
}

/// Statistics returned by metrics functions
#[derive(Debug, Clone)]
pub struct QueueStats {
    pub queue_size: usize,
    pub flushes: u64,
    pub total_refreshes: u64,
    pub total_iterations: u64,
    pub max_iterations: usize,
    pub total_timing_ns: u128,
    pub graph_cache_hits: u64,
    pub graph_cache_misses: u64,
    pub table_cache_hits: u64,
    pub table_cache_misses: u64,
    /// Value and key locks taken, escalations, and lock waits (ADR 0207).
    pub value_locks: u64,
    pub value_lock_escalations: u64,
    pub value_lock_waits: u64,
    pub value_lock_wait_ns: u128,
    /// Session-cumulative direct-patch counters.
    pub direct_patch_captured: u64,
    pub direct_patches_applied: u64,
    pub direct_patch_fallbacks: u64,
    pub view_recomputes: u64,
    /// Session-cumulative refresh writes skipped as no-ops.
    pub refresh_noop_skipped: u64,
    /// Session-cumulative catalog queries made on cache misses.
    pub catalog_lookups: u64,
    /// Session-cumulative propagation edges skipped at unchanged rows.
    pub propagation_pruned: u64,
}

impl QueueStats {
    /// Convert timing to milliseconds
    #[allow(clippy::cast_precision_loss)] // Reason: a session's flush time stays far below 2^53 ns
    pub fn total_timing_ms(&self) -> f64 {
        self.total_timing_ns as f64 / 1_000_000.0
    }

    /// Time spent waiting for value locks, in milliseconds.
    #[allow(clippy::cast_precision_loss)] // Reason: a transaction's wait time stays far below 2^53 ns
    pub fn value_lock_wait_ms(&self) -> f64 {
        self.value_lock_wait_ns as f64 / 1_000_000.0
    }

    /// Calculate cache hit rates
    #[allow(clippy::cast_precision_loss)] // Reason: a ratio; counters past 2^53 only lose precision
    pub fn graph_cache_hit_rate(&self) -> f64 {
        let total = self.graph_cache_hits + self.graph_cache_misses;
        if total == 0 {
            0.0
        } else {
            self.graph_cache_hits as f64 / total as f64
        }
    }

    /// Calculate the table cache hit rate
    #[allow(clippy::cast_precision_loss)] // Reason: a ratio; counters past 2^53 only lose precision
    pub fn table_cache_hit_rate(&self) -> f64 {
        let total = self.table_cache_hits + self.table_cache_misses;
        if total == 0 {
            0.0
        } else {
            self.table_cache_hits as f64 / total as f64
        }
    }
}
