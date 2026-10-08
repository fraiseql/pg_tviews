/*!
# `pg_tviews` - `PostgreSQL` Transactional Views

A `PostgreSQL` extension that provides transactional materialized views with
incremental refresh capabilities. `TVIEW`s automatically maintain consistency
between base tables and derived views through trigger-based change tracking.

## Architecture

`pg_tviews` implements a sophisticated refresh system:

1. **Change Tracking**: Triggers on base tables enqueue changes to a transaction-scoped queue
2. **Dependency Analysis**: Resolves view dependencies using topological sorting
3. **Incremental Refresh**: Updates only affected rows in dependent views
4. **Transaction Safety**: All refreshes occur within the same transaction as the original changes

## Key Features

- **Transactional Consistency**: View refreshes are atomic with base table changes
- **Dependency Resolution**: Handles complex multi-level view dependencies
- **Performance Optimized**: Incremental updates avoid full view rebuilds
- **`PostgreSQL` Native**: Written as a C extension using `pgrx` framework

## Safety

- No panics in FFI callbacks (all wrapped in `catch_unwind`)
- Transaction rollback on refresh failures
- Memory safety through Rust's ownership system
*/

use pgrx::pg_sys::panic::ErrorReport;
use pgrx::prelude::*;

// Core modules
mod audit;
mod cache;
mod catalog;
mod delta;
mod executor;
mod flush;
mod hooks;
mod internal_ddl;
mod lineage;
mod metrics;
mod owner;
mod propagate;
mod queue;
mod rebuild_worker;
mod refresh;
mod replication;
mod report;
mod revision;
mod trigger;
mod utils;

// Feature modules
mod admin;
mod health;
mod lifecycle;
mod suspend;

// Public API modules
mod config;
mod ddl;
mod dependency;
mod error;
mod install_sql;
mod jsonb_delta;
mod validation;

// Public re-exports
use error::{TViewError, TViewResult};

pg_module_magic!();

#[pg_extern]
#[must_use]
pub fn pg_tviews_is_suspended() -> bool {
    crate::suspend::is_suspended()
}

#[pg_extern]
pub fn pg_tviews_suspend_triggers() {
    crate::suspend::suspend();
}

/// Resume trigger-based refresh. When the outermost suspension ends, every TVIEW
/// changed while suspended (and every TVIEW embedding one of them) is rebuilt.
#[pg_extern]
pub fn pg_tviews_resume_triggers() -> Result<(), ErrorReport> {
    crate::revision::check();
    crate::suspend::resume()?;
    if !crate::suspend::is_suspended() {
        crate::suspend::catch_up()?;
    }
    Ok(())
}

/// Rebuild every TVIEW, dependencies first, and report how many were rebuilt,
/// in which order, and how long it took.
#[pg_extern]
pub fn pg_tviews_refresh_all() -> Result<pgrx::datum::JsonB, ErrorReport> {
    crate::revision::check();
    if crate::suspend::is_suspended() {
        return Err(TViewError::WrongState {
            reason: "Cannot refresh: triggers are suspended".to_string(),
        }
        .into());
    }

    let start = std::time::Instant::now();
    let order = crate::admin::refresh_all_in_dependency_order()?;

    Ok(pgrx::datum::JsonB(serde_json::json!({
        "refreshed_count": order.len(),
        "order": order,
        "duration_ms": start.elapsed().as_millis(),
    })))
}

#[pg_extern]
#[must_use]
pub fn pg_tviews_suspended_entities() -> Vec<String> {
    crate::suspend::get_changed_entities()
}
