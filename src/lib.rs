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
