# Phase 5 Task 3: Dependency Type Detection

**Status:** Ready to implement
**Duration:** 2-3 days
**Parent:** Phase 5 - jsonb_ivm Integration
**TDD Phase:** RED → GREEN → REFACTOR → QA

---

## Objective

Implement dependency type detection logic that analyzes SELECT statements to determine how each foreign key relationship manifests in the JSONB structure (scalar, nested object, or array).

**Success Criteria:**
- ✅ New module `src/schema/analyzer.rs` with dependency detection logic
- ✅ Rust unit tests verify pattern matching for scalar, nested, and array dependencies
- ✅ Integration with `pg_tviews_create()` to populate metadata fields
- ✅ SQL integration test verifies metadata is populated correctly
- ✅ Detection works for common JSONB patterns (jsonb_build_object, jsonb_agg)
- ✅ No breaking changes to existing functionality

---

## Context

**What We Have (From Task 2):**
```rust
pub struct TviewMeta {
    // ... existing fields ...
    pub dependency_types: Vec<DependencyType>,      // ✅ Added in Task 2
    pub dependency_paths: Vec<Option<Vec<String>>>, // ✅ Added in Task 2
    pub array_match_keys: Vec<Option<String>>,      // ✅ Added in Task 2
}
```

**Problem:** These fields are currently empty! We need to populate them by analyzing the SELECT statement.

**How Detection Works:**

When creating a TVIEW with this SELECT:
```sql
SELECT
    p.pk_post,
    p.fk_user,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'author', v_user.data,              -- ← NESTED OBJECT dependency
        'comments', jsonb_agg(v_comment.data ORDER BY created_at) -- ← ARRAY dependency
    ) AS data
FROM tb_post p
LEFT JOIN v_user ON v_user.pk_user = p.fk_user
LEFT JOIN v_comment ON v_comment.fk_post = p.pk_post
GROUP BY p.pk_post, v_user.data;
```

We should detect:
1. **fk_user** → NestedObject at path `["author"]`, match_key = None
2. **fk_comment** → Array at path `["comments"]`, match_key = "id"

**Current SELECT Patterns We Need to Handle:**

| Pattern | Dependency Type | Path | Match Key |
|---------|----------------|------|-----------|
| `'author', v_user.data` | NestedObject | `["author"]` | None |
| `'comments', jsonb_agg(v_comment.data)` | Array | `["comments"]` | `"id"` (convention) |
| Direct column reference (no JOIN) | Scalar | None | None |

---

## RED Phase: Write Failing Tests First

### Test 1: Module Structure and Basic Pattern Detection (Unit Tests)

**File:** `src/schema/analyzer.rs` (NEW)

Create new file with failing unit tests:

```rust
use crate::catalog::DependencyType;

/// Information about a detected dependency
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyInfo {
    /// Type of dependency (Scalar, NestedObject, Array)
    pub dep_type: DependencyType,
    /// JSONB path to the nested data (e.g., ["author"], ["comments"])
    pub jsonb_path: Option<Vec<String>>,
    /// For arrays, the key used to match elements (e.g., "id")
    pub array_match_key: Option<String>,
}

/// Analyze SELECT statement to detect dependency types
///
/// # Arguments
/// * `select_sql` - The SELECT statement defining the TVIEW
/// * `fk_columns` - List of FK column names from schema inference
///
/// # Returns
/// Vector of DependencyInfo, one per FK column (order matches input)
pub fn analyze_dependencies(
    _select_sql: &str,
    _fk_columns: &[String],
) -> Vec<DependencyInfo> {
    // RED: Not implemented yet
    unimplemented!("Task 3 GREEN phase")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_nested_object_simple() {
        let sql = r#"
            SELECT pk_post, fk_user,
                   jsonb_build_object('id', id, 'author', v_user.data) AS data
            FROM tb_post
            LEFT JOIN v_user ON v_user.pk_user = fk_user
        "#;
        let fk_cols = vec!["fk_user".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 1, "Should detect 1 dependency");
        assert_eq!(deps[0].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[0].jsonb_path, Some(vec!["author".to_string()]));
        assert_eq!(deps[0].array_match_key, None);
    }

    #[test]
    fn test_detect_array_simple() {
        let sql = r#"
            SELECT pk_user,
                   jsonb_build_object(
                       'id', id,
                       'posts', jsonb_agg(v_post.data ORDER BY created_at)
                   ) AS data
            FROM tb_user
            LEFT JOIN v_post ON v_post.fk_user = pk_user
            GROUP BY pk_user, id
        "#;
        let fk_cols = vec!["fk_post".to_string()]; // Inferred from v_post reference

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].dep_type, DependencyType::Array);
        assert_eq!(deps[0].jsonb_path, Some(vec!["posts".to_string()]));
        assert_eq!(deps[0].array_match_key, Some("id".to_string())); // Convention
    }

    #[test]
    fn test_detect_scalar_direct_column() {
        let sql = r#"
            SELECT pk_post, jsonb_build_object('id', id, 'title', title) AS data
            FROM tb_post
        "#;
        let fk_cols = vec![]; // No FKs

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 0, "No dependencies for scalar-only TVIEW");
    }

    #[test]
    fn test_detect_multiple_dependencies() {
        let sql = r#"
            SELECT pk_post, fk_user, fk_category,
                   jsonb_build_object(
                       'id', id,
                       'title', title,
                       'author', v_user.data,
                       'category', v_category.data,
                       'comments', jsonb_agg(v_comment.data)
                   ) AS data
            FROM tb_post
            LEFT JOIN v_user ON v_user.pk_user = fk_user
            LEFT JOIN v_category ON v_category.pk_category = fk_category
            LEFT JOIN v_comment ON v_comment.fk_post = pk_post
            GROUP BY pk_post, fk_user, fk_category, v_user.data, v_category.data
        "#;
        let fk_cols = vec!["fk_user".to_string(), "fk_category".to_string(), "fk_comment".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps.len(), 3);

        // fk_user → nested object
        assert_eq!(deps[0].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[0].jsonb_path, Some(vec!["author".to_string()]));

        // fk_category → nested object
        assert_eq!(deps[1].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[1].jsonb_path, Some(vec!["category".to_string()]));

        // fk_comment → array
        assert_eq!(deps[2].dep_type, DependencyType::Array);
        assert_eq!(deps[2].jsonb_path, Some(vec!["comments".to_string()]));
        assert_eq!(deps[2].array_match_key, Some("id".to_string()));
    }

    #[test]
    fn test_detect_no_fk_in_select() {
        // FK exists in schema but isn't referenced in SELECT
        let sql = r#"
            SELECT pk_post, jsonb_build_object('id', id) AS data
            FROM tb_post
        "#;
        let fk_cols = vec!["fk_user".to_string()];

        let deps = analyze_dependencies(sql, &fk_cols);

        // Should still return 1 dependency, but type = Scalar (not used)
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].dep_type, DependencyType::Scalar);
        assert_eq!(deps[0].jsonb_path, None);
    }
}
```

**Expected Result:** All tests FAIL because `analyze_dependencies()` is unimplemented.

**Run Tests:**
```bash
cargo test --lib schema::analyzer
# Expected: 6 failed tests
```

---

### Test 2: SQL Integration Test

**File:** `test/sql/52_dependency_detection.sql` (NEW)

```sql
-- Phase 5 Task 3 RED: Test dependency type detection
-- This test verifies that dependency metadata is populated by analyzing SELECT

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Test Case 1: Nested Object Dependency
    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, id UUID DEFAULT gen_random_uuid(), name TEXT);
    INSERT INTO tb_user VALUES (1, gen_random_uuid(), 'Alice');
    INSERT INTO tb_user VALUES (2, gen_random_uuid(), 'Bob');

    CREATE TABLE tb_post (
        pk_post INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        fk_user INT REFERENCES tb_user(pk_user),
        title TEXT
    );
    INSERT INTO tb_post VALUES (1, gen_random_uuid(), 1, 'First Post');

    -- Create user TVIEW first
    SELECT pg_tviews_create('user', $$
        SELECT pk_user, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_user
    $$);

    -- Create post TVIEW with nested author object
    SELECT pg_tviews_create('post', $$
        SELECT
            p.pk_post,
            p.id,
            p.fk_user,
            u.id AS user_id,
            jsonb_build_object(
                'id', p.id,
                'title', p.title,
                'author', v_user.data
            ) AS data
        FROM tb_post p
        LEFT JOIN tb_user u ON u.pk_user = p.fk_user
        LEFT JOIN v_user ON v_user.pk_user = p.fk_user
    $$);

    -- Verify metadata has dependency_types populated
    SELECT
        entity,
        fk_columns,
        dependency_types,
        dependency_paths,
        array_match_keys
    FROM pg_tview_meta
    WHERE entity = 'post';

    -- Expected output (approximate):
    -- entity | fk_columns  | dependency_types    | dependency_paths      | array_match_keys
    -- post   | {fk_user}   | {nested_object}     | {{author}}            | {NULL}

    -- Test Case 2: Array Dependency
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_comment (
        pk_comment INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        fk_post INT,
        content TEXT
    );

    -- Re-create user and post
    SELECT pg_tviews_create('user', $$
        SELECT pk_user, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_user
    $$);

    SELECT pg_tviews_create('post', $$
        SELECT pk_post, id,
               jsonb_build_object('id', id, 'title', title) AS data
        FROM tb_post
    $$);

    -- Create comment TVIEW
    SELECT pg_tviews_create('comment', $$
        SELECT pk_comment, id, fk_post,
               jsonb_build_object('id', id, 'content', content) AS data
        FROM tb_comment
    $$);

    -- Create post_with_comments TVIEW with array aggregation
    SELECT pg_tviews_create('post_with_comments', $$
        SELECT
            p.pk_post,
            p.id,
            jsonb_build_object(
                'id', p.id,
                'title', p.title,
                'comments', COALESCE(
                    jsonb_agg(v_comment.data ORDER BY v_comment.id),
                    '[]'::jsonb
                )
            ) AS data
        FROM tb_post p
        LEFT JOIN v_comment ON v_comment.fk_post = p.pk_post
        GROUP BY p.pk_post, p.id, p.title
    $$);

    -- Verify array dependency detected
    SELECT
        entity,
        dependency_types,
        dependency_paths,
        array_match_keys
    FROM pg_tview_meta
    WHERE entity = 'post_with_comments';

    -- Expected output:
    -- entity              | dependency_types | dependency_paths | array_match_keys
    -- post_with_comments  | {array}          | {{comments}}     | {id}

ROLLBACK;
```

**Expected Result:** Test will fail or show empty arrays for dependency metadata because detection isn't implemented yet.

**Run Test:**
```bash
cargo pgrx test pg17
# Look for test/sql/52_dependency_detection.sql output
```

---

## GREEN Phase: Make Tests Pass (Minimal Implementation)

### Step 1: Implement Pattern Detection Logic

**File:** `src/schema/analyzer.rs`

Replace `unimplemented!()` with actual logic:

```rust
use crate::catalog::DependencyType;
use regex::Regex;

/// Information about a detected dependency
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyInfo {
    pub dep_type: DependencyType,
    pub jsonb_path: Option<Vec<String>>,
    pub array_match_key: Option<String>,
}

impl DependencyInfo {
    /// Create a scalar dependency (default)
    fn scalar() -> Self {
        Self {
            dep_type: DependencyType::Scalar,
            jsonb_path: None,
            array_match_key: None,
        }
    }

    /// Create a nested object dependency
    fn nested_object(path: String) -> Self {
        Self {
            dep_type: DependencyType::NestedObject,
            jsonb_path: Some(vec![path]),
            array_match_key: None,
        }
    }

    /// Create an array dependency
    fn array(path: String, match_key: String) -> Self {
        Self {
            dep_type: DependencyType::Array,
            jsonb_path: Some(vec![path]),
            array_match_key: Some(match_key),
        }
    }
}

/// Analyze SELECT statement to detect dependency types
pub fn analyze_dependencies(
    select_sql: &str,
    fk_columns: &[String],
) -> Vec<DependencyInfo> {
    let mut deps = Vec::new();

    for fk_col in fk_columns {
        let dep_info = detect_dependency_type(select_sql, fk_col);
        deps.push(dep_info);
    }

    deps
}

/// Detect how a single FK is used in the SELECT statement
fn detect_dependency_type(select_sql: &str, fk_col: &str) -> DependencyInfo {
    // Normalize SQL: remove extra whitespace, make lowercase for pattern matching
    let sql_normalized = select_sql
        .replace('\n', " ")
        .replace('\t', " ")
        .to_lowercase();

    // Infer view name from FK column
    // Convention: fk_user → v_user
    let view_name = if fk_col.starts_with("fk_") {
        format!("v_{}", &fk_col[3..])
    } else {
        return DependencyInfo::scalar(); // Can't infer view name
    };

    // Pattern 1: Nested Object
    // Look for: 'key_name', v_something.data
    // Example: 'author', v_user.data
    let nested_pattern = format!(r"'(\w+)',\s*{}.data", regex::escape(&view_name));
    if let Ok(re) = Regex::new(&nested_pattern) {
        if let Some(captures) = re.captures(&sql_normalized) {
            if let Some(key_match) = captures.get(1) {
                let key_name = key_match.as_str().to_string();
                return DependencyInfo::nested_object(key_name);
            }
        }
    }

    // Pattern 2: Array Aggregation
    // Look for: 'array_name', jsonb_agg(v_something.data ...)
    // Example: 'comments', jsonb_agg(v_comment.data ORDER BY ...)
    let array_pattern = format!(r"'(\w+)',\s*(?:coalesce\s*\()?\s*jsonb_agg\s*\(\s*{}.data", regex::escape(&view_name));
    if let Ok(re) = Regex::new(&array_pattern) {
        if let Some(captures) = re.captures(&sql_normalized) {
            if let Some(key_match) = captures.get(1) {
                let array_name = key_match.as_str().to_string();
                // Convention: arrays use "id" as match key
                return DependencyInfo::array(array_name, "id".to_string());
            }
        }
    }

    // Default: Scalar (FK exists but not used in JSONB composition)
    DependencyInfo::scalar()
}

#[cfg(test)]
mod tests {
    // ... tests from RED phase (should now pass)
}
```

**Verify Tests Pass:**
```bash
cargo test --lib schema::analyzer
# Expected: 6 passed tests
```

---

### Step 2: Export Module and Add to Schema

**File:** `src/schema/mod.rs`

Add the analyzer module:

```rust
pub mod parser;
pub mod inference;
pub mod types;
pub mod analyzer;  // ← NEW

// ... rest of file unchanged
```

---

### Step 3: Update register_metadata() to Call Analyzer

**File:** `src/ddl/create.rs`

Find the `register_metadata()` function and update it to populate dependency metadata:

```rust
use crate::schema::analyzer::analyze_dependencies;

/// Register TVIEW metadata in pg_tview_meta
fn register_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    select_sql: &str,
    schema: &TViewSchema,
    base_tables: &[pg_sys::Oid],
) -> TViewResult<()> {
    // Get OIDs
    let view_oid = get_relation_oid(view_name)?;
    let tview_oid = get_relation_oid(tview_name)?;

    // Analyze dependencies to populate type/path/match_key info
    let dep_infos = analyze_dependencies(select_sql, &schema.fk_columns);

    // Convert DependencyInfo to SQL arrays
    let dep_types_sql = format_text_array(
        &dep_infos.iter()
            .map(|d| d.dep_type.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // Convert paths: Vec<Option<Vec<String>>> → TEXT[][]
    // For now, flatten to TEXT[] (single-level paths only)
    let dep_paths_sql = format_text_array_2d(
        &dep_infos.iter()
            .map(|d| d.jsonb_path.clone())
            .collect::<Vec<_>>()
    );

    // Convert array match keys: Vec<Option<String>> → TEXT[] with NULLs
    let array_keys_sql = format_text_array_nullable(
        &dep_infos.iter()
            .map(|d| d.array_match_key.clone())
            .collect::<Vec<_>>()
    );

    // Build INSERT statement
    let insert_sql = format!(
        "INSERT INTO pg_tview_meta (
            entity, view_oid, table_oid, definition,
            dependencies, fk_columns, uuid_fk_columns,
            dependency_types, dependency_paths, array_match_keys
        ) VALUES (
            '{}',
            {}::oid,
            {}::oid,
            $1,
            $2,
            $3,
            $4,
            {},
            {},
            {}
        )",
        entity_name.replace("'", "''"),
        view_oid.as_u32(),
        tview_oid.as_u32(),
        dep_types_sql,
        dep_paths_sql,
        array_keys_sql
    );

    // Execute insert
    Spi::connect(|client| {
        client.update(
            &insert_sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), select_sql.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::OIDARRAYOID), base_tables.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID), schema.fk_columns.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID), schema.uuid_fk_columns.clone().into_datum()),
            ]),
        )?;
        Ok(())
    }).map_err(|e| TViewError::SpiError {
        query: insert_sql,
        error: e.to_string(),
    })?;

    info!("Registered metadata for TVIEW: {}", entity_name);
    Ok(())
}

/// Format Vec<String> as PostgreSQL TEXT[] literal
fn format_text_array(items: &[String]) -> String {
    if items.is_empty() {
        return "'{}'".to_string();
    }
    let quoted: Vec<String> = items.iter()
        .map(|s| format!("\"{}\"", s.replace("\"", "\\\"")))
        .collect();
    format!("'{{{}}}'", quoted.join(","))
}

/// Format Vec<Option<Vec<String>>> as PostgreSQL TEXT[][] literal
fn format_text_array_2d(items: &[Option<Vec<String>>]) -> String {
    if items.is_empty() {
        return "'{{}}'".to_string();
    }
    let arrays: Vec<String> = items.iter()
        .map(|opt| match opt {
            Some(path) => {
                let quoted: Vec<String> = path.iter()
                    .map(|s| format!("\"{}\"", s.replace("\"", "\\\"")))
                    .collect();
                format!("{{{}}}", quoted.join(","))
            }
            None => "{}".to_string(),
        })
        .collect();
    format!("'{{{}}}'", arrays.join(","))
}

/// Format Vec<Option<String>> as TEXT[] with NULL values
fn format_text_array_nullable(items: &[Option<String>]) -> String {
    if items.is_empty() {
        return "'{}'".to_string();
    }
    let quoted: Vec<String> = items.iter()
        .map(|opt| match opt {
            Some(s) => format!("\"{}\"", s.replace("\"", "\\\"")),
            None => "NULL".to_string(),
        })
        .collect();
    format!("'{{{}}}'", quoted.join(","))
}

/// Get OID of a relation (table or view)
fn get_relation_oid(relation_name: &str) -> TViewResult<pg_sys::Oid> {
    let oid_opt: Option<pg_sys::Oid> = Spi::get_one(&format!(
        "SELECT '{}'::regclass::oid",
        relation_name.replace("'", "''")
    ))
    .map_err(|e| TViewError::SpiError {
        query: format!("Get OID for {}", relation_name),
        error: e.to_string(),
    })?;

    oid_opt.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for {}", relation_name),
        pg_error: "Relation not found".to_string(),
    })
}
```

**Add Cargo Dependency:**

**File:** `Cargo.toml`

```toml
[dependencies]
# ... existing dependencies ...
regex = "1.10"  # ← Add this
```

**Verify Compilation:**
```bash
cargo build --lib
# Expected: Compiles successfully
```

---

### Step 4: Run SQL Integration Test

**Run Test:**
```bash
cargo pgrx test pg17
# Look for test/sql/52_dependency_detection.sql output
# Expected: metadata rows should now have populated dependency_types, dependency_paths, array_match_keys
```

---

## REFACTOR Phase: Improve Code Quality

### Refactor 1: Add Comprehensive Documentation

**File:** `src/schema/analyzer.rs`

Add module-level documentation:

```rust
//! Dependency type detection for jsonb_ivm optimization
//!
//! This module analyzes TVIEW SELECT statements to determine how foreign key
//! relationships manifest in the JSONB structure. This information is used
//! to choose the appropriate jsonb_ivm patch function for efficient updates.
//!
//! # Detection Patterns
//!
//! ## Nested Object
//! ```sql
//! jsonb_build_object('author', v_user.data)
//! ```
//! → `dependency_type = 'nested_object'`, `path = ['author']`
//!
//! ## Array Aggregation
//! ```sql
//! jsonb_build_object('comments', jsonb_agg(v_comment.data))
//! ```
//! → `dependency_type = 'array'`, `path = ['comments']`, `match_key = 'id'`
//!
//! ## Scalar (Default)
//! FK column exists but not used in JSONB composition
//! → `dependency_type = 'scalar'`, `path = NULL`

use crate::catalog::DependencyType;
use regex::Regex;

// ... rest of implementation
```

### Refactor 2: Extract Pattern Constants

**File:** `src/schema/analyzer.rs`

Make patterns more maintainable:

```rust
/// Regex pattern for nested object detection
/// Matches: 'key_name', v_something.data
const NESTED_PATTERN_TEMPLATE: &str = r"'(\w+)',\s*{}.data";

/// Regex pattern for array aggregation detection
/// Matches: 'array_name', jsonb_agg(v_something.data ...)
/// Also handles COALESCE wrapper: COALESCE(jsonb_agg(...), '[]'::jsonb)
const ARRAY_PATTERN_TEMPLATE: &str = r"'(\w+)',\s*(?:coalesce\s*\()?\s*jsonb_agg\s*\(\s*{}.data";

/// Default match key for array dependencies
const DEFAULT_ARRAY_MATCH_KEY: &str = "id";

fn detect_dependency_type(select_sql: &str, fk_col: &str) -> DependencyInfo {
    // ... use constants instead of inline strings
    let nested_pattern = NESTED_PATTERN_TEMPLATE.replace("{}", &regex::escape(&view_name));
    let array_pattern = ARRAY_PATTERN_TEMPLATE.replace("{}", &regex::escape(&view_name));
    // ... rest of implementation
}
```

### Refactor 3: Add Error Handling for Edge Cases

**File:** `src/schema/analyzer.rs`

Handle malformed FK columns gracefully:

```rust
/// Infer TVIEW name from FK column name
///
/// Conventions:
/// - `fk_user` → `v_user`
/// - `fk_blog_post` → `v_blog_post`
fn infer_view_name(fk_col: &str) -> Option<String> {
    if !fk_col.starts_with("fk_") {
        return None;
    }

    let entity = &fk_col[3..];
    if entity.is_empty() {
        return None;
    }

    Some(format!("v_{}", entity))
}

fn detect_dependency_type(select_sql: &str, fk_col: &str) -> DependencyInfo {
    let sql_normalized = select_sql
        .replace('\n', " ")
        .replace('\t', " ")
        .to_lowercase();

    // Try to infer view name from FK column
    let view_name = match infer_view_name(fk_col) {
        Some(name) => name,
        None => {
            // Can't infer view name → assume scalar
            return DependencyInfo::scalar();
        }
    };

    // ... rest of pattern matching
}
```

### Refactor 4: Add Logging for Debugging

**File:** `src/schema/analyzer.rs`

Help developers understand what's detected:

```rust
use pgrx::prelude::*; // For info!, warning! macros

pub fn analyze_dependencies(
    select_sql: &str,
    fk_columns: &[String],
) -> Vec<DependencyInfo> {
    info!("Analyzing {} FK dependencies in SELECT", fk_columns.len());

    let mut deps = Vec::new();

    for fk_col in fk_columns {
        let dep_info = detect_dependency_type(select_sql, fk_col);

        info!(
            "FK '{}' detected as {:?} at path {:?}",
            fk_col,
            dep_info.dep_type,
            dep_info.jsonb_path
        );

        deps.push(dep_info);
    }

    deps
}
```

**Verify Refactoring:**
```bash
cargo test --lib schema::analyzer
cargo build --lib
# Expected: All tests still pass, code compiles
```

---

## QA Phase: Integration Testing and Validation

### QA Test 1: Complex Multi-Dependency TVIEW

**File:** `test/sql/53_dependency_detection_complex.sql` (NEW)

```sql
-- Phase 5 Task 3 QA: Complex dependency detection scenarios

BEGIN;
    SET client_min_messages TO WARNING;

    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Create a complex schema: Company → Department → Employee → Task
    CREATE TABLE tb_company (
        pk_company INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        name TEXT
    );

    CREATE TABLE tb_department (
        pk_department INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        fk_company INT REFERENCES tb_company(pk_company),
        name TEXT
    );

    CREATE TABLE tb_employee (
        pk_employee INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        fk_department INT REFERENCES tb_department(pk_department),
        name TEXT,
        email TEXT
    );

    CREATE TABLE tb_task (
        pk_task INT PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid(),
        fk_employee INT REFERENCES tb_employee(pk_employee),
        title TEXT,
        status TEXT
    );

    -- Insert test data
    INSERT INTO tb_company VALUES (1, gen_random_uuid(), 'ACME Corp');
    INSERT INTO tb_department VALUES (1, gen_random_uuid(), 1, 'Engineering');
    INSERT INTO tb_employee VALUES (1, gen_random_uuid(), 1, 'Alice', 'alice@acme.com');
    INSERT INTO tb_employee VALUES (2, gen_random_uuid(), 1, 'Bob', 'bob@acme.com');
    INSERT INTO tb_task VALUES (1, gen_random_uuid(), 1, 'Build Feature X', 'in_progress');
    INSERT INTO tb_task VALUES (2, gen_random_uuid(), 1, 'Fix Bug Y', 'done');
    INSERT INTO tb_task VALUES (3, gen_random_uuid(), 2, 'Write Tests', 'todo');

    -- Create TVIEWs
    SELECT pg_tviews_create('company', $$
        SELECT pk_company, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_company
    $$);

    SELECT pg_tviews_create('department', $$
        SELECT
            d.pk_department,
            d.id,
            d.fk_company,
            c.id AS company_id,
            jsonb_build_object(
                'id', d.id,
                'name', d.name,
                'company', v_company.data
            ) AS data
        FROM tb_department d
        LEFT JOIN tb_company c ON c.pk_company = d.fk_company
        LEFT JOIN v_company ON v_company.pk_company = d.fk_company
    $$);

    SELECT pg_tviews_create('employee', $$
        SELECT
            e.pk_employee,
            e.id,
            e.fk_department,
            d.id AS department_id,
            jsonb_build_object(
                'id', e.id,
                'name', e.name,
                'email', e.email,
                'department', v_department.data
            ) AS data
        FROM tb_employee e
        LEFT JOIN tb_department d ON d.pk_department = e.fk_department
        LEFT JOIN v_department ON v_department.pk_department = e.fk_department
    $$);

    SELECT pg_tviews_create('task', $$
        SELECT
            t.pk_task,
            t.id,
            t.fk_employee,
            e.id AS employee_id,
            jsonb_build_object(
                'id', t.id,
                'title', t.title,
                'status', t.status,
                'assignee', v_employee.data
            ) AS data
        FROM tb_task t
        LEFT JOIN tb_employee e ON e.pk_employee = t.fk_employee
        LEFT JOIN v_employee ON v_employee.pk_employee = t.fk_employee
    $$);

    -- Create aggregated view: Employee with all their tasks
    SELECT pg_tviews_create('employee_with_tasks', $$
        SELECT
            e.pk_employee,
            e.id,
            e.fk_department,
            d.id AS department_id,
            jsonb_build_object(
                'id', e.id,
                'name', e.name,
                'email', e.email,
                'department', v_department.data,
                'tasks', COALESCE(
                    jsonb_agg(v_task.data ORDER BY v_task.id),
                    '[]'::jsonb
                )
            ) AS data
        FROM tb_employee e
        LEFT JOIN tb_department d ON d.pk_department = e.fk_department
        LEFT JOIN v_department ON v_department.pk_department = e.fk_department
        LEFT JOIN v_task ON v_task.fk_employee = e.pk_employee
        GROUP BY e.pk_employee, e.id, e.name, e.email, e.fk_department, d.id, v_department.data
    $$);

    -- Verify metadata for employee_with_tasks
    SELECT
        entity,
        fk_columns,
        dependency_types,
        dependency_paths,
        array_match_keys
    FROM pg_tview_meta
    WHERE entity = 'employee_with_tasks';

    -- Expected:
    -- entity               | fk_columns              | dependency_types                | dependency_paths           | array_match_keys
    -- employee_with_tasks  | {fk_department,fk_task} | {nested_object,array}           | {{department},{tasks}}     | {NULL,id}

    -- Verify cascade works correctly with detected metadata
    UPDATE tb_department SET name = 'Product Engineering' WHERE pk_department = 1;

    -- Check that department name updated in nested object
    SELECT data->'department'->>'name' AS dept_name
    FROM tv_employee_with_tasks
    WHERE pk_employee = 1;
    -- Expected: 'Product Engineering'

    -- Update a task status
    UPDATE tb_task SET status = 'completed' WHERE pk_task = 1;

    -- Check that task status updated in array
    SELECT
        data->>'name' AS employee_name,
        jsonb_array_length(data->'tasks') AS task_count,
        (data->'tasks'->0->>'status') AS first_task_status
    FROM tv_employee_with_tasks
    WHERE pk_employee = 1;
    -- Expected: 'Alice', 2, 'completed'

ROLLBACK;
```

**Expected Result:** All metadata fields populated correctly, cascade updates work as expected.

**Run Test:**
```bash
cargo pgrx test pg17
# Look for test/sql/53_dependency_detection_complex.sql output
```

---

### QA Test 2: Edge Cases and Error Handling

**File:** `test/sql/54_dependency_detection_edge_cases.sql` (NEW)

```sql
-- Phase 5 Task 3 QA: Edge cases for dependency detection

BEGIN;
    SET client_min_messages TO WARNING;

    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_entity (pk_entity INT PRIMARY KEY, id UUID, data TEXT);

    -- Edge Case 1: No FKs at all
    SELECT pg_tviews_create('entity', $$
        SELECT pk_entity, id,
               jsonb_build_object('id', id, 'data', data) AS data
        FROM tb_entity
    $$);

    SELECT fk_columns, dependency_types FROM pg_tview_meta WHERE entity = 'entity';
    -- Expected: {}, {}

    -- Edge Case 2: FK exists but not used in SELECT
    CREATE TABLE tb_parent (pk_parent INT PRIMARY KEY, id UUID, name TEXT);
    CREATE TABLE tb_child (
        pk_child INT PRIMARY KEY,
        id UUID,
        fk_parent INT REFERENCES tb_parent(pk_parent),
        value TEXT
    );

    SELECT pg_tviews_create('child_no_join', $$
        SELECT pk_child, id, fk_parent,
               jsonb_build_object('id', id, 'value', value) AS data
        FROM tb_child
    $$);

    SELECT fk_columns, dependency_types FROM pg_tview_meta WHERE entity = 'child_no_join';
    -- Expected: {fk_parent}, {scalar} (FK detected but not used)

    -- Edge Case 3: Multiple identical FK references (should handle gracefully)
    SELECT pg_tviews_create('parent', $$
        SELECT pk_parent, id,
               jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_parent
    $$);

    SELECT pg_tviews_create('child_with_parent', $$
        SELECT
            c.pk_child,
            c.id,
            c.fk_parent,
            p.id AS parent_id,
            jsonb_build_object(
                'id', c.id,
                'value', c.value,
                'parent', v_parent.data
            ) AS data
        FROM tb_child c
        LEFT JOIN tb_parent p ON p.pk_parent = c.fk_parent
        LEFT JOIN v_parent ON v_parent.pk_parent = c.fk_parent
    $$);

    SELECT dependency_types, dependency_paths FROM pg_tview_meta WHERE entity = 'child_with_parent';
    -- Expected: {nested_object}, {{parent}}

ROLLBACK;
```

**Expected Result:** All edge cases handled gracefully without errors.

---

## Acceptance Criteria

### Functional Requirements
- [x] `src/schema/analyzer.rs` module created with pattern detection logic
- [x] Unit tests pass for nested object, array, and scalar detection
- [x] `analyze_dependencies()` integrates with `pg_tviews_create()`
- [x] Metadata rows have populated `dependency_types`, `dependency_paths`, `array_match_keys`
- [x] Detection works for common patterns: `v_*.data` and `jsonb_agg(v_*.data)`
- [x] No breaking changes to existing TVIEW creation

### Quality Requirements
- [x] Comprehensive Rustdoc comments explaining detection patterns
- [x] Unit test coverage for all detection scenarios
- [x] SQL integration tests verify end-to-end metadata population
- [x] QA tests cover complex multi-dependency and edge cases
- [x] Code compiles without warnings
- [x] All existing tests still pass

### Performance Requirements
- [x] Pattern detection adds < 10ms overhead to TVIEW creation
- [x] Regex patterns compile once and cached (if needed)
- [x] No significant memory increase

---

## Files to Create/Modify

### New Files
1. **`src/schema/analyzer.rs`** - Dependency detection logic
2. **`test/sql/52_dependency_detection.sql`** - Basic integration test
3. **`test/sql/53_dependency_detection_complex.sql`** - QA complex scenarios
4. **`test/sql/54_dependency_detection_edge_cases.sql`** - QA edge cases

### Modified Files
1. **`src/schema/mod.rs`** - Export analyzer module
2. **`src/ddl/create.rs`** - Call `analyze_dependencies()` in `register_metadata()`
3. **`Cargo.toml`** - Add `regex` dependency
4. **`.phases/README.md`** - Update Task 3 status to complete

---

## DO NOT

- ❌ Do NOT add complex SQL parsing (just pattern matching)
- ❌ Do NOT handle multi-level nested paths yet (Task 4 enhancement)
- ❌ Do NOT detect array match keys dynamically (use "id" convention)
- ❌ Do NOT break existing TVIEWs without metadata
- ❌ Do NOT modify Phase 4 refresh logic yet (that's Task 4)

---

## Verification Commands

### After RED Phase
```bash
# Should see failing tests
cargo test --lib schema::analyzer 2>&1 | grep "test result"
# Expected: FAILED (6 failed)
```

### After GREEN Phase
```bash
# Should compile and pass unit tests
cargo build --lib
cargo test --lib schema::analyzer
# Expected: 6 passed

# Should pass integration test
cargo pgrx test pg17
# Check output of test/sql/52_dependency_detection.sql
```

### After REFACTOR Phase
```bash
# Should still pass with improved code
cargo test --lib
cargo build --lib
cargo clippy -- -D warnings
```

### After QA Phase
```bash
# All tests should pass
cargo pgrx test pg17
# Check outputs of all 3 SQL tests:
# - 52_dependency_detection.sql
# - 53_dependency_detection_complex.sql
# - 54_dependency_detection_edge_cases.sql
```

---

## Success Metrics

**Phase 5 Task 3 is complete when:**

✅ All 6 unit tests in `src/schema/analyzer.rs` pass
✅ `analyze_dependencies()` correctly detects nested objects, arrays, and scalars
✅ Metadata rows have populated `dependency_types`, `dependency_paths`, `array_match_keys`
✅ SQL integration tests verify end-to-end detection
✅ QA tests cover complex scenarios and edge cases
✅ No regressions in existing functionality
✅ Code is well-documented and maintainable

---

## Next Steps After Task 3

**Task 4:** Update `apply_patch()` to use jsonb_ivm smart functions based on detected dependency types
**Task 5:** Pass changed FK context to enable surgical updates
**Task 6:** Performance benchmarking to verify 1.5-3× speedup

---

## Timeline Estimate

| Phase | Duration | Deliverable |
|-------|----------|-------------|
| PLAN | 1-2 hours | This document |
| RED | 2-3 hours | Failing tests written |
| GREEN | 4-6 hours | Basic implementation passes tests |
| REFACTOR | 2-3 hours | Improved code quality |
| QA | 2-3 hours | Complex scenarios tested |

**Total: 2-3 days**

---

## References

- Phase 5 Overall Plan: `.phases/phase-5-jsonb-ivm-integration.md`
- Task 2 Plan (Metadata Enhancement): `.phases/phase-5-task-2-metadata-enhancement.md`
- TviewMeta Structure: `src/catalog.rs:40-71`
- TVIEW Creation Flow: `src/ddl/create.rs:19-94`
- Schema Inference: `src/schema/inference.rs`
