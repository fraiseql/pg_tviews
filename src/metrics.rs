use crate::queue::key::RefreshKey;

// Metrics tracking for TVIEW operations
// Thread-local storage to avoid contention between transactions
thread_local! {
    static METRICS: std::cell::RefCell<QueueMetrics> = const { std::cell::RefCell::new(QueueMetrics::new_const()) };
}

/// Structure holding current transaction metrics
#[derive(Debug, Default, Clone)]
struct QueueMetrics {
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
}

impl QueueMetrics {
    const fn new_const() -> Self {
        Self {
            total_refreshes: 0,
            total_iterations: 0,
            max_iterations: 0,
            total_timing_ns: 0,
            graph_cache_hits: 0,
            graph_cache_misses: 0,
            table_cache_hits: 0,
            table_cache_misses: 0,
        }
    }
}

/// Public interface for metrics tracking
pub mod metrics_api {
    use super::*;

    /// Record the start of a refresh operation
    pub fn record_refresh_start() -> RefreshTimer {
        RefreshTimer::new()
    }

    /// Record completion of refresh operations
    pub fn record_refresh_complete(
        refresh_count: usize,
        iteration_count: usize,
        timer: RefreshTimer,
    ) {
        METRICS.with(|m| {
            let mut metrics = m.borrow_mut();
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

    /// Get current queue statistics
    pub fn get_queue_stats() -> QueueStats {
        // Get current queue size from state
        let queue_size = crate::queue::get_queue_size();

        METRICS.with(|m| {
            let metrics = m.borrow();
            QueueStats {
                queue_size,
                total_refreshes: metrics.total_refreshes,
                total_iterations: metrics.total_iterations,
                max_iterations: metrics.max_iterations,
                total_timing_ns: metrics.total_timing_ns,
                graph_cache_hits: metrics.graph_cache_hits,
                graph_cache_misses: metrics.graph_cache_misses,
                table_cache_hits: metrics.table_cache_hits,
                table_cache_misses: metrics.table_cache_misses,
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
    pub total_refreshes: u64,
    pub total_iterations: u64,
    pub max_iterations: usize,
    pub total_timing_ns: u128,
    pub graph_cache_hits: u64,
    pub graph_cache_misses: u64,
    pub table_cache_hits: u64,
    pub table_cache_misses: u64,
}

impl QueueStats {
    /// Convert timing to milliseconds
    pub fn total_timing_ms(&self) -> f64 {
        self.total_timing_ns as f64 / 1_000_000.0
    }

    /// Calculate cache hit rates
    pub fn graph_cache_hit_rate(&self) -> f64 {
        let total = self.graph_cache_hits + self.graph_cache_misses;
        if total == 0 {
            0.0
        } else {
            self.graph_cache_hits as f64 / total as f64
        }
    }

    pub fn table_cache_hit_rate(&self) -> f64 {
        let total = self.table_cache_hits + self.table_cache_misses;
        if total == 0 {
            0.0
        } else {
            self.table_cache_hits as f64 / total as f64
        }
    }
}