//! Configuration: Compile-time and Runtime Settings
//!
//! This module centralizes all configuration for `pg_tviews`:
//! - **Compile-time constants**: Fixed limits and defaults
//! - **Runtime settings**: GUC-based configuration (PostgreSQL Grand Unified Configuration)
//! - **Feature flags**: Enable/disable optional functionality
//!
//! ## Configuration System
//!
//! Settings are now configurable at runtime via PostgreSQL GUC variables:
//! - `pg_tviews.max_propagation_depth`: Maximum cascade refresh iterations (default: 100)
//! - `pg_tviews.graph_cache_enabled`: Enable dependency graph caching (default: on)
//! - `pg_tviews.table_cache_enabled`: Enable table OID caching (default: on)
//! - `pg_tviews.metrics_enabled`: Enable performance metrics collection (default: off)
//!
//! ## Key Settings
//!
//! - `MAX_DEPENDENCY_DEPTH`: Prevents infinite recursion in view hierarchies (compile-time)
//! - `max_propagation_depth()`: Limits cascade refresh iterations (GUC-configurable)
//! - Cache enable/disable flags for performance tuning (GUC-configurable)
//! - Debug and metrics collection controls (GUC-configurable)

use pgrx::{GucContext, GucFlags, GucRegistry, GucSetting};

// GUC variables - must be static for PostgreSQL to reference them
static MAX_PROPAGATION_DEPTH: GucSetting<i32> = GucSetting::<i32>::new(100);
static GRAPH_CACHE_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
static METRICS_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(false);

/// Maximum depth for `pg_depend` traversal
/// Prevents infinite recursion and overly complex view hierarchies
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Enable verbose dependency logging (for debugging)
pub const DEBUG_DEPENDENCIES: bool = false;

/// Register all pg_tviews GUC variables
///
/// Must be called from `_PG_init()` during extension loading.
pub fn register_gucs() {
    GucRegistry::define_int_guc(
        c"pg_tviews.max_propagation_depth",
        c"Maximum iterations for cascade refresh propagation",
        c"Prevents infinite loops in complex dependency chains. Default: 100",
        &MAX_PROPAGATION_DEPTH,
        1,      // min
        10000,  // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.graph_cache_enabled",
        c"Enable caching of entity dependency graph",
        c"Improves performance by avoiding repeated pg_tview_meta queries. Default: true",
        &GRAPH_CACHE_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.table_cache_enabled",
        c"Enable caching of table OID to entity name mapping",
        c"Improves trigger performance by caching table lookups. Default: true",
        &TABLE_CACHE_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"pg_tviews.metrics_enabled",
        c"Enable collection of performance metrics",
        c"Tracks cache hits, refresh counts, and timing. Default: false",
        &METRICS_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );
}

/// Maximum propagation iteration depth
///
/// Configurable via: `SET pg_tviews.max_propagation_depth = N;`
pub fn max_propagation_depth() -> usize {
    MAX_PROPAGATION_DEPTH.get() as usize
}

/// Check if graph caching is enabled
///
/// Configurable via: `SET pg_tviews.graph_cache_enabled = on/off;`
pub fn graph_cache_enabled() -> bool {
    GRAPH_CACHE_ENABLED.get()
}

/// Check if table caching is enabled
///
/// Configurable via: `SET pg_tviews.table_cache_enabled = on/off;`
pub fn table_cache_enabled() -> bool {
    TABLE_CACHE_ENABLED.get()
}

/// Get the current log level (still compile-time for now)
///
/// Log level configuration requires more complex enum handling.
pub const fn log_level() -> &'static str {
    "info"
}

/// Check if metrics collection is enabled
///
/// Configurable via: `SET pg_tviews.metrics_enabled = on/off;`
pub fn metrics_enabled() -> bool {
    METRICS_ENABLED.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_propagation_depth_default() {
        // Test that the default value is returned
        // Note: In a test environment, we can't actually change GUC values,
        // but we can verify the function doesn't panic and returns expected defaults
        let depth = max_propagation_depth();
        assert!(depth > 0, "Max propagation depth should be positive");
        // Default should be 100
        assert_eq!(depth, 100, "Default max propagation depth should be 100");
    }

    #[test]
    fn test_cache_enabled_defaults() {
        // Test default values for cache settings
        assert_eq!(graph_cache_enabled(), true, "Graph cache should be enabled by default");
        assert_eq!(table_cache_enabled(), true, "Table cache should be enabled by default");
        assert_eq!(metrics_enabled(), false, "Metrics should be disabled by default");
    }

    #[test]
    fn test_log_level_constant() {
        // Log level should remain a compile-time constant
        assert_eq!(log_level(), "info", "Log level should be 'info'");
    }

    #[test]
    fn test_compile_time_constants() {
        // These should remain compile-time constants
        assert_eq!(MAX_DEPENDENCY_DEPTH, 10, "Max dependency depth should be 10");
        assert_eq!(DEBUG_DEPENDENCIES, false, "Debug dependencies should be false");
    }
}
