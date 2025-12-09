use pgrx::prelude::*;
use std::collections::HashSet;
use crate::error::{TViewError, TViewResult};
use crate::config::MAX_DEPENDENCY_DEPTH;

pub mod graph;
pub mod triggers;

pub use graph::{find_base_tables, find_helper_views, DependencyGraph};
pub use triggers::{install_triggers, remove_triggers};
