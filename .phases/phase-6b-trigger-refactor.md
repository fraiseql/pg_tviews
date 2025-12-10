# Phase 6B: Trigger Refactoring

**Status:** BLOCKED (requires Phase 6A complete)
**Prerequisites:** Phase 6A Foundation ✅
**Estimated Time:** 1-2 days
**TDD Phase:** RED → GREEN → REFACTOR

---

## Objective

Convert triggers from **immediate refresh** (Phase 1-5) to **enqueue-only** (Phase 6):

1. Modify `pg_tview_trigger_handler()` to enqueue instead of calling `pg_tviews_cascade()`
2. Implement `entity_for_table()` to map table OID → entity name
3. Register commit callback once per transaction
4. Maintain backward compatibility during transition

---

## Context

### Current Behavior (Phase 1-5)

```rust
#[pg_trigger]
fn pg_tview_trigger_handler(trigger: &PgTrigger) -> Result<...> {
    let table_oid = trigger.relation()?.oid();
    let pk_value = extract_pk(trigger)?;

    // IMMEDIATE refresh - happens NOW
    pg_tviews_cascade(table_oid, pk_value);

    Ok(None)
}
```

**Problem**: Multiple updates to the same row trigger multiple refreshes.

### Target Behavior (Phase 6)

```rust
#[pg_trigger]
fn pg_tview_trigger_handler(trigger: &PgTrigger) -> Result<...> {
    let table_oid = trigger.relation()?.oid();
    let pk_value = extract_pk(trigger)?;

    // Map table OID → entity name
    let entity = entity_for_table(table_oid)?;

    // ENQUEUE for later (at commit)
    enqueue_refresh(&entity, pk_value)?;

    // Register callback (once per transaction)
    register_commit_callback_once()?;

    Ok(None)
}
```

**Benefit**: Multiple updates to `tb_user` row 42 → single refresh of `tv_user` row 42 at commit.

---

## Files to Modify

### 1. `src/trigger.rs`

**Current** (Phase 5 Task 7):
```rust
#[pg_trigger]
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    let table_oid = trigger.relation()?.oid();
    let pk_value = crate::utils::extract_pk(trigger)?;

    // Immediate refresh
    crate::pg_tviews_cascade(table_oid, pk_value);

    Ok(None)
}
```

**Target** (Phase 6B):
```rust
use pgrx::prelude::*;
use pgrx::spi;
use crate::queue::{enqueue_refresh, register_commit_callback_once};
use crate::catalog::entity_for_table;

#[pg_trigger]
fn pg_tview_trigger_handler<'a>(
    trigger: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, spi::Error> {
    // Extract table OID and PK
    let table_oid = trigger.relation()?.oid();
    let pk_value = match crate::utils::extract_pk(trigger) {
        Ok(pk) => pk,
        Err(e) => {
            warning!("Failed to extract primary key from trigger: {:?}", e);
            return Ok(None);
        }
    };

    // Map table OID → entity name
    let entity = match entity_for_table(table_oid) {
        Ok(Some(e)) => e,
        Ok(None) => {
            // Table not in pg_tview_meta, skip
            return Ok(None);
        }
        Err(e) => {
            warning!("Failed to resolve entity for table OID {:?}: {:?}", table_oid, e);
            return Ok(None);
        }
    };

    // Enqueue refresh request (deferred to commit)
    if let Err(e) = enqueue_refresh(&entity, pk_value) {
        warning!("Failed to enqueue refresh for {}[{}]: {:?}", entity, pk_value, e);
        return Ok(None);
    }

    // Register commit callback (once per transaction)
    if let Err(e) = register_commit_callback_once() {
        warning!("Failed to register commit callback: {:?}", e);
        return Ok(None);
    }

    Ok(None)
}
```

### 2. `src/catalog.rs` (or `src/catalog/table_mapping.rs`)

Add function to map table OID → entity name:

```rust
use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use crate::TViewResult;

/// Map a base table OID to its entity name
///
/// Example: OID of tb_user → Some("user")
///
/// Returns:
/// - Ok(Some(entity)) if table is tracked in pg_tview_meta
/// - Ok(None) if table is not tracked
/// - Err(...) on database error
pub fn entity_for_table(table_oid: Oid) -> TViewResult<Option<String>> {
    // Strategy 1: Query pg_tview_meta.dependencies
    // The dependencies column contains all base table OIDs that a TVIEW depends on.
    // We need to reverse this: given a base table OID, find which entity it maps to.

    // Note: A base table can map to multiple entities (e.g., tb_user might be a dependency
    // of tv_user, tv_post, tv_feed). For trigger purposes, we need the PRIMARY entity
    // (the one where this table is the main source, not just a FK dependency).

    // Approach: Check if table name matches "tb_<entity>" pattern
    let table_name = Spi::get_one::<String>(&format!(
        "SELECT relname::text FROM pg_class WHERE oid = {:?}",
        table_oid
    ))?.ok_or_else(|| TViewError::SpiError {
        query: format!("SELECT relname FROM pg_class WHERE oid = {:?}", table_oid),
        error: "Table OID not found".to_string(),
    })?;

    if let Some(entity) = table_name.strip_prefix("tb_") {
        Ok(Some(entity.to_string()))
    } else {
        // Not a tb_* table, skip
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_for_table_name_parsing() {
        // This is a unit test that doesn't require database access
        let test_cases = vec![
            ("tb_user", Some("user")),
            ("tb_post", Some("post")),
            ("tb_company", Some("company")),
            ("users", None),  // Not a tb_* table
            ("pg_class", None),  // System table
        ];

        for (table_name, expected_entity) in test_cases {
            let result = if let Some(entity) = table_name.strip_prefix("tb_") {
                Some(entity.to_string())
            } else {
                None
            };

            assert_eq!(result.as_deref(), expected_entity);
        }
    }
}
```

---

## Implementation Steps

### Step 1: Implement `entity_for_table()` (RED → GREEN)

1. **RED**: Write test in `src/catalog.rs` (or `src/catalog/table_mapping.rs`)
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn test_entity_for_table_name_parsing() {
           // Test tb_* → entity mapping logic
           assert_eq!(parse_entity_from_table_name("tb_user"), Some("user"));
           assert_eq!(parse_entity_from_table_name("users"), None);
       }
   }
   ```

2. **GREEN**: Implement `entity_for_table()`
   - Query `pg_class` to get table name from OID
   - Strip `tb_` prefix
   - Return `Ok(Some(entity))` if match, `Ok(None)` if not

3. **Test**: `cargo test --lib catalog::`

### Step 2: Refactor Trigger Handler (RED → GREEN)

1. **RED**: Write integration test (this will fail until Phase 6C provides commit handler)
   ```rust
   // In src/trigger.rs tests module
   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn test_trigger_enqueues_refresh() {
           // This test will be implemented in Phase 6C
           // For now, just verify trigger doesn't crash
       }
   }
   ```

2. **GREEN**: Modify `pg_tview_trigger_handler()`
   - Replace `pg_tviews_cascade(table_oid, pk_value)` with:
     - `entity_for_table(table_oid)?`
     - `enqueue_refresh(&entity, pk_value)?`
     - `register_commit_callback_once()?`
   - Add error handling (warnings, not errors)

3. **Compile**: `cargo clippy --release -- -D warnings`

### Step 3: Add Module Exports

Update `src/catalog.rs` or `src/catalog/mod.rs`:

```rust
mod table_mapping;
pub use table_mapping::entity_for_table;
```

Update `src/lib.rs`:

```rust
pub use catalog::entity_for_table;
```

### Step 4: Manual Testing (Placeholder)

Since commit callback handler is not yet implemented (Phase 6C), manual testing will show:

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- Trigger fires → enqueue() called → warning logged
COMMIT;
-- No refresh happens yet (Phase 6C will implement this)
```

Expected logs:
```
WARNING: No commit callback handler registered (Phase 6C TODO)
```

---

## Verification Commands

### Compilation Check
```bash
cargo clippy --release -- -D warnings
```

### Unit Tests
```bash
cargo test --lib catalog::entity_for_table
cargo test --lib trigger::
```

### Expected Behavior (Pre-Phase 6C)

At this point, triggers will **enqueue but not flush**:
- Triggers fire → enqueue succeeds
- Commit happens → queue is NOT flushed (no handler yet)
- TVIEWs remain stale until Phase 6C

This is expected and intentional. Phase 6C will implement the flush logic.

---

## Performance Considerations

### entity_for_table() Overhead

The `entity_for_table()` function queries `pg_class` on **every trigger fire**:

```rust
// Phase 6B implementation:
pub fn entity_for_table(table_oid: Oid) -> TViewResult<Option<String>> {
    // Queries: SELECT relname::text FROM pg_class WHERE oid = $1
    // Cost: ~0.1ms per trigger (cached by PostgreSQL's syscache, but still overhead)
}
```

**Performance Impact:**
- Single update: Negligible (~0.1ms)
- 1000 updates in transaction: ~100ms total overhead
- High-frequency writes: May become bottleneck

**Optimization Strategy (Phase 6B+):**

Add static cache in Rust (HashMap):

```rust
use std::sync::Mutex;
use once_cell::sync::Lazy;

static TABLE_ENTITY_CACHE: Lazy<Mutex<HashMap<pg_sys::Oid, String>>> = Lazy::new(|| {
    Mutex::new(HashMap::new())
});

pub fn entity_for_table_cached(table_oid: Oid) -> TViewResult<Option<String>> {
    // Fast path: check cache (no syscall)
    {
        let cache = TABLE_ENTITY_CACHE.lock().unwrap();
        if let Some(entity) = cache.get(&table_oid) {
            return Ok(Some(entity.clone()));
        }
    }

    // Slow path: query and cache
    let entity = entity_for_table_uncached(table_oid)?;

    if let Some(ref e) = entity {
        let mut cache = TABLE_ENTITY_CACHE.lock().unwrap();
        cache.insert(table_oid, e.clone());
    }

    Ok(entity)
}
```

**Trade-offs:**
- ✅ 100× faster (0.001ms vs 0.1ms per trigger)
- ✅ OIDs are stable within a PostgreSQL session
- ❌ Requires invalidation on CREATE/DROP TVIEW
- ❌ Adds ~50 lines of code

**Decision for Phase 6B:**
- **Ship without caching** (simple, correct, "good enough")
- **Benchmark first** (measure 10K updates/transaction)
- **Optimize if needed** (add cache if >5% of transaction time)

---

## Acceptance Criteria

- ✅ `entity_for_table()` correctly maps `tb_*` tables to entity names
- ✅ `pg_tview_trigger_handler()` calls `enqueue_refresh()` instead of `pg_tviews_cascade()`
- ✅ Triggers compile and don't crash
- ✅ Clippy strict compliance (0 warnings)
- ⚠️ TVIEWs remain stale (expected until Phase 6C)

---

## DO NOT

- ❌ Implement commit callback handler (that's Phase 6C)
- ❌ Remove `pg_tviews_cascade()` function (still used for testing)
- ❌ Expect TVIEWs to refresh (Phase 6C will enable this)

---

## Rollback Strategy

If Phase 6B breaks existing functionality:

1. Revert trigger changes:
   ```rust
   // Restore immediate refresh
   crate::pg_tviews_cascade(table_oid, pk_value);
   ```

2. Keep `entity_for_table()` (useful for future)

3. Investigate issue before re-attempting Phase 6B

---

## Next Phase

After Phase 6B is complete and triggers are enqueue-only:
**Read**: `.phases/phase-6c-commit-processing.md`
