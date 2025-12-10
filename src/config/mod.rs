/// Maximum depth for pg_depend traversal
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
