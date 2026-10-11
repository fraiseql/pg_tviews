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

- Every callback `PostgreSQL` calls carries `#[pg_guard]`; previous hooks are
  called across `pg_guard_ffi_boundary` (`scripts/check-ffi-guards.sh` checks it).
- A refresh that fails fails the write; refresh work still queued fails the commit.
- `unsafe` is confined to FFI with `PostgreSQL`, each block with its `SAFETY:` reason.
*/

use pgrx::prelude::*;

// Core modules
mod api;
mod audit;
mod cache;
mod catalog;
mod concurrency;
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
mod stats;
mod trigger;
mod utils;

// Feature modules
mod admin;
mod health;
mod lifecycle;
mod suspend;

mod config;
mod ddl;
mod dependency;
mod error;
mod install_sql;
mod jsonb_delta;
mod validation;

use error::{TViewError, TViewResult};

pg_module_magic!();

/// Initialize the extension
/// Installs the `ProcessUtility` hook to intercept CREATE TABLE `tv_*` commands
///
/// Safety: Only installs hooks when running in a proper `PostgreSQL` backend,
/// not during initdb or other bootstrap contexts.
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    crate::config::register_gucs();
    crate::stats::init();
    crate::cache::register_relcache_callback();
    crate::rebuild_worker::register();

    // SAFETY: _PG_init runs in PostgreSQL backend context. Installing hooks and
    // registering callbacks is valid in this context.
    unsafe {
        crate::hooks::ensure_hook_installed();
    }

    // Register transaction callbacks once at startup.
    // PostgreSQL's RegisterXactCallback appends to a persistent linked list,
    // so registering per-transaction would accumulate N copies after N transactions.
    // SAFETY: Transaction callbacks are registered in backend initialization context.
    unsafe {
        crate::flush::register_xact_callback();
        crate::flush::register_subxact_callback();
    }
}
