/// Maximum depth for pg_depend traversal
/// Prevents infinite recursion and overly complex view hierarchies
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Enable verbose dependency logging (for debugging)
pub const DEBUG_DEPENDENCIES: bool = false;
