

//! Dependency Analysis: Base Table Discovery and Trigger Management
//!
//! This module analyzes `PostgreSQL`'s system catalogs to understand view dependencies:
//! - **Base Table Discovery**: Finds all tables a view depends on
//! - **Dependency Graph**: Builds hierarchical relationship maps
//! - **Trigger Management**: Installs/removes change-tracking triggers
//!
//! ## Architecture
//!
//! Dependency analysis uses `PostgreSQL`'s `pg_depend` and `pg_rewrite` catalogs:
//! 1. Start from a view's OID
//! 2. Follow dependency chains through `pg_depend`
//! 3. Identify base tables (non-view objects)
//! 4. Build trigger installation plan
//!
//! ## Key Functions
//!
//! - `find_base_tables()`: Core dependency resolution
//! - `install_triggers()`: Set up change tracking
//! - `DependencyGraph`: Caches analysis results

pub mod graph;
pub mod triggers;

pub use graph::{find_base_tables, find_helper_views, DependencyGraph};
pub use triggers::{install_triggers, remove_triggers};
