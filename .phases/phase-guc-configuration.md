# Phase: GUC Configuration (Issue #27)

## Objective

Replace the 5 hardcoded `const fn` configuration values in `src/config/mod.rs` with
runtime-tunable PostgreSQL GUC (Grand Unified Configuration) parameters using pgrx's
`GucSetting` / `GucRegistry` API.

## Success Criteria

- [ ] All 5 GUCs registered and accessible via `SHOW pg_tviews.*` / `SET pg_tviews.*`
- [ ] Existing callers work unchanged (same function signatures, same defaults)
- [ ] `cargo check` and `cargo clippy -- -D warnings` pass clean
- [ ] Unit tests verify default values

## Scope

### GUCs to implement

| GUC name                            | Type | Default | Used by                                |
|--------------------------------------|------|---------|----------------------------------------|
| `pg_tviews.max_propagation_depth`    | i32  | 100     | `lib.rs:461`, `queue/xact.rs:334`      |
| `pg_tviews.graph_cache_enabled`      | bool | true    | `queue/cache.rs:23`                    |
| `pg_tviews.table_cache_enabled`      | bool | true    | `queue/cache.rs:59`                    |
| `pg_tviews.log_level`               | enum | info    | Currently unused — register for future |
| `pg_tviews.metrics_enabled`         | bool | false   | Currently unused — register for future |

### Unchanged constants

- `MAX_DEPENDENCY_DEPTH` (compile-time `const usize = 10`) — safety limit, stays const
- `DEBUG_DEPENDENCIES` (compile-time `const bool = false`) — stays const

## Files to modify

1. **`src/config/mod.rs`** — Major: define `GucSetting` statics, add `register_gucs()`,
   replace `const fn` with runtime `.get()` accessors
2. **`src/lib.rs`** — Minor: call `config::register_gucs()` in `_PG_init()`

No other files change — all callers use `config::max_propagation_depth()` etc. which
keep the same signatures.

## TDD Cycles

### Cycle 1: Integer GUC — `max_propagation_depth`

**RED**: Write a `#[cfg(test)]` test that calls `max_propagation_depth()` and asserts it
returns 100 (the default). This test already passes trivially with the `const fn`, but
after the refactor it validates the GUC default.

**GREEN**: In `src/config/mod.rs`:
1. Add imports:
   ```rust
   use pgrx::GucSetting;
   use pgrx::GucRegistry;
   use pgrx::GucContext;
   use pgrx::GucFlags;
   ```
2. Define the static:
   ```rust
   static MAX_PROPAGATION_DEPTH_GUC: GucSetting<i32> = GucSetting::<i32>::new(100);
   ```
3. Add `register_gucs()` function:
   ```rust
   pub fn register_gucs() {
       GucRegistry::define_int_guc(
           "pg_tviews.max_propagation_depth",
           "Maximum cascade propagation iterations before aborting.",
           "Prevents infinite loops in circular dependency chains.",
           &MAX_PROPAGATION_DEPTH_GUC,
           1,        // min
           10_000,   // max
           GucContext::Userset,
           GucFlags::default(),
       );
   }
   ```
4. Replace the `const fn`:
   ```rust
   pub fn max_propagation_depth() -> usize {
       MAX_PROPAGATION_DEPTH_GUC.get() as usize
   }
   ```
5. In `src/lib.rs` `_PG_init()`, add `crate::config::register_gucs();` before the hook
   registration (GUCs must be registered before any code that reads them).

**REFACTOR**: Ensure `#[must_use]` is kept on the accessor. Remove the "Future: make
this configurable via a GUC" comment — it's done.

**CLEANUP**: `cargo clippy -- -D warnings`, `cargo check`.

### Cycle 2: Bool GUCs — `graph_cache_enabled` + `table_cache_enabled`

**RED**: Tests assert `graph_cache_enabled() == true` and `table_cache_enabled() == true`.

**GREEN**: Define two `GucSetting<bool>` statics, register via `define_bool_guc`, replace
`const fn` bodies:
```rust
static GRAPH_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
static TABLE_CACHE_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(true);
```

Register both in `register_gucs()`:
```rust
GucRegistry::define_bool_guc(
    "pg_tviews.graph_cache_enabled",
    "Enable in-memory caching of entity dependency graphs.",
    "When false, graphs are loaded from pg_tview_meta on every refresh.",
    &GRAPH_CACHE_ENABLED_GUC,
    GucContext::Userset,
    GucFlags::default(),
);
GucRegistry::define_bool_guc(
    "pg_tviews.table_cache_enabled",
    "Enable in-memory caching of table OID to entity name mappings.",
    "When false, entity lookups query pg_tview_meta on every trigger.",
    &TABLE_CACHE_ENABLED_GUC,
    GucContext::Userset,
    GucFlags::default(),
);
```

**REFACTOR**: Group the registration calls logically.

**CLEANUP**: Lint, format.

### Cycle 3: Bool GUC — `metrics_enabled`

**RED**: Test asserts `metrics_enabled() == false`.

**GREEN**: Same pattern:
```rust
static METRICS_ENABLED_GUC: GucSetting<bool> = GucSetting::<bool>::new(false);
```

Register and replace accessor.

**REFACTOR**: Clean.

**CLEANUP**: Lint, format.

### Cycle 4: Enum/String GUC — `log_level`

**RED**: Test asserts `log_level()` returns `"info"`.

**GREEN**: Use `GucSetting<Option<&'static CStr>>` for the string GUC:
```rust
use std::ffi::CStr;

static LOG_LEVEL_GUC: GucSetting<Option<&'static CStr>> =
    GucSetting::<Option<&'static CStr>>::new(None);  // None = use boot_val
```

Register via `define_string_guc`:
```rust
GucRegistry::define_string_guc(
    "pg_tviews.log_level",
    "Logging verbosity for pg_tviews operations.",
    "Allowed values: debug, info, warning, error.",
    &LOG_LEVEL_GUC,
    GucContext::Userset,
    GucFlags::default(),
);
```

Accessor:
```rust
pub fn log_level() -> &'static str {
    match LOG_LEVEL_GUC.get() {
        Some(cstr) => cstr.to_str().unwrap_or("info"),
        None => "info",
    }
}
```

Note: pgrx string GUCs require `CStr`. The boot value is set during registration.
If this proves tricky with pgrx 0.16.1's API, fall back to keeping `log_level()` as
a `const fn` with a TODO — it has zero callers today and can be deferred.

**REFACTOR**: Clean.

**CLEANUP**: Lint, format, commit.

### Cycle 5: Final verification

- Remove all "Future: make this configurable via a GUC" comments
- Update module-level docstring to reflect runtime GUC support
- `cargo check && cargo clippy -- -D warnings`
- Final commit

## Verification

```bash
cargo check
cargo clippy -- -D warnings

# In a running PostgreSQL instance:
SHOW pg_tviews.max_propagation_depth;   -- 100
SET pg_tviews.max_propagation_depth = 50;
SHOW pg_tviews.max_propagation_depth;   -- 50

SHOW pg_tviews.graph_cache_enabled;     -- on
SET pg_tviews.graph_cache_enabled = off;
```

## Risk Assessment

**Low risk.** All callers already use the function-call API (`config::max_propagation_depth()`),
so swapping from `const fn` to runtime `GucSetting::get()` is transparent. Defaults are
identical. The only subtle point is the string GUC (`log_level`) which uses `CStr` — if
pgrx 0.16.1's API is awkward there, defer it (zero callers).

## Dependencies

- None — self-contained change
- Blocks: nothing

## Status
[x] Cycle 1: Integer GUC — max_propagation_depth (GucSetting<i32>, registered, accessor returns usize)
[x] Cycle 2: Bool GUCs — graph_cache_enabled + table_cache_enabled
[x] Cycle 3: Bool GUC — metrics_enabled
[x] Cycle 4: String GUC — log_level (GucSetting<Option<CString>>, boot_val="info")
[x] Cycle 5: Final verification — all 5 GUCs verified via SHOW/SET on PG18
