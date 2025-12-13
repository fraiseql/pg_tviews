# Phase 1: Helper Functions - jsonb_array_contains_id & jsonb_extract_id

**Objective**: Add two utility functions from jsonb_ivm for cleaner code and faster operations

**Duration**: 1-2 hours

**Difficulty**: LOW

**Dependencies**: None (first phase)

---

## Context

The jsonb_ivm extension provides two helper functions that will improve pg_tviews:

1. **`jsonb_array_contains_id()`**: Fast existence checking (~10× faster than jsonb_path_query)
2. **`jsonb_extract_id()`**: Clean ID extraction (~5× faster than data->>'id' with type conversion)

These are simple wrappers that provide immediate value with minimal risk.

---

## Files to Modify

1. ✏️ **`src/utils.rs`** - Add `extract_jsonb_id()` wrapper
2. ✏️ **`src/refresh/array_ops.rs`** - Add `check_array_element_exists()` and `insert_array_element_safe()`
3. 📝 **`test/sql/92-helper-functions.sql`** - New test file

---

## Implementation Steps

### Step 0: Understand Validation Infrastructure

Before implementing Phase 1, understand the validation helpers:

**Location**: `src/validation.rs`

Read the validation module documentation to understand:
- `validate_sql_identifier()` - For table/column names
- `validate_jsonb_path()` - For JSONB paths
- When to use each validator
- Error types returned

These validators are used in ALL functions to prevent SQL injection.

### Step 1: Add jsonb_extract_id() Wrapper to utils.rs

**Location**: `src/utils.rs` (append to end of file)

**Code to Add**:

```rust
/// Extract ID field from JSONB data using jsonb_ivm extension.
///
/// **Security**: This function validates the id_key parameter to prevent SQL injection.
/// Only alphanumeric characters and underscores are allowed in id_key.
///
/// # Arguments
///
/// * `data` - JSONB data to extract ID from
/// * `id_key` - Key name for ID field (must be valid identifier: [a-zA-Z0-9_]+)
///
/// # Returns
///
/// ID value as string, or None if not found
///
/// # Errors
///
/// Returns `TViewError` if:
/// - `id_key` contains invalid characters (security)
/// - Database query fails
///
/// # Performance
///
/// - With jsonb_ivm: ~5× faster than data->>'id'
/// - Without jsonb_ivm: Same as data->>'id'
///
/// # Example
///
/// ```rust
/// let data = JsonB(json!({"id": "user_123", "name": "Alice"}));
/// let id = extract_jsonb_id(&data, "id")?;
/// assert_eq!(id, Some("user_123".to_string()));
/// ```
pub fn extract_jsonb_id(data: &JsonB, id_key: &str) -> spi::Result<Option<String>> {
    // Validate id_key to prevent SQL injection
    if !id_key.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(spi::Error::from(crate::TViewError::SpiError {
            query: String::new(),
            error: format!("Invalid identifier in id_key: '{}'. Only alphanumeric and underscore allowed.", id_key),
        }));
    }

    // Check if jsonb_ivm is available
    let has_jsonb_ivm = Spi::get_one::<bool>(
        "SELECT EXISTS(SELECT 1 FROM pg_proc WHERE proname = 'jsonb_extract_id')"
    )?.unwrap_or(false);

    if has_jsonb_ivm {
        // Use optimized jsonb_ivm function with parameterized id_key
        let sql = "SELECT jsonb_extract_id($1::jsonb, $2::text)";
        Spi::get_one_with_args::<String>(
            sql,
            vec![
                unsafe { DatumWithOid::new(data.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(id_key.to_string(), PgOid::BuiltIn(PgBuiltInOids::TEXTOID).value()) },
            ],
        )
    } else {
        // Fallback to standard operator (validated id_key is safe to interpolate)
        // Note: We still prefer parameterized but PostgreSQL doesn't support
        // parameterized identifiers in ->> operator, so we use validated string
        let sql = format!("SELECT $1::jsonb->>'{}'", id_key);
        Spi::get_one_with_args::<String>(
            &sql,
            vec![unsafe { DatumWithOid::new(data.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) }],
        )
    }
}

// ✅ Tests at module level (outside function)
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod helper_tests {
    use super::*;

    #[pg_test]
    fn test_extract_jsonb_id_basic() {
        let data = JsonB(serde_json::json!({
            "id": "user_123",
            "name": "Alice"
        }));

        let id = extract_jsonb_id(&data, "id").unwrap();
        assert_eq!(id, Some("user_123".to_string()));
    }

    #[pg_test]
    fn test_extract_jsonb_id_custom_key() {
        let data = JsonB(serde_json::json!({
            "uuid": "abc-def-ghi",
            "name": "Bob"
        }));

        let uuid = extract_jsonb_id(&data, "uuid").unwrap();
        assert_eq!(uuid, Some("abc-def-ghi".to_string()));
    }

    #[pg_test]
    fn test_extract_jsonb_id_missing() {
        let data = JsonB(serde_json::json!({
            "name": "Charlie"
        }));

        let id = extract_jsonb_id(&data, "id").unwrap();
        assert_eq!(id, None);
    }

    #[pg_test]
    #[should_panic(expected = "Invalid identifier")]
    fn test_extract_jsonb_id_sql_injection() {
        let data = JsonB(serde_json::json!({"id": "test"}));

        // Should reject malicious input
        let _ = extract_jsonb_id(&data, "id'); DROP TABLE users; --").unwrap();
    }
}
```

**Imports to Add** (at top of `src/utils.rs` if not already present):

```rust
use pgrx::JsonB;
use pgrx::datum::DatumWithOid;
```

---

### Step 2: Add Array Existence Check to array_ops.rs

**Location**: `src/refresh/array_ops.rs` (after `delete_array_element()` function, around line 156)

**Code to Add**:

```rust
/// Validate that a string is a safe PostgreSQL identifier.
///
/// **Security**: Prevents SQL injection by ensuring only valid identifier characters.
///
/// # Arguments
///
/// * `s` - String to validate
///
/// # Returns
///
/// `Ok(())` if valid, `Err` with descriptive message if invalid
fn validate_identifier(s: &str) -> TViewResult<()> {
    if s.is_empty() {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: "Identifier cannot be empty".to_string(),
        });
    }

    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(TViewError::SpiError {
            query: String::new(),
            error: format!(
                "Invalid identifier '{}'. Only alphanumeric characters and underscore allowed.",
                s
            ),
        });
    }

    Ok(())
}

/// Check if an array element with the given ID exists.
///
/// This function uses jsonb_ivm's optimized existence check when available,
/// providing ~10× performance improvement over jsonb_path_query.
///
/// **Security**: Validates all identifier parameters to prevent SQL injection.
///
/// # Arguments
///
/// * `data` - JSONB data containing the array
/// * `array_path` - Path to the array (e.g., ["comments"])
/// * `id_key` - Key to match (e.g., "id")
/// * `id_value` - Value to search for
///
/// # Returns
///
/// `true` if element exists, `false` otherwise
///
/// # Errors
///
/// Returns error if identifiers contain invalid characters or query fails.
///
/// # Performance
///
/// - With jsonb_ivm: ~10× faster than jsonb_path_query
/// - Without jsonb_ivm: Falls back to jsonb_path_query
///
/// # Example
///
/// ```rust
/// let data = JsonB(json!({
///     "comments": [
///         {"id": 1, "text": "Hello"},
///         {"id": 2, "text": "World"}
///     ]
/// }));
///
/// let exists = check_array_element_exists(
///     &data,
///     &["comments".to_string()],
///     "id",
///     &JsonB(json!(2))
/// )?;
/// assert!(exists);
/// ```
pub fn check_array_element_exists(
    data: &JsonB,
    array_path: &[String],
    id_key: &str,
    id_value: &JsonB,
) -> TViewResult<bool> {
    // Validate all identifiers to prevent SQL injection
    for segment in array_path {
        validate_identifier(segment)?;
    }
    validate_identifier(id_key)?;

    // Check if jsonb_ivm is available
    let has_jsonb_ivm = check_array_functions_available()?;

    if has_jsonb_ivm {
        // Use optimized jsonb_ivm function
        // Now safe to use in format! after validation
        let path_str = array_path.join("','");
        let sql = format!(
            "SELECT jsonb_array_contains_id($1::jsonb, ARRAY['{}'], '{}', $2::jsonb)",
            path_str, id_key
        );

        Spi::get_one_with_args::<bool>(
            &sql,
            vec![
                unsafe { DatumWithOid::new(data.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(id_value.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            ],
        )
        .map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
    } else {
        // Fallback to jsonb_path_query with correct syntax
        let path = array_path.join(".");
        // ✅ FIXED: Use [*] instead of **
        let sql = format!(
            "SELECT EXISTS(
                SELECT 1 FROM jsonb_path_query($1::jsonb, '$.{}[*] ? (@.{} == $2)')
            )",
            path, id_key
        );

        Spi::get_one_with_args::<bool>(
            &sql,
            vec![
                unsafe { DatumWithOid::new(data.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
                unsafe { DatumWithOid::new(id_value.clone(), PgOid::BuiltIn(PgBuiltInOids::JSONBOID).value()) },
            ],
        )
        .map_err(|e| TViewError::SpiError {
            query: sql,
            error: e.to_string(),
        })
        .map(|opt| opt.unwrap_or(false))
    }
}

/// Insert array element only if it doesn't already exist.
///
/// This prevents duplicate entries in arrays by checking existence first
/// using the fast jsonb_array_contains_id() function.
///
/// # Arguments
///
/// Same as `insert_array_element()` plus:
/// * `id_key` - Key to check for duplicates (e.g., "id")
/// * `id_value` - ID value to check for duplicates
///
/// # Returns
///
/// - `Ok(true)` if element was inserted
/// - `Ok(false)` if element already exists (no insert)
/// - `Err` if operation failed
///
/// # Example
///
/// ```rust
/// // Will only insert if comment with id=123 doesn't exist
/// let inserted = insert_array_element_safe(
///     "tv_post",
///     "pk_post",
///     1,
///     &["comments".to_string()],
///     JsonB(json!({"id": 123, "text": "Hello"})),
///     None,
///     "id",
///     &JsonB(json!(123))
/// )?;
/// ```
pub fn insert_array_element_safe(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &[String],
    new_element: JsonB,
    sort_key: Option<String>,
    id_key: &str,
    id_value: &JsonB,
) -> TViewResult<bool> {
    // First, get current data
    let sql = format!("SELECT data FROM {} WHERE {} = $1", table_name, pk_column);
    let current_data = Spi::get_one_with_args::<JsonB>(
        &sql,
        vec![unsafe { DatumWithOid::new(pk_value, PgOid::BuiltIn(PgBuiltInOids::INT8OID).value()) }],
    )
    .map_err(|e| TViewError::SpiError {
        query: sql.clone(),
        error: e.to_string(),
    })?;

    let current_data = match current_data {
        Some(data) => data,
        None => {
            return Err(TViewError::SpiError {
                query: sql,
                error: format!("No row found with {} = {}", pk_column, pk_value),
            });
        }
    };

    // Check if element already exists
    let exists = check_array_element_exists(&current_data, array_path, id_key, id_value)?;

    if exists {
        // Element already exists, skip insert
        debug!(
            "Array element with {}={:?} already exists in {}.{}, skipping insert",
            id_key, id_value, table_name, array_path.join(".")
        );
        return Ok(false);
    }

    // Element doesn't exist, perform insert
    insert_array_element(table_name, pk_column, pk_value, array_path, new_element, sort_key)?;
    Ok(true)
}
```

---

### Step 3: Add Integration Tests

**Create New File**: `test/sql/92-helper-functions.sql`

**Content**:

```sql
-- Test jsonb_extract_id() and jsonb_array_contains_id() wrappers
\set ECHO none
\set QUIET 1

SET client_min_messages TO WARNING;
SET log_min_messages TO WARNING;

\set ECHO all

-- Setup test schema
CREATE EXTENSION IF NOT EXISTS pg_tviews CASCADE;

-- Test 1: jsonb_extract_id with default 'id' key
\echo '### Test 1: Extract ID from JSONB'
DO $$
DECLARE
    test_data jsonb := '{"id": "user_123", "name": "Alice"}'::jsonb;
    extracted_id text;
BEGIN
    -- This would call the Rust wrapper, but we can test the SQL function directly
    SELECT jsonb_extract_id(test_data, 'id') INTO extracted_id;

    IF extracted_id = 'user_123' THEN
        RAISE NOTICE 'PASS: Extracted ID correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected user_123, got %', extracted_id;
    END IF;
END $$;

-- Test 2: jsonb_extract_id with custom key
\echo '### Test 2: Extract custom key from JSONB'
DO $$
DECLARE
    test_data jsonb := '{"uuid": "abc-def-ghi", "name": "Bob"}'::jsonb;
    extracted_uuid text;
BEGIN
    SELECT jsonb_extract_id(test_data, 'uuid') INTO extracted_uuid;

    IF extracted_uuid = 'abc-def-ghi' THEN
        RAISE NOTICE 'PASS: Extracted UUID correctly';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected abc-def-ghi, got %', extracted_uuid;
    END IF;
END $$;

-- Test 3: jsonb_array_contains_id - element exists
\echo '### Test 3: Check array contains element (exists)'
DO $$
DECLARE
    test_data jsonb := '{
        "comments": [
            {"id": 1, "text": "Hello"},
            {"id": 2, "text": "World"}
        ]
    }'::jsonb;
    element_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(test_data, ARRAY['comments'], 'id', '2'::jsonb)
    INTO element_exists;

    IF element_exists THEN
        RAISE NOTICE 'PASS: Found existing element';
    ELSE
        RAISE EXCEPTION 'FAIL: Should have found element with id=2';
    END IF;
END $$;

-- Test 4: jsonb_array_contains_id - element doesn't exist
\echo '### Test 4: Check array contains element (not exists)'
DO $$
DECLARE
    test_data jsonb := '{
        "comments": [
            {"id": 1, "text": "Hello"},
            {"id": 2, "text": "World"}
        ]
    }'::jsonb;
    element_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(test_data, ARRAY['comments'], 'id', '99'::jsonb)
    INTO element_exists;

    IF NOT element_exists THEN
        RAISE NOTICE 'PASS: Correctly identified missing element';
    ELSE
        RAISE EXCEPTION 'FAIL: Should not have found element with id=99';
    END IF;
END $$;

-- Test 5: Integration test with safe insert
\echo '### Test 5: Safe array insert (prevents duplicates)'
CREATE TABLE test_safe_insert (
    pk_test BIGINT PRIMARY KEY,
    data JSONB DEFAULT '{"items": []}'::jsonb
);

INSERT INTO test_safe_insert VALUES (1, '{"items": []}'::jsonb);

-- First insert should succeed
UPDATE test_safe_insert
SET data = jsonb_array_insert_where(
    data,
    ARRAY['items'],
    '{"id": 1, "name": "Item 1"}'::jsonb,
    NULL, NULL
)
WHERE pk_test = 1;

-- Check it was inserted
DO $$
DECLARE
    item_count int;
BEGIN
    SELECT jsonb_array_length(data->'items') INTO item_count FROM test_safe_insert WHERE pk_test = 1;
    IF item_count = 1 THEN
        RAISE NOTICE 'PASS: First insert succeeded';
    ELSE
        RAISE EXCEPTION 'FAIL: Expected 1 item, got %', item_count;
    END IF;
END $$;

-- Second insert of same ID should be prevented (when using safe wrapper)
DO $$
DECLARE
    already_exists boolean;
BEGIN
    SELECT jsonb_array_contains_id(data, ARRAY['items'], 'id', '1'::jsonb)
    INTO already_exists
    FROM test_safe_insert WHERE pk_test = 1;

    IF already_exists THEN
        RAISE NOTICE 'PASS: Detected duplicate, preventing insert';
    ELSE
        RAISE EXCEPTION 'FAIL: Should have detected existing element';
    END IF;
END $$;

DROP TABLE test_safe_insert;

\echo '### All helper function tests passed! ✓'
```

---

## Verification Steps

### Step 1: Build and Install Extension

```bash
cargo pgrx install --release
```

**Expected Output**: Clean build with no errors

---

### Step 2: Run Rust Unit Tests

```bash
cargo pgrx test
```

**Expected Output**:
```
test helper_tests::test_extract_jsonb_id_basic ... ok
test helper_tests::test_extract_jsonb_id_custom_key ... ok
test helper_tests::test_extract_jsonb_id_missing ... ok
test tests::test_check_array_element_exists ... ok (if added)
test tests::test_insert_array_element_safe ... ok (if added)
```

---

### Step 3: Run SQL Integration Tests

```bash
psql -d postgres -c "DROP DATABASE IF EXISTS test_phase1"
psql -d postgres -c "CREATE DATABASE test_phase1"
psql -d test_phase1 -c "CREATE EXTENSION jsonb_ivm"
psql -d test_phase1 -c "CREATE EXTENSION pg_tviews"
psql -d test_phase1 -f test/sql/92-helper-functions.sql
```

**Expected Output**:
```
### Test 1: Extract ID from JSONB
NOTICE: PASS: Extracted ID correctly

### Test 2: Extract custom key from JSONB
NOTICE: PASS: Extracted UUID correctly

### Test 3: Check array contains element (exists)
NOTICE: PASS: Found existing element

### Test 4: Check array contains element (not exists)
NOTICE: PASS: Correctly identified missing element

### Test 5: Safe array insert (prevents duplicates)
NOTICE: PASS: First insert succeeded
NOTICE: PASS: Detected duplicate, preventing insert

### All helper function tests passed! ✓
```

---

### Step 4: Security Testing

**Critical**: Verify SQL injection protection works correctly:

```bash
# Test SQL injection attempts (should fail with error)
psql -d test_phase1 -c "SELECT extract_jsonb_id('{\"id\": \"test\"}'::jsonb, 'id''; DROP TABLE users; --')"

# Test valid identifiers (should work)
psql -d test_phase1 -c "SELECT extract_jsonb_id('{\"id\": \"test\"}'::jsonb, 'id')"
psql -d test_phase1 -c "SELECT extract_jsonb_id('{\"user_id\": \"test\"}'::jsonb, 'user_id')"
```

**Expected Output**:
```
ERROR:  Invalid identifier in id_key: 'id'); DROP TABLE users; --'. Only alphanumeric and underscore allowed.
```

---

### Step 5: Performance Benchmark (Optional)

Create a quick benchmark to verify performance gains:

```sql
-- Benchmark: jsonb_extract_id vs standard operator
EXPLAIN ANALYZE
SELECT jsonb_extract_id(data, 'id')
FROM (SELECT '{"id": "test_123", "name": "Test"}'::jsonb as data) t;

EXPLAIN ANALYZE
SELECT data->>'id'
FROM (SELECT '{"id": "test_123", "name": "Test"}'::jsonb as data) t;

-- Benchmark: jsonb_array_contains_id vs jsonb_path_query
EXPLAIN ANALYZE
SELECT jsonb_array_contains_id(
    '{"items": [{"id": 1}, {"id": 2}, {"id": 3}]}'::jsonb,
    ARRAY['items'],
    'id',
    '2'::jsonb
);

EXPLAIN ANALYZE
SELECT EXISTS(
    SELECT 1 FROM jsonb_path_query(
        '{"items": [{"id": 1}, {"id": 2}, {"id": 3}]}'::jsonb,
        '$.items[*] ? (@.id == 2)'
    )
);
```

**Expected**: jsonb_ivm functions should be faster (exact timings depend on dataset size)

---

## Acceptance Criteria

- ✅ `extract_jsonb_id()` function added to `src/utils.rs` with SQL injection protection
- ✅ `validate_identifier()` helper function prevents SQL injection
- ✅ `check_array_element_exists()` function added to `src/refresh/array_ops.rs` with validation
- ✅ `insert_array_element_safe()` function added to `src/refresh/array_ops.rs`
- ✅ JSONPath syntax corrected (`[*]` instead of `**`)
- ✅ Test module moved to correct location (module level, not inside function)
- ✅ All Rust unit tests pass including SQL injection test
- ✅ SQL integration tests pass
- ✅ Security testing verifies injection protection works
- ✅ Functions gracefully fallback when jsonb_ivm not installed
- ✅ Documentation comments complete with security notes
- ✅ No clippy warnings
- ✅ Performance improvement verified (optional benchmark)

---

## DO NOT

- ❌ **DO NOT** modify existing function signatures (only add new ones)
- ❌ **DO NOT** remove fallback logic (must work without jsonb_ivm)
- ❌ **DO NOT** skip error handling
- ❌ **DO NOT** use unwrap() in production code
- ❌ **DO NOT** commit without running tests
- ❌ **DO NOT** change behavior of existing insert_array_element() (create new safe variant)
- ❌ **DO NOT** bypass identifier validation (security critical)
- ❌ **DO NOT** use string interpolation for unvalidated identifiers
- ❌ **DO NOT** use incorrect JSONPath syntax (`**` instead of `[*]`)

---

## Troubleshooting

**Problem**: `jsonb_extract_id` function not found
- **Solution**: Ensure jsonb_ivm extension is installed: `CREATE EXTENSION jsonb_ivm;`

**Problem**: Rust compilation errors about JsonB type
- **Solution**: Check imports at top of file include `use pgrx::JsonB;`

**Problem**: Tests fail with "relation does not exist"
- **Solution**: Ensure pg_tviews extension is installed in test database

**Problem**: Performance not improved
- **Solution**: Verify jsonb_ivm is actually being used (check `has_jsonb_ivm` variable in debug output)

---

## Commit Message

```
feat(helpers): Add jsonb_extract_id and jsonb_array_contains_id wrappers [PHASE1]

- Add extract_jsonb_id() utility function for fast ID extraction
- Add check_array_element_exists() for ~10× faster existence checks
- Add insert_array_element_safe() to prevent duplicate array inserts
- Add validate_identifier() for SQL injection protection
- Fix JSONPath syntax: use [*] instead of invalid **
- Comprehensive security testing including SQL injection prevention
- Graceful fallback when jsonb_ivm not installed
- Performance: 5-10× improvement over standard operators

Part of jsonb_ivm enhancement initiative (Phase 1/5)
```

---

## Next Phase

After verification passes, proceed to **Phase 2: Nested Path Array Updates**

See: `phase-2-nested-path-updates.md`
