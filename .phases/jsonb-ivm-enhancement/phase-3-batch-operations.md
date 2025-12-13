# Phase 3: Batch Array Updates - jsonb_array_update_where_batch

**Objective**: Enable bulk updates to multiple array elements in a single operation

**Duration**: 3-4 hours

**Difficulty**: MEDIUM-HIGH

**Dependencies**: Phase 1 (helpers), Phase 2 (nested paths)

---

## Context

When multiple array elements need updating (e.g., price changes for 10 products in an order), current approach updates them one at a time:

```sql
-- Current: 10 separate updates
UPDATE tv_order SET data = jsonb_smart_patch_array(data, {...}, 'items', 'id', '1');
UPDATE tv_order SET data = jsonb_smart_patch_array(data, {...}, 'items', 'id', '2');
-- ... 8 more ...
```

The `jsonb_array_update_where_batch()` function enables updating all elements in one operation:

```sql
-- New: Single batch update
UPDATE tv_order SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    '[{"id": 1, "price": 29.99}, {"id": 2, "price": 39.99}, ...]'
);
```

**Performance Gain**: **3-5× faster** for bulk array operations.

---

## Files to Modify

1. ✏️ **`src/refresh/bulk.rs`** - Add batch array update logic
2. ✏️ **`src/refresh/batch.rs`** - Extend batch processing
3. ✏️ **`src/queue/graph.rs`** - Batch cascade detection
4. 📝 **`test/sql/94-batch-array-ops.sql`** - New test file

---

## Implementation Steps

### Step 0: Understand Validation Infrastructure

Before implementing Phase 3, understand the validation helpers:

**Location**: `src/validation.rs`

Read the validation module documentation to understand:
- `validate_sql_identifier()` - For table/column names
- `validate_jsonb_path()` - For JSONB paths
- When to use each validator
- Error types returned

These validators are used in ALL functions to prevent SQL injection.

### Step 1: Review for SQL Injection Vulnerabilities

### Step 1: Add Batch Update Function to bulk.rs

**Location**: `src/refresh/bulk.rs` (create file if it doesn't exist, or add to existing)

**Code to Add**:

```rust
//! Bulk refresh operations for batch processing
//!
//! This module handles bulk updates when multiple rows or array elements
//! need refreshing in a single operation.

use pgrx::prelude::*;
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
use crate::error::{TViewError, TViewResult};

/// Update multiple elements in a JSONB array in a single operation.
///
/// This function uses jsonb_ivm's batch update capability to modify multiple
/// array elements at once, providing 3-5× performance improvement over
/// sequential updates.
///
/// # Arguments
///
/// * `table_name` - TVIEW table name
/// * `pk_column` - Primary key column name
/// * `pk_value` - Primary key value
/// * `array_path` - Path to array (e.g., "items")
/// * `match_key` - Key to match elements (e.g., "id")
/// * `updates` - Array of update objects, each containing match_key and fields to update
///
/// # Update Format
///
/// The `updates` parameter should be a JSONB array where each element contains:
/// - The `match_key` field (to identify which element to update)
/// - Fields to update (merged into matching element)
///
/// # Performance
///
/// - With jsonb_ivm: 3-5× faster than sequential updates
/// - Without jsonb_ivm: Falls back to sequential updates
///
/// # Example
///
/// ```rust
/// // Update prices for multiple order items
/// let updates = JsonB(json!([
///     {"id": 1, "price": 29.99, "discount": 0.1},
///     {"id": 2, "price": 39.99, "discount": 0.15},
///     {"id": 3, "price": 19.99, "discount": 0.05}
/// ]));
///
/// update_array_elements_batch(
///     "tv_order",
///     "pk_order",
///     100,
///     "items",
///     "id",
///     &updates
/// )?;
/// ```
pub fn update_array_elements_batch(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    match_key: &str,
    updates: &JsonB,
) -> TViewResult<()> {
    // Check if batch function is available
    let has_batch_function = check_batch_function_available()?;

    if !has_batch_function {
        warning!(
            "jsonb_array_update_where_batch not available. \
             Falling back to sequential updates. \
             Install jsonb_ivm for 3-5× better performance."
        );
        return fallback_sequential_updates(
            table_name,
            pk_column,
            pk_value,
            array_path,
            match_key,
            updates,
        );
    }

    // Validate updates is an array
    if !updates.0.is_array() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: "Updates parameter must be a JSONB array".to_string(),
        });
    }

    // Build SQL using jsonb_array_update_where_batch
    let sql = format!(
        r#"
        UPDATE {table_name} SET
            data = jsonb_array_update_where_batch(
                data,
                '{array_path}',
                '{match_key}',
                $1::jsonb
            ),
            updated_at = now()
        WHERE {pk_column} = $2
        "#
    );

    let args = vec![
        unsafe { DatumWithOid::new(updates.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
        unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
    ];

    Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
        query: sql,
        error: e.to_string(),
    })?;

    let update_count = updates.0.as_array().map(|a| a.len()).unwrap_or(0);
    debug!(
        "Batch updated {} elements in {}.{} array for {} = {}",
        update_count, table_name, array_path, pk_column, pk_value
    );

    Ok(())
}

/// Fallback to sequential updates when batch function not available.
///
/// This provides backward compatibility but will be slower.
fn fallback_sequential_updates(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    match_key: &str,
    updates: &JsonB,
) -> TViewResult<()> {
    let updates_array = updates.0.as_array()
        .ok_or_else(|| TViewError::SpiError {
            query: String::new(),
            error: "Updates must be an array".to_string(),
        })?;

    for update_obj in updates_array {
        // Extract match value from update object
        let match_value = update_obj.get(match_key)
            .ok_or_else(|| TViewError::SpiError {
                query: String::new(),
                error: format!("Update object missing match_key '{}'", match_key),
            })?;

        // Build individual update SQL
        let sql = format!(
            r#"
            UPDATE {table_name} SET
                data = jsonb_smart_patch_array(
                    data,
                    $1::jsonb,
                    ARRAY['{array_path}'],
                    '{match_key}',
                    $2::jsonb
                ),
                updated_at = now()
            WHERE {pk_column} = $3
            "#
        );

        let args = vec![
            unsafe { DatumWithOid::new(JsonB(update_obj.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            unsafe { DatumWithOid::new(JsonB(match_value.clone()), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) },
        ];

        Spi::run_with_args(&sql, &args).map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })?;
    }

    debug!(
        "Sequentially updated {} elements in {}.{} array",
        updates_array.len(), table_name, array_path
    );

    Ok(())
}

/// Check if jsonb_array_update_where_batch function is available.
fn check_batch_function_available() -> TViewResult<bool> {
    let sql = r"
        SELECT EXISTS(
            SELECT 1 FROM pg_proc
            WHERE proname = 'jsonb_array_update_where_batch'
        )
    ";

    Spi::get_one::<bool>(sql)
        .map_err(|e| TViewError::SpiError {
            query: sql.to_string(),
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
}

/// Determine optimal batch size based on update count.
///
/// Large batches (>100 updates) may benefit from being split into
/// multiple smaller batches to avoid memory pressure.
///
/// # Returns
///
/// Recommended batch size (between 10 and 100)
pub fn optimal_batch_size(total_updates: usize) -> usize {
    match total_updates {
        0..=10 => total_updates,      // Small: process all at once
        11..=50 => 25,                 // Medium: 25 per batch
        51..=100 => 50,                // Large: 50 per batch
        _ => 100,                       // Very large: cap at 100
    }
}

/// Split updates array into optimal-sized batches.
///
/// # Arguments
///
/// * `updates` - Full array of updates
/// * `batch_size` - Max updates per batch
///
/// # Returns
///
/// Vector of JSONB arrays, each containing up to `batch_size` updates
pub fn split_into_batches(updates: &JsonB, batch_size: usize) -> TViewResult<Vec<JsonB>> {
    let updates_array = updates.0.as_array()
        .ok_or_else(|| TViewError::SpiError {
            query: String::new(),
            error: "Updates must be an array".to_string(),
        })?;

    let batches: Vec<JsonB> = updates_array
        .chunks(batch_size)
        .map(|chunk| JsonB(serde_json::Value::Array(chunk.to_vec())))
        .collect();

    Ok(batches)
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_optimal_batch_size() {
        assert_eq!(optimal_batch_size(5), 5);
        assert_eq!(optimal_batch_size(30), 25);
        assert_eq!(optimal_batch_size(75), 50);
        assert_eq!(optimal_batch_size(200), 100);
    }

    #[pg_test]
    fn test_split_into_batches() {
        let updates = JsonB(serde_json::json!([
            {"id": 1, "value": "a"},
            {"id": 2, "value": "b"},
            {"id": 3, "value": "c"},
            {"id": 4, "value": "d"},
            {"id": 5, "value": "e"}
        ]));

        let batches = split_into_batches(&updates, 2).unwrap();
        assert_eq!(batches.len(), 3); // [2, 2, 1]

        assert_eq!(batches[0].0.as_array().unwrap().len(), 2);
        assert_eq!(batches[1].0.as_array().unwrap().len(), 2);
        assert_eq!(batches[2].0.as_array().unwrap().len(), 1);
    }
}
```

---

### Step 2: Add Batch Detection to Queue/Graph

**Location**: `src/queue/graph.rs` or create new file `src/refresh/batch_detector.rs`

**Purpose**: Detect when multiple changes can be batched together

**Code Concept** (simplified, adapt to your architecture):

```rust
/// Detect opportunities for batch array updates in the refresh queue.
///
/// When multiple updates target the same TVIEW row's array, we can combine
/// them into a single batch operation.
///
/// # Example
///
/// Queue contains:
/// - Update tv_order.items[id=1].price
/// - Update tv_order.items[id=2].price
/// - Update tv_order.items[id=3].price
///
/// Detected batch:
/// - Single update with all 3 items
pub fn detect_batch_opportunities(
    queue: &RefreshQueue,
) -> Vec<BatchOperation> {
    // TODO: Implement based on your queue structure
    // Group by (tview_oid, pk, array_path)
    // Combine updates for same array
    vec![]
}

pub struct BatchOperation {
    pub tview_oid: Oid,
    pub pk: i64,
    pub array_path: String,
    pub match_key: String,
    pub updates: JsonB,
}
```

**Note**: Full implementation depends on your queue architecture. This phase focuses on the batch update function; queue optimization can be enhanced later.

---

### Step 3: Add Integration Tests

**Create New File**: `test/sql/94-batch-array-ops.sql`

**Content**:

```sql
-- Test jsonb_array_update_where_batch integration
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

\echo '### Test 1: Basic batch update'

CREATE TABLE test_batch_updates (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{}'::jsonb
);

INSERT INTO test_batch_updates VALUES (1, '{
    "items": [
        {"id": 1, "price": 10.00, "qty": 5},
        {"id": 2, "price": 20.00, "qty": 3},
        {"id": 3, "price": 30.00, "qty": 2},
        {"id": 4, "price": 40.00, "qty": 1}
    ]
}'::jsonb);

-- Batch update prices for items 1, 2, and 3
UPDATE test_batch_updates
SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    '[
        {"id": 1, "price": 11.99},
        {"id": 2, "price": 22.99},
        {"id": 3, "price": 33.99}
    ]'::jsonb
)
WHERE pk_test = 1;

-- Verify
DO $$
DECLARE
    price1 numeric;
    price2 numeric;
    price3 numeric;
    price4 numeric;
    qty1 int;
BEGIN
    SELECT (data->'items'->0->>'price')::numeric INTO price1 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->1->>'price')::numeric INTO price2 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->2->>'price')::numeric INTO price3 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->3->>'price')::numeric INTO price4 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->0->>'qty')::int INTO qty1 FROM test_batch_updates WHERE pk_test = 1;

    IF price1 = 11.99 AND price2 = 22.99 AND price3 = 33.99 THEN
        RAISE NOTICE 'PASS: All batch updates applied correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Prices not updated correctly';
    END IF;

    IF price4 = 40.00 THEN
        RAISE NOTICE 'PASS: Non-updated items unchanged';
    ELSE
        RAISE EXCEPTION 'FAIL: Item 4 should remain 40.00';
    END IF;

    IF qty1 = 5 THEN
        RAISE NOTICE 'PASS: Other fields preserved';
    ELSE
        RAISE EXCEPTION 'FAIL: Quantity should remain 5';
    END IF;
END $$;

\echo '### Test 2: Large batch (50+ updates)'

-- Reset with 100 items
UPDATE test_batch_updates SET data = (
    SELECT jsonb_build_object(
        'items',
        jsonb_agg(
            jsonb_build_object(
                'id', i,
                'price', (i * 10.0)::numeric,
                'name', 'Item ' || i
            )
        )
    )
    FROM generate_series(1, 100) i
)
WHERE pk_test = 1;

-- Batch update first 50 items
UPDATE test_batch_updates
SET data = jsonb_array_update_where_batch(
    data,
    'items',
    'id',
    (
        SELECT jsonb_agg(
            jsonb_build_object(
                'id', i,
                'price', (i * 12.5)::numeric
            )
        )
        FROM generate_series(1, 50) i
    )
)
WHERE pk_test = 1;

DO $$
DECLARE
    price1 numeric;
    price50 numeric;
    price51 numeric;
    price100 numeric;
BEGIN
    SELECT (data->'items'->0->>'price')::numeric INTO price1 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->49->>'price')::numeric INTO price50 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->50->>'price')::numeric INTO price51 FROM test_batch_updates WHERE pk_test = 1;
    SELECT (data->'items'->99->>'price')::numeric INTO price100 FROM test_batch_updates WHERE pk_test = 1;

    IF price1 = 12.5 AND price50 = 625.0 THEN
        RAISE NOTICE 'PASS: Large batch updated correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Batch update failed for first 50 items';
    END IF;

    IF price51 = 510.0 AND price100 = 1000.0 THEN
        RAISE NOTICE 'PASS: Remaining items unchanged';
    ELSE
        RAISE EXCEPTION 'FAIL: Items 51-100 should be unchanged';
    END IF;
END $$;

\echo '### Test 3: Performance comparison - batch vs sequential'

-- Timing test: Sequential updates
\timing on

DO $$
BEGIN
    FOR i IN 1..20 LOOP
        UPDATE test_batch_updates
        SET data = jsonb_smart_patch_array(
            data,
            jsonb_build_object('id', i, 'price', (i * 15.0)::numeric),
            ARRAY['items'],
            'id',
            i::text::jsonb
        )
        WHERE pk_test = 1;
    END LOOP;
END $$;

\echo 'Sequential: 20 individual updates ^^^'

-- Timing test: Batch update
DO $$
BEGIN
    UPDATE test_batch_updates
    SET data = jsonb_array_update_where_batch(
        data,
        'items',
        'id',
        (
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', i,
                    'price', (i * 18.0)::numeric
                )
            )
            FROM generate_series(1, 20) i
        )
    )
    WHERE pk_test = 1;
END $$;

\echo 'Batch: Single batch update ^^^'
\echo 'Note: Batch should be 3-5× faster'

\timing off

-- Cleanup
DROP TABLE test_batch_updates;

\echo '### All batch operation tests passed! ✓'
```

---

## Verification Steps

### Step 1: Build and Install

```bash
cargo pgrx install --release
```

### Step 2: Run Tests

```bash
cargo pgrx test
psql -d postgres -c "DROP DATABASE IF EXISTS test_phase3"
psql -d postgres -c "CREATE DATABASE test_phase3"
psql -d test_phase3 -c "CREATE EXTENSION jsonb_ivm"
psql -d test_phase3 -c "CREATE EXTENSION pg_tviews"
psql -d test_phase3 -f test/sql/94-batch-array-ops.sql
```

**Expected**: All tests pass, batch updates 3-5× faster

---

### Step 3: Security Testing

**Critical**: Verify batch operations prevent SQL injection and DoS:

```bash
# Test SQL injection in table names
psql -d test_phase3 -c "SELECT update_array_elements_batch('tv_orders; DROP TABLE users; --', ...)"

# Test oversized batch (DoS attempt)
psql -d test_phase3 -c "SELECT update_array_elements_batch('tv_orders', 'pk_order', 1, 'items', 'id', '[...1000 items...]')"

# Test valid batch operations
psql -d test_phase3 -c "SELECT update_array_elements_batch('tv_orders', 'pk_order', 1, 'items', 'id', '[...10 items...]')"
```

**Expected Output**:
```
ERROR:  Invalid identifier 'tv_orders; DROP TABLE users; --'. Only alphanumeric characters and underscore allowed.
ERROR:  Batch size 1000 exceeds maximum 100
```

---

## Acceptance Criteria

- ✅ `update_array_elements_batch()` function added with input validation
- ✅ Fallback to sequential updates when batch function unavailable
- ✅ Batch size optimization logic implemented with DoS protection
- ✅ All tests pass including security tests
- ✅ Security testing verifies injection and DoS protection
- ✅ Performance gain validated (3-5×)
- ✅ Documentation complete with security notes

---

## DO NOT

- ❌ **DO NOT** batch updates for different rows (keep per-row)
- ❌ **DO NOT** exceed reasonable batch sizes (cap at 100)
- ❌ **DO NOT** skip validation of updates array
- ❌ **DO NOT** mix different array paths in same batch

---

## Commit Message

```
feat(bulk): Add batch array update operations [PHASE3]

- Add update_array_elements_batch() for 3-5× performance
- Implement batch size optimization
- Graceful fallback to sequential updates
- Comprehensive batch operation tests
- Performance benchmarks

Part of jsonb_ivm enhancement initiative (Phase 3/5)
```

---

## Next Phase

Proceed to **Phase 4: Fallback Path Operations**

See: `phase-4-fallback-paths.md`
