//! Configuration: Compile-time and Runtime Settings
//!
//! This module centralizes all configuration for `pg_tviews`:
//! - **Compile-time constants**: Fixed limits and defaults
//! - **Runtime settings**: Planned GUC-based configuration
//! - **Feature flags**: Enable/disable optional functionality
//!
//! ## Current Configuration
//!
//! All settings are currently compile-time constants. Future versions
//! will support `PostgreSQL` GUC (Grand Unified Configuration) variables
//! for runtime configuration without recompilation.
//!
//! ## Key Settings
//!
//! - `MAX_DEPENDENCY_DEPTH`: Prevents infinite recursion in view hierarchies
//! - `max_propagation_depth()`: Limits cascade refresh iterations
//! - Cache enable/disable flags for performance tuning
//! - Debug and metrics collection controls

/// Maximum depth for `pg_depend` traversal
/// Prevents infinite recursion and overly complex view hierarchies
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Enable verbose dependency logging (for debugging)
pub const DEBUG_DEPENDENCIES: bool = false;

/// Maximum propagation iteration depth (default: 100)
/// Prevents infinite loops in dependency chains
/// TODO: Make this configurable via GUC in future version
pub fn max_propagation_depth() -> usize {
    100
}

/// Check if graph caching is enabled
/// TODO: Make this configurable via GUC in future version
pub fn graph_cache_enabled() -> bool {
    true
}

/// Check if table caching is enabled
/// TODO: Make this configurable via GUC in future version
pub fn table_cache_enabled() -> bool {
    true
}

/// Get the current log level
/// TODO: Make this configurable via GUC in future version
pub fn log_level() -> &'static str {
    "info"
}

/// Check if metrics collection is enabled
/// TODO: Make this configurable via GUC in future version
pub fn metrics_enabled() -> bool {
    false
}
