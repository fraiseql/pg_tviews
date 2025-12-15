# Phase 2: Implement GUC Configuration System

## Objective

Replace compile-time constants in `src/config/mod.rs` with PostgreSQL GUC (Grand Unified Configuration) variables, enabling runtime configuration without recompilation.

## Context

Currently, all configuration in `src/config/mod.rs` uses `const fn` returning hardcoded values:

```rust
pub const fn max_propagation_depth() -> usize { 100 }
pub const fn graph_cache_enabled() -> bool { true }
pub const fn table_cache_enabled() -> bool { true }
pub const fn log_level() -> &'static str { "info" }
pub const fn metrics_enabled() -> bool { false }
```

This requires recompilation to change any setting. PostgreSQL's GUC system allows runtime configuration via:
- `SET pg_tviews.max_propagation_depth = 50;`
- `postgresql.conf` entries
- `ALTER SYSTEM` commands

## Files to Modify

| File | Changes |
|------|---------|
| `src/config/mod.rs` | Add GUC variable definitions, replace const fns with GUC readers |
| `src/lib.rs` | Register GUCs in `_PG_init()` |

## Implementation Steps

### Step 1: Add GUC imports and static variables

In `src/config/mod.rs`:

```rust
use pgrx::prelude::*;
use pgrx::{GucContext, GucFlags, GucRegistry, GucSetting};
use std::ffi::CStr;

// GUC variables - must be static for PostgreSQL to reference them
static MAX_PROPAGATION_DEPTH: GucSetting<i32> = GucSetting::<i32>::new(100);
static GRAPH_CACHE_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
static METRICS_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(false);
```

### Step 2: Create GUC registration function

```rust
/// Register all pg_tviews GUC variables
///
/// Must be called from `_PG_init()` during extension loading.
pub fn register_gucs() {
    GucRegistry::define_int_guc(
        "pg_tviews.max_propagation_depth",
        "Maximum iterations for cascade refresh propagation",
        "Prevents infinite loops in complex dependency chains. Default: 100",
        &MAX_PROPAGATION_DEPTH,
        1,      // min
        10000,  // max
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        "pg_tviews.graph_cache_enabled",
        "Enable caching of entity dependency graph",
        "Improves performance by avoiding repeated pg_tview_meta queries. Default: true",
        &GRAPH_CACHE_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        "pg_tviews.table_cache_enabled",
        "Enable caching of table OID to entity name mapping",
        "Improves trigger performance by caching table lookups. Default: true",
        &TABLE_CACHE_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        "pg_tviews.metrics_enabled",
        "Enable collection of performance metrics",
        "Tracks cache hits, refresh counts, and timing. Default: false",
        &METRICS_ENABLED,
        GucContext::Userset,
        GucFlags::default(),
    );
}
```

### Step 3: Update accessor functions

Replace `const fn` with runtime GUC readers:

```rust
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

/// Check if metrics collection is enabled
///
/// Configurable via: `SET pg_tviews.metrics_enabled = on/off;`
pub fn metrics_enabled() -> bool {
    METRICS_ENABLED.get()
}

/// Get the current log level (still compile-time for now)
///
/// Log level configuration requires more complex enum handling.
pub const fn log_level() -> &'static str {
    "info"
}
```

### Step 4: Register GUCs in _PG_init

In `src/lib.rs`, find `_PG_init()` and add:

```rust
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    // Register GUC configuration variables
    crate::config::register_gucs();

    // ... rest of initialization ...
}
```

### Step 5: Keep compile-time constants for places that need them

Some code paths may still need compile-time constants (e.g., const generics). Keep `MAX_DEPENDENCY_DEPTH` as a const:

```rust
/// Maximum depth for pg_depend traversal (compile-time constant)
///
/// This is a hard limit to prevent stack overflow during recursive traversal.
/// Not configurable at runtime for safety.
pub const MAX_DEPENDENCY_DEPTH: usize = 10;
```

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Test GUC registration (requires pgrx test environment)
cargo pgrx test pg18
```

## SQL Verification

After implementation, these commands should work:

```sql
-- Check default values
SHOW pg_tviews.max_propagation_depth;  -- Should show 100
SHOW pg_tviews.graph_cache_enabled;    -- Should show on
SHOW pg_tviews.metrics_enabled;        -- Should show off

-- Modify at session level
SET pg_tviews.max_propagation_depth = 50;
SET pg_tviews.graph_cache_enabled = off;
SET pg_tviews.metrics_enabled = on;

-- Verify changes took effect
SHOW pg_tviews.max_propagation_depth;  -- Should show 50

-- Reset to default
RESET pg_tviews.max_propagation_depth;
```

## Acceptance Criteria

- [ ] All GUC variables registered successfully
- [ ] `SHOW pg_tviews.*` commands work
- [ ] `SET pg_tviews.*` commands modify behavior
- [ ] Default values match previous constants
- [ ] Code compiles without warnings
- [ ] Clippy passes
- [ ] Existing tests still pass

## DO NOT

- Do not remove `MAX_DEPENDENCY_DEPTH` const (needed for compile-time checks)
- Do not make GUCs `PGC_SUSET` or higher without good reason (breaks usability)
- Do not add GUCs for log_level yet (requires enum GUC which is more complex)
- Do not change the semantics of any configuration value
