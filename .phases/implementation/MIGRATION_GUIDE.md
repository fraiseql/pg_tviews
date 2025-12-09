# Migration Guide: Original Plans → Corrected Plans

This document helps implementers understand what needs to be changed in the original phase plans (phases 0, 1, 5) that haven't been fully rewritten.

---

## Overview

**Fully Rewritten (Use These):**
- ✅ Phase 0-A: Error Types & Safety (NEW)
- ✅ Phase 2: View & Table Creation (FIXED)
- ✅ Phase 3: Dependency Detection (FIXED)
- ✅ Phase 4: Refresh & Cascade (FIXED)

**Need Updates (Apply Changes Below):**
- ⚠️ Phase 0: Foundation
- ⚠️ Phase 1: Schema Inference
- ⚠️ Phase 5: Arrays & Optimization

---

## Phase 0: Foundation - Changes Required

**File:** `phase-0-foundation.md`

### Change 1: Update Cargo.toml Dependencies

```toml
[dependencies]
pgrx = "=0.12.8"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
regex = "1.10"  # Add version

[dev-dependencies]
pgrx-tests = "=0.12.8"
```

### Change 2: Update src/lib.rs

```rust
// OLD:
use pgrx::prelude::*;

::pgrx::pg_module_magic!();

// NEW:
use pgrx::prelude::*;

mod error;  // ADD THIS
pub use error::{TViewError, TViewResult};  // ADD THIS

::pgrx::pg_module_magic!();

#[pg_guard]
extern "C" fn _PG_init() {
    // Create metadata tables on extension load
    if let Err(e) = metadata::create_metadata_tables() {
        error!("Failed to initialize pg_tviews metadata: {}", e);  // SAME
    }

    // NEW: Install ProcessUtility hook
    crate::hooks::install_hooks();  // ADD THIS
}
```

### Change 3: Update Metadata Module

```rust
// src/metadata.rs

// OLD signature:
pub fn create_metadata_tables() -> Result<(), Box<dyn std::error::Error>> {

// NEW signature:
use crate::error::{TViewError, TViewResult};

pub fn create_metadata_tables() -> TViewResult<()> {
    Spi::run(
        r#"
        CREATE TABLE IF NOT EXISTS public.pg_tview_meta (
            entity TEXT NOT NULL PRIMARY KEY,
            view_oid OID NOT NULL,
            table_oid OID NOT NULL,
            definition TEXT NOT NULL,
            dependencies OID[] NOT NULL DEFAULT '{}',
            fk_columns TEXT[] NOT NULL DEFAULT '{}',
            uuid_fk_columns TEXT[] NOT NULL DEFAULT '{}',
            dependency_types TEXT[] NOT NULL DEFAULT '{}',
            dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
            array_match_keys TEXT[] NOT NULL DEFAULT '{}',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        -- ... rest of SQL
        "#,
    )
    .map_err(|e| TViewError::CatalogError {  // CHANGE THIS
        operation: "Create metadata tables".to_string(),
        pg_error: format!("{:?}", e),
    })?;

    Ok(())
}
```

### Change 4: Add Config Module

Create `src/config.rs`:

```rust
//! Configuration constants for pg_tviews

/// Maximum dependency depth for pg_depend traversal
pub const MAX_DEPENDENCY_DEPTH: usize = 10;

/// Maximum cascade depth for refresh propagation
pub const MAX_CASCADE_DEPTH: usize = 10;

/// Maximum batch size for bulk operations
pub const MAX_BATCH_SIZE: usize = 10000;

/// Lock timeout for metadata operations (milliseconds)
pub const METADATA_LOCK_TIMEOUT_MS: u64 = 5000;
```

Add to `src/lib.rs`:

```rust
pub mod config;
```

### Change 5: Add Hooks Module Placeholder

Create `src/hooks/mod.rs`:

```rust
//! PostgreSQL hook integration

use pgrx::prelude::*;
use std::sync::Once;

static INIT_HOOK: Once = Once::new();

pub fn install_hooks() {
    INIT_HOOK.call_once(|| {
        info!("pg_tviews hooks initialized (ProcessUtility hook will be added in Phase 2)");
        // Hook installation code will be added in Phase 2
    });
}
```

---

## Phase 1: Schema Inference - Changes Required

**File:** `phase-1-schema-inference.md`

### Change 1: Update Return Types

**Find all functions and change:**

```rust
// OLD:
pub fn infer_schema(sql: &str) -> Result<TViewSchema, String> {

// NEW:
use crate::error::{TViewError, TViewResult};

pub fn infer_schema(sql: &str) -> TViewResult<TViewSchema> {
```

### Change 2: Update Error Returns

```rust
// OLD:
if columns.is_empty() {
    return Err("No columns found in SELECT statement".to_string());
}

// NEW:
if columns.is_empty() {
    return Err(TViewError::InvalidSelectStatement {
        sql: sql.to_string(),
        reason: "No columns found in SELECT statement".to_string(),
    });
}
```

### Change 3: Update Parser Errors

```rust
// src/schema/parser.rs

// OLD:
fn extract_columns_regex(sql: &str) -> Result<Vec<String>, String> {
    let select_start = sql.to_lowercase().find("select")
        .ok_or("No SELECT found")?;

// NEW:
fn extract_columns_regex(sql: &str) -> TViewResult<Vec<String>> {
    let select_start = sql.to_lowercase().find("select")
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "No SELECT keyword found".to_string(),
        })?;
```

### Change 4: Update Type Inference Errors

```rust
// src/schema/types.rs

// OLD:
pub fn infer_column_types(
    table_name: &str,
    columns: &[String],
) -> Result<HashMap<String, String>, String> {

// NEW:
pub fn infer_column_types(
    table_name: &str,
    columns: &[String],
) -> TViewResult<HashMap<String, String>> {

    // OLD error:
    Err(format!("Failed to get type for column {}: {}", col, e))?

    // NEW error:
    .map_err(|e| TViewError::TypeInferenceFailed {
        column_name: col.clone(),
        reason: format!("{:?}", e),
    })?
```

### Change 5: Add Parser Limitations Warning

At the top of `src/schema/parser.rs`:

```rust
//! SQL SELECT statement parser
//!
//! # Limitations (v1)
//!
//! This v1 implementation uses regex-based parsing with known limitations:
//!
//! **Supported:**
//! - Simple SELECT with FROM clause
//! - JOINs (INNER, LEFT, RIGHT, FULL)
//! - WHERE, GROUP BY, ORDER BY
//! - Multi-line statements
//!
//! **NOT Supported:**
//! - CTEs (WITH clause)
//! - Parenthesized SELECT
//! - Comments in SELECT (may break parser)
//! - String literals containing 'AS' keyword
//!
//! # v2 Plan
//!
//! v2 will use PostgreSQL's native parser via SPI_prepare + pg_parse_query.
//! Expected: Q2 2026.

use crate::error::{TViewError, TViewResult};
use regex::Regex;

// ... rest of module
```

---

## Phase 5: Arrays & Optimization - Changes Required

**File:** `phase-5-arrays-and-optimization.md`

### Change 1: Update Return Types

Same pattern as Phase 1:

```rust
// OLD:
pub fn insert_array_element(...) -> Result<(), Box<dyn std::error::Error>> {

// NEW:
pub fn insert_array_element(...) -> TViewResult<()> {
```

### Change 2: Update Batch Refresh Errors

```rust
// src/refresh/batch.rs

// OLD:
if pk_values.len() > 10000 {
    return Err("Batch too large".into());
}

// NEW:
use crate::config::MAX_BATCH_SIZE;

if pk_values.len() > MAX_BATCH_SIZE {
    return Err(TViewError::BatchTooLarge {
        size: pk_values.len(),
        max_size: MAX_BATCH_SIZE,
    });
}
```

### Change 3: Add Array Semantics Documentation

At top of `src/refresh/array_ops.rs`:

```rust
//! Array element operations for JSONB arrays
//!
//! # Array Semantics
//!
//! ## Insert
//! - Elements are inserted at the end by default
//! - If `sort_key` is provided, array is kept sorted
//! - Duplicate elements are allowed
//!
//! ## Delete
//! - `delete_array_element` deletes **ALL** matching elements
//! - Matching is done by comparing `match_key` field
//! - Empty arrays after delete are valid
//!
//! ## Update
//! - `update_array_element` updates **FIRST** matching element
//! - If multiple elements match, only first is updated
//! - Use `match_key` to uniquely identify elements
//!
//! # Edge Cases
//!
//! - Empty arrays: Valid, no error
//! - NULL elements: Skipped during matching
//! - Duplicate elements: All operations work correctly
//!
//! # Performance
//!
//! - Insert: O(n) for sorted arrays, O(1) for unsorted
//! - Delete: O(n) - must scan entire array
//! - Update: O(n) - stops at first match
```

### Change 4: Add Array Tests

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[pg_test]
    fn test_insert_empty_array() {
        // Create TVIEW with empty array
        Spi::run("
            CREATE TABLE tb_base (pk INTEGER PRIMARY KEY);
            CREATE TVIEW tv_test AS
            SELECT pk, jsonb_build_object('items', '[]'::jsonb) AS data
            FROM tb_base;
        ").unwrap();

        // Insert into empty array should work
        let result = insert_array_element(
            "tv_test",
            "pk",
            1,
            "items",
            JsonB::from_value(json!({"id": 1})),
            None,
        );

        assert!(result.is_ok());
    }

    #[pg_test]
    fn test_delete_all_matching() {
        // Setup array with duplicates
        // ...

        // Delete should remove ALL matching elements
        let result = delete_array_element(
            "tv_test",
            "pk",
            1,
            "items",
            "id",
            JsonB::from_value(json!(42)),
        );

        // Verify count reduced by 2 (both duplicates removed)
        // ...
    }

    #[pg_test]
    fn test_null_elements_skipped() {
        // Array with NULL element: [{"id": 1}, null, {"id": 2}]
        // ...

        // Should skip NULL during matching
        // ...
    }
}
```

---

## Common Patterns for All Phases

### Pattern 1: Converting Generic Errors

```rust
// OLD:
.map_err(|e| format!("Error: {}", e))?

// NEW:
.map_err(|e| TViewError::CatalogError {
    operation: "Description of what failed".to_string(),
    pg_error: format!("{:?}", e),
})?
```

### Pattern 2: Converting String Errors

```rust
// OLD:
return Err("Something went wrong".into());

// NEW:
return Err(TViewError::InternalError {
    message: "Something went wrong".to_string(),
    file: file!(),
    line: line!(),
});

// OR use macro:
return Err(internal_error!("Something went wrong"));
```

### Pattern 3: Handling Option

```rust
// OLD:
let value = option.ok_or("Not found")?;

// NEW:
use crate::require;

let value = require!(option, TViewError::MetadataNotFound {
    entity: entity_name.to_string(),
});

// OR manually:
let value = option.ok_or_else(|| TViewError::MetadataNotFound {
    entity: entity_name.to_string(),
})?;
```

### Pattern 4: SPI Errors

```rust
// OLD:
Spi::run(sql)?;

// NEW:
Spi::run(sql)
    .map_err(|e| TViewError::SpiError {
        query: sql.to_string(),
        error: format!("{:?}", e),
    })?;
```

### Pattern 5: Validation Errors

```rust
// OLD:
if name.is_empty() {
    return Err("Empty name".into());
}

// NEW:
if name.is_empty() {
    return Err(TViewError::InvalidTViewName {
        name: name.to_string(),
        reason: "TVIEW name cannot be empty".to_string(),
    });
}
```

---

## Testing Changes

### Update Test Helpers

```rust
// OLD:
#[pg_test]
fn test_something() {
    let result = some_function();
    assert!(result.is_ok());
}

// NEW:
use crate::error::testing::*;

#[pg_test]
fn test_something() {
    let result = some_function();
    assert!(result.is_ok());
}

#[pg_test]
fn test_error_case() {
    let result = some_function();

    // Use helper to check error type
    assert_error_sqlstate(result, "P0001");

    // OR check error message
    assert_error_contains(result, "TVIEW not found");
}
```

---

## Build & Test Commands (Updated)

```bash
# Phase 0: After implementing error types
cargo test --lib  # Unit tests
cargo pgrx test pg17  # Integration tests

# Phase 1: After updating schema inference
cargo test --lib schema::  # Schema module tests
cargo pgrx test pg17  # Full integration

# Phase 2: After ProcessUtility hook
cargo pgrx test pg17  # Must test in pgrx environment
psql -d test -f test/sql/20_create_tview_simple.sql

# Phase 3: After dependency detection
cargo pgrx test pg17
psql -d test -f test/sql/30_dependency_detection_simple.sql

# Phase 4: After refresh logic
cargo pgrx test pg17
psql -d test -f test/sql/40_refresh_single_row.sql

# Phase 5: After array support
cargo pgrx test pg17
psql -d test -f test/sql/50_array_columns.sql
```

---

## Checklist: Converting a Phase

When updating an original phase plan:

- [ ] Change all `Result<T, Box<dyn Error>>` to `TViewResult<T>`
- [ ] Change all `Result<T, String>` to `TViewResult<T>`
- [ ] Replace string errors with `TViewError` variants
- [ ] Add `.map_err(|e| TViewError::...)` to all `?` operators
- [ ] Use `internal_error!()` macro for internal errors
- [ ] Add SAFETY comments to any `unsafe` blocks
- [ ] Update test helpers to use `assert_error_sqlstate`
- [ ] Add edge case tests (NULL, empty, limits)
- [ ] Update module documentation with limitations (if any)
- [ ] Verify all tests still pass

---

## Quick Reference: Error Mapping

| Original Error | New Error Variant |
|----------------|-------------------|
| "TVIEW not found" | `TViewError::MetadataNotFound { entity }` |
| "Circular dependency" | `TViewError::CircularDependency { cycle }` |
| "Invalid SQL" | `TViewError::InvalidSelectStatement { sql, reason }` |
| "Column missing" | `TViewError::RequiredColumnMissing { column_name, context }` |
| "Type inference failed" | `TViewError::TypeInferenceFailed { column_name, reason }` |
| "jsonb_ivm not installed" | `TViewError::JsonbIvmNotInstalled` |
| "Too deep" | `TViewError::DependencyDepthExceeded { depth, max_depth }` |
| "Lock timeout" | `TViewError::LockTimeout { resource, timeout_ms }` |
| "Refresh failed" | `TViewError::RefreshFailed { entity, pk_value, reason }` |
| "Batch too large" | `TViewError::BatchTooLarge { size, max_size }` |
| SPI errors | `TViewError::SpiError { query, error }` |
| Catalog errors | `TViewError::CatalogError { operation, pg_error }` |
| Internal/unknown | `TViewError::InternalError { message, file, line }` |

---

## Support

If you encounter issues during migration:

1. **Check error type:** Review `phase-0-error-types.md` for appropriate variant
2. **Check FIXED phases:** See if similar code exists in corrected phases
3. **Check tests:** Use `assert_error_sqlstate` helper
4. **Add context:** Error messages should include entity names, SQL fragments, etc.

**Remember:** The goal is consistent, helpful error messages with proper SQLSTATE codes!

---

**Last Updated:** 2025-12-09
**Review Status:** Ready for Implementation
