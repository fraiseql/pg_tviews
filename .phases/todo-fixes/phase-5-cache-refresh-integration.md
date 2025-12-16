# Phase 5: Integrate Cached Plan Refresh with Main Logic

## Objective

Complete the integration between the cached prepared statement refresh path (`src/refresh/cache.rs`) and the main refresh logic, enabling 10x performance improvement for high-frequency refresh operations.

## Context

Currently, `src/refresh/cache.rs:70` has:

```rust
// Process result (similar to main.rs recompute_view_row)
if let Some(row) = result.next() {
    // Extract data and apply patch (delegate to main refresh logic)
    let _data: JsonB = row["data"].value()?
        .ok_or_else(|| ...)?;
    // TODO: Integrate with main refresh logic to apply patches
    info!("TVIEW: Refreshed {}[{}] with cached plan", entity, pk);
}
```

The cached plan path fetches data but doesn't actually apply the JSONB patch to update the TVIEW table.

## Performance Context

| Path | Query Parse | Plan | Execute | Total |
|------|-------------|------|---------|-------|
| Without cache | 0.2ms | 0.3ms | 0.1ms | ~0.6ms |
| With cache | 0ms | 0ms | 0.1ms | ~0.1ms |

**Potential speedup: 6x for query-heavy refreshes**

## Files to Modify

| File | Changes |
|------|---------|
| `src/refresh/cache.rs` | Complete `refresh_pk_with_cached_plan()` implementation |
| `src/refresh/main.rs` | Extract reusable patch application function |

## Implementation Steps

### Step 1: Extract patch application from main.rs

First, identify the core patch logic in `src/refresh/main.rs` and extract it:

```rust
// In src/refresh/main.rs - extract this as a reusable function:

/// Apply JSONB data to TVIEW table row
///
/// This is the core update operation that:
/// 1. Updates the `data` column with new JSONB
/// 2. Optionally updates denormalized fields (fk_*, path, etc.)
///
/// # Arguments
///
/// * `entity` - Entity name (e.g., "user")
/// * `pk` - Primary key value
/// * `new_data` - New JSONB data to store
///
/// # Returns
///
/// Ok(()) on success, error on failure
pub fn apply_tview_data(entity: &str, pk: i64, new_data: &JsonB) -> TViewResult<()> {
    let table_name = format!("tv_{}", entity);
    let pk_column = format!("pk_{}", entity);

    Spi::run(&format!(
        "UPDATE {} SET data = $1 WHERE {} = $2",
        quote_identifier(&table_name),
        quote_identifier(&pk_column)
    ))?;

    // Note: actual implementation needs proper parameterized query
    // This is pseudocode showing the intent

    Ok(())
}
```

### Step 2: Complete cached refresh implementation

```rust
// In src/refresh/cache.rs:

/// Refresh a single entity+pk using cached prepared statement
///
/// Performance: 10x faster than uncached path by avoiding query parsing.
pub fn refresh_pk_with_cached_plan(entity: &str, pk: i64) -> TViewResult<()> {
    let stmt_name = get_or_prepare_statement(entity)?;

    Spi::connect(|client| {
        // Execute cached prepared statement
        let args = vec![unsafe {
            DatumWithOid::new(pk, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value())
        }];

        let mut result = client.select(
            &format!("EXECUTE {}", stmt_name),
            None,
            &args,
        )?;

        if let Some(row) = result.next() {
            // Extract the new data
            let new_data: JsonB = row["data"].value()?
                .ok_or_else(|| spi::Error::from(crate::TViewError::SpiError {
                    query: String::new(),
                    error: "data column is NULL".to_string(),
                }))?;

            // Apply the data to TVIEW table
            let table_name = format!("tv_{}", entity);
            let pk_column = format!("pk_{}", entity);

            // Use UPDATE with the fetched data
            let update_args = vec![
                unsafe { DatumWithOid::new(new_data.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(pk, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
            ];

            client.update(
                &format!(
                    "UPDATE {} SET data = $1 WHERE {} = $2",
                    table_name, pk_column
                ),
                None,
                &update_args,
            )?;

            info!("TVIEW: Refreshed {}[{}] with cached plan", entity, pk);
        } else {
            // Row not found in view - might be deleted or filtered out
            // This is not necessarily an error for views with WHERE clauses
            warning!("TVIEW: No row found for {}[{}] during cached refresh", entity, pk);
        }

        Ok(())
    })
}
```

### Step 3: Add cache usage decision logic

```rust
/// Decide whether to use cached or uncached refresh path
///
/// Cached path is preferred for:
/// - Simple single-row refreshes
/// - Entities with stable view definitions
///
/// Uncached path is needed for:
/// - First refresh (cache not populated)
/// - Complex multi-row refreshes
/// - After schema changes
pub fn should_use_cached_refresh(entity: &str) -> bool {
    // Check if statement is already cached
    let cache = PREPARED_STATEMENTS.lock().unwrap();
    cache.contains_key(entity)
}
```

### Step 4: Integrate with main refresh dispatcher

In the main refresh entry point, add logic to choose the fast path:

```rust
// In refresh dispatcher:
pub fn refresh_entity_pk(entity: &str, pk: i64) -> TViewResult<()> {
    // Try cached path first for performance
    if crate::config::cache_enabled() && should_use_cached_refresh(entity) {
        match refresh_pk_with_cached_plan(entity, pk) {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Cache might be stale, clear and fall back to uncached
                warning!("Cached refresh failed, falling back: {}", e);
                clear_prepared_statement_cache();
            }
        }
    }

    // Fall back to uncached path
    refresh_pk_uncached(entity, pk)
}
```

## Verification Commands

```bash
# Build check
cargo check --no-default-features --features pg18

# Run clippy
cargo clippy --no-default-features --features pg18 -- -D warnings

# Run tests
cargo test --no-default-features --features pg18 -- cache
```

## Performance Verification

```sql
-- Benchmark setup
CREATE TABLE tb_bench (pk_bench BIGINT PRIMARY KEY, name TEXT);
SELECT pg_tviews_create('bench', 'SELECT pk_bench, jsonb_build_object(''name'', name) as data FROM tb_bench');

-- Insert test data
INSERT INTO tb_bench SELECT i, 'name_' || i FROM generate_series(1, 1000) i;

-- Benchmark uncached (first run, cache cold)
\timing on
UPDATE tb_bench SET name = name || '_v2' WHERE pk_bench = 1;
-- Note the time

-- Benchmark cached (subsequent run, cache warm)
UPDATE tb_bench SET name = name || '_v3' WHERE pk_bench = 1;
-- Should be noticeably faster
```

## Acceptance Criteria

- [ ] Cached refresh actually updates TVIEW table
- [ ] Performance improvement measurable (>2x faster)
- [ ] Fallback to uncached path works
- [ ] Cache invalidation on schema change
- [ ] Code compiles without warnings
- [ ] Clippy passes

## DO NOT

- Do not change the uncached refresh logic
- Do not remove the caching infrastructure
- Do not make cached path the only option (fallback needed)
- Do not add external dependencies

## Notes

This is an optimization phase. The extension works correctly without it - the uncached path is always available. This phase enables the performance benefit documented in the cache module.
