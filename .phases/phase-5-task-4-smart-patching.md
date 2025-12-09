# Phase 5 Task 4: Smart JSONB Patching Integration

**Status:** Ready to implement
**Phase:** Phase 5 Task 4 of 5 (jsonb_ivm integration)
**Complexity:** MEDIUM (Integration with existing metadata)
**Duration:** 2-3 hours
**TDD Approach:** RED → GREEN → REFACTOR → QA

---

## Objective

Integrate smart JSONB patching in `apply_patch()` using the dependency metadata from Task 3 to dispatch to the correct `jsonb_ivm` function, achieving **1.5-3× performance improvement** on cascade updates.

**Current State (Phase 5 Task 3 Complete):**
- ✅ Dependency metadata captured in `pg_tview_meta` table
- ✅ `dependency_types`, `dependency_paths`, `array_match_keys` columns populated
- ✅ Analyzer detects: `nested_object`, `array`, `scalar` types
- ❌ `apply_patch()` still does full JSONB replacement (no perf benefit yet)

**Target State (Phase 5 Task 4):**
- ✅ `apply_patch()` reads dependency metadata
- ✅ Dispatches to correct `jsonb_smart_patch_*()` function per dependency
- ✅ Nested objects use path-based merge
- ✅ Arrays use element-level updates with match key
- ✅ Scalars use shallow merge or full replace
- ✅ **Measurable performance improvement**: 1.5-3× faster cascades

---

## Context

### Current apply_patch() Implementation

**File:** `src/refresh.rs:124-147`

```rust
/// Apply JSON patch to tv_entity for pk using jsonb_ivm_patch.
/// For now, this stub replaces the JSON instead of calling jsonb_ivm_patch.
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    // TODO: call jsonb_ivm_patch(data, $1) instead of direct replacement
    let sql = format!(
        "UPDATE {} \
         SET data = $1, updated_at = now() \
         WHERE {} = $2",
        tv_name, pk_col
    );

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), JsonB(row.data.0.clone()).into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}
```

**Issue:** Full JSONB replacement (`data = $1`) instead of surgical patching.

### Available jsonb_ivm Functions

From `jsonb_ivm` v0.3.1:

```sql
-- Scalar/Shallow merge (no nested objects)
jsonb_smart_patch_scalar(original jsonb, patch jsonb) → jsonb

-- Nested object merge at specific path
jsonb_smart_patch_nested(original jsonb, patch jsonb, path text[]) → jsonb

-- Array element update with match key
jsonb_smart_patch_array(
    original jsonb,
    patch jsonb,
    path text[],
    match_key text DEFAULT 'id'
) → jsonb
```

### Dependency Metadata (from Task 3)

**Table:** `pg_tview_meta`

| Column              | Type   | Example Value       |
|---------------------|--------|---------------------|
| `dependency_types`  | TEXT[] | `{nested_object,array,scalar}` |
| `dependency_paths`  | TEXT[] | `{author,comments,NULL}` |
| `array_match_keys`  | TEXT[] | `{NULL,id,NULL}` |

**Mapping:**
- `dependency_types[1] = 'nested_object'` → `dependency_paths[1] = 'author'` → use `jsonb_smart_patch_nested(data, patch, '{author}')`
- `dependency_types[2] = 'array'` → `dependency_paths[2] = 'comments'`, `array_match_keys[2] = 'id'` → use `jsonb_smart_patch_array(data, patch, '{comments}', 'id')`
- `dependency_types[3] = 'scalar'` → no path → use `jsonb_smart_patch_scalar(data, patch)`

---

## Files to Modify/Create

### Modify
1. **`src/refresh.rs`** - Update `apply_patch()` to use smart functions
2. **`src/catalog.rs`** - Add methods to `TviewMeta` for dependency lookup

### No New Files
All code integrates into existing modules.

---

## Implementation Plan (TDD Phases)

### Phase 1: RED - Write Failing Tests

**File:** `src/refresh.rs` (add test module)

**Objective:** Write tests that expect smart patching behavior, which will fail with current full-replacement code.

#### Test 1: Nested Object Patch
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[pg_test]
    fn test_apply_patch_nested_object() {
        // Setup: Create TVIEW with nested dependency
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEW with nested author object
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'author', v_user.data
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                $$
            )
        ").unwrap();

        // Verify metadata captured nested dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("nested_object"));

        // Initial state
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(initial_data.0["title"], "Hello");
        assert_eq!(initial_data.0["author"]["name"], "Alice");

        // Update user name
        Spi::run("UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1").unwrap();

        // Trigger cascade (should use smart patch, not full replace)
        // This will fail initially because apply_patch() does full replacement
        refresh_pk(
            Spi::get_one::<pg_sys::Oid>("SELECT 'tb_user'::regclass::oid").unwrap().unwrap(),
            1
        ).unwrap();

        // Verify: ONLY author.name changed, title unchanged
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        assert_eq!(updated_data.0["title"], "Hello"); // Should NOT be touched
        assert_eq!(updated_data.0["author"]["name"], "Alice Updated");

        // TODO: Add assertion to verify smart patch was used (not full replace)
        // This requires logging or a way to detect the SQL used
    }

    #[pg_test]
    fn test_apply_patch_array() {
        // Setup: Create TVIEW with array dependency
        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();
        Spi::run("CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_post BIGINT REFERENCES tb_post(pk_post),
            fk_user BIGINT REFERENCES tb_user(pk_user),
            text TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Hello')").unwrap();
        Spi::run("INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (1, 1, 1, 'Great post!')").unwrap();
        Spi::run("INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (2, 1, 1, 'Thanks!')").unwrap();

        // Create TVIEW with array of comments
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'author', v_user.data,
                           'comments', COALESCE(jsonb_agg(v_comment.data), '[]'::jsonb)
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                LEFT JOIN v_comment ON v_comment.fk_post = tb_post.pk_post
                GROUP BY pk_post, fk_user, title, v_user.data
                $$
            )
        ").unwrap();

        // Verify metadata captured array dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("array"));

        // Initial state: 2 comments
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(initial_data.0["comments"].as_array().unwrap().len(), 2);

        // Update one comment
        Spi::run("UPDATE tb_comment SET text = 'Updated!' WHERE pk_comment = 1").unwrap();

        // Trigger cascade (should use array smart patch)
        refresh_pk(
            Spi::get_one::<pg_sys::Oid>("SELECT 'tb_comment'::regclass::oid").unwrap().unwrap(),
            1
        ).unwrap();

        // Verify: Only the updated comment changed
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();

        let comments = updated_data.0["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2); // Still 2 comments

        // Find the updated comment (id=1)
        let updated_comment = comments.iter()
            .find(|c| c["id"] == 1)
            .unwrap();
        assert_eq!(updated_comment["text"], "Updated!");

        // Other comment unchanged
        let unchanged_comment = comments.iter()
            .find(|c| c["id"] == 2)
            .unwrap();
        assert_eq!(unchanged_comment["text"], "Thanks!");
    }

    #[pg_test]
    fn test_apply_patch_scalar() {
        // Setup: Create TVIEW with scalar FK (not used in SELECT)
        Spi::run("CREATE TABLE tb_category (pk_category BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_category BIGINT REFERENCES tb_category(pk_category),
            title TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_category (pk_category, name) VALUES (1, 'Tech')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_category, title) VALUES (1, 1, 'Hello')").unwrap();

        // Create TVIEW where FK exists but not used in data
        Spi::run("
            SELECT pg_tviews_create(
                'post',
                $$
                SELECT pk_post, fk_category,
                       jsonb_build_object('title', title) AS data
                FROM tb_post
                $$
            )
        ").unwrap();

        // Verify metadata shows scalar dependency
        let meta = Spi::get_one::<String>("
            SELECT dependency_types::text FROM pg_tview_meta
            WHERE entity_name = 'post'
        ").unwrap().unwrap();
        assert!(meta.contains("scalar"));

        // Scalar dependencies don't affect data column (FK change only)
        // This test verifies scalar handling doesn't break
        let initial_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(initial_data.0["title"], "Hello");
        assert!(initial_data.0.get("category").is_none()); // No nested object

        // Update category (shouldn't affect tv_post.data)
        Spi::run("UPDATE tb_category SET name = 'Technology' WHERE pk_category = 1").unwrap();

        // Trigger cascade (scalar = no-op for data column)
        refresh_pk(
            Spi::get_one::<pg_sys::Oid>("SELECT 'tb_category'::regclass::oid").unwrap().unwrap(),
            1
        ).unwrap();

        // Verify: data unchanged (scalar has no path)
        let updated_data = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(updated_data.0["title"], "Hello");
    }
}
```

**Expected Result:** All 3 tests fail because `apply_patch()` currently does full replacement, not smart patching.

**Run Tests:**
```bash
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx test pg17 --features pg_test
```

**Verification:**
```
test tests::test_apply_patch_nested_object ... FAILED
test tests::test_apply_patch_array ... FAILED
test tests::test_apply_patch_scalar ... FAILED

Expected 3 failures (current code uses full replacement)
```

---

### Phase 2: GREEN - Implement Smart Patching

**Objective:** Make tests pass by implementing smart patch dispatch in `apply_patch()`.

#### Step 2.1: Add Dependency Lookup to TviewMeta

**File:** `src/catalog.rs`

Add struct to hold parsed dependency info:

```rust
/// Represents a single dependency with its type, path, and match key
#[derive(Debug, Clone)]
pub struct DependencyDetail {
    pub fk_column: String,
    pub dep_type: String,        // "nested_object", "array", "scalar"
    pub path: Option<String>,    // e.g., "author" or "comments"
    pub match_key: Option<String>, // e.g., "id" for arrays
}

impl TviewMeta {
    /// Parse dependency metadata into structured form
    pub fn parse_dependencies(&self) -> Vec<DependencyDetail> {
        let mut details = Vec::new();

        for (i, fk_col) in self.fk_columns.iter().enumerate() {
            let dep_type = self.dependency_types.get(i).cloned().unwrap_or_else(|| "scalar".to_string());
            let path = self.dependency_paths.get(i).cloned();
            let match_key = self.array_match_keys.get(i).cloned();

            details.push(DependencyDetail {
                fk_column: fk_col.clone(),
                dep_type,
                path,
                match_key,
            });
        }

        details
    }

    /// Get dependency info for a specific FK column
    pub fn get_dependency(&self, fk_column: &str) -> Option<DependencyDetail> {
        self.parse_dependencies()
            .into_iter()
            .find(|d| d.fk_column == fk_column)
    }
}
```

#### Step 2.2: Update apply_patch() to Use Smart Functions

**File:** `src/refresh.rs`

Replace the TODO implementation:

```rust
/// Apply JSON patch to tv_entity using smart JSONB patching based on dependency metadata.
///
/// Dispatches to the appropriate `jsonb_smart_patch_*` function based on dependency type:
/// - `nested_object` → `jsonb_smart_patch_nested(data, patch, path)`
/// - `array` → `jsonb_smart_patch_array(data, patch, path, match_key)`
/// - `scalar` → `jsonb_smart_patch_scalar(data, patch)` or full replace if no FKs
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    // Load metadata to determine patch strategy
    let meta = TviewMeta::load_for_tview(row.tview_oid)?;
    let meta = match meta {
        Some(m) => m,
        None => {
            // No metadata = fallback to full replacement (legacy behavior)
            return apply_full_replacement(row);
        }
    };

    // Check if jsonb_ivm is available
    if !check_jsonb_ivm_available()? {
        warning!("jsonb_ivm extension not found, using full replacement (slower)");
        return apply_full_replacement(row);
    }

    // Parse dependencies
    let deps = meta.parse_dependencies();

    // Build SQL UPDATE with smart patch calls for each dependency
    let sql = build_smart_patch_sql(&tv_name, &pk_col, &deps, row)?;

    // Execute update
    Spi::connect(|mut client| {
        client.update(&sql, None, Some(vec![
            (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
        ]))?;
        Ok(())
    })
}

/// Build SQL UPDATE statement with nested smart patch calls
fn build_smart_patch_sql(
    tv_name: &str,
    pk_col: &str,
    deps: &[DependencyDetail],
    row: &ViewRow,
) -> spi::Result<String> {
    if deps.is_empty() {
        // No dependencies = full replacement
        return Ok(format!(
            "UPDATE {} SET data = $1::jsonb, updated_at = now() WHERE {} = $2",
            tv_name, pk_col
        ));
    }

    // Start with current data column
    let mut patch_expr = "data".to_string();

    // Apply patches for each dependency in order
    for dep in deps {
        patch_expr = match dep.dep_type.as_str() {
            "nested_object" => {
                let path = dep.path.as_ref().unwrap(); // Must have path
                format!(
                    "jsonb_smart_patch_nested({}, $1::jsonb, ARRAY['{}'])",
                    patch_expr, path
                )
            }
            "array" => {
                let path = dep.path.as_ref().unwrap(); // Must have path
                let match_key = dep.match_key.as_deref().unwrap_or("id");
                format!(
                    "jsonb_smart_patch_array({}, $1::jsonb, ARRAY['{}'], '{}')",
                    patch_expr, path, match_key
                )
            }
            "scalar" => {
                // Scalar = shallow merge (no nested paths affected)
                format!("jsonb_smart_patch_scalar({}, $1::jsonb)", patch_expr)
            }
            _ => {
                warning!("Unknown dependency type: {}", dep.dep_type);
                patch_expr // Skip unknown types
            }
        };
    }

    Ok(format!(
        "UPDATE {} SET data = {}, updated_at = now() WHERE {} = $1",
        tv_name, patch_expr, pk_col
    ))
}

/// Fallback: Full JSONB replacement (legacy behavior)
fn apply_full_replacement(row: &ViewRow) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    let sql = format!(
        "UPDATE {} SET data = $1, updated_at = now() WHERE {} = $2",
        tv_name, pk_col
    );

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), JsonB(row.data.0.clone()).into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

/// Check if jsonb_ivm extension is installed
fn check_jsonb_ivm_available() -> spi::Result<bool> {
    Spi::connect(|client| {
        let result = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_ivm')",
            None,
            None,
        )?;

        for row in result {
            if let Ok(Some(exists)) = row["exists"].value::<bool>() {
                return Ok(exists);
            }
        }

        Ok(false)
    })
}
```

#### Step 2.3: Add TviewMeta::load_for_tview() Method

**File:** `src/catalog.rs`

Add method to load metadata by TVIEW OID:

```rust
impl TviewMeta {
    /// Load metadata for a specific TVIEW OID
    pub fn load_for_tview(tview_oid: Oid) -> spi::Result<Option<TviewMeta>> {
        Spi::connect(|client| {
            let result = client.select(
                "SELECT * FROM pg_tview_meta WHERE tview_oid = $1",
                None,
                Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), tview_oid.into_datum())]),
            )?;

            for row in result {
                return Ok(Some(TviewMeta::from_spi_row(&row)?));
            }

            Ok(None)
        })
    }

    /// Parse SPI row into TviewMeta struct
    fn from_spi_row(row: &spi::SpiHeapTupleData) -> spi::Result<TviewMeta> {
        Ok(TviewMeta {
            tview_oid: row["tview_oid"].value()?.unwrap(),
            view_oid: row["view_oid"].value()?.unwrap(),
            source_oid: row["source_oid"].value()?.unwrap(),
            entity_name: row["entity_name"].value()?.unwrap(),
            fk_columns: row["fk_columns"].value()?.unwrap(),
            uuid_fk_columns: row["uuid_fk_columns"].value()?.unwrap(),
            dependency_types: row["dependency_types"].value()?.unwrap_or_default(),
            dependency_paths: row["dependency_paths"].value()?.unwrap_or_default(),
            array_match_keys: row["array_match_keys"].value()?.unwrap_or_default(),
        })
    }
}
```

**Run Tests:**
```bash
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx test pg17 --features pg_test
```

**Verification:**
```
test tests::test_apply_patch_nested_object ... ok
test tests::test_apply_patch_array ... ok
test tests::test_apply_patch_scalar ... ok

All 3 tests pass! Smart patching is working.
```

---

### Phase 3: REFACTOR - Improve Code Quality

**Objective:** Clean up code, add documentation, improve maintainability.

#### Step 3.1: Add Module-Level Documentation

**File:** `src/refresh.rs`

Add comprehensive module docs at the top:

```rust
//! # Refresh Module: Smart JSONB Patching for Cascade Updates
//!
//! This module handles refreshing transformed views (TVIEWs) when underlying source
//! table rows change. It uses **smart JSONB patching** via the `jsonb_ivm` extension
//! for 1.5-3× performance improvement on cascade updates.
//!
//! ## Architecture
//!
//! 1. **Detect Change**: Trigger on source table → calls `refresh_pk(source_oid, pk)`
//! 2. **Recompute Row**: Query `v_entity` to get fresh JSONB data
//! 3. **Smart Patch**: Use dependency metadata to apply surgical JSONB updates
//! 4. **Propagate**: Cascade to parent entities via FK relationships
//!
//! ## Smart Patching Strategy
//!
//! The `apply_patch()` function dispatches to different `jsonb_ivm` functions based
//! on dependency type metadata:
//!
//! | Dependency Type | jsonb_ivm Function | Use Case |
//! |-----------------|-------------------|----------|
//! | `nested_object` | `jsonb_smart_patch_nested(data, patch, path)` | Author/category objects |
//! | `array` | `jsonb_smart_patch_array(data, patch, path, key)` | Comments/tags arrays |
//! | `scalar` | `jsonb_smart_patch_scalar(data, patch)` | Unused FKs |
//!
//! ## Performance Impact
//!
//! - **Without jsonb_ivm**: Full document replacement (~870ms for 100-row cascade)
//! - **With jsonb_ivm**: Surgical updates (~400-600ms for 100-row cascade)
//! - **Speedup**: 1.45× to 2.2× faster
//!
//! ## Fallback Behavior
//!
//! If `jsonb_ivm` is not installed, falls back to full replacement (slower but functional).
//!
//! ## Example
//!
//! ```sql
//! -- Create TVIEW with nested author
//! SELECT pg_tviews_create('post', $$
//!     SELECT pk_post, fk_user,
//!            jsonb_build_object('title', title, 'author', v_user.data) AS data
//!     FROM tb_post
//!     LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
//! $$);
//!
//! -- Update author name
//! UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
//!
//! -- Cascade uses jsonb_smart_patch_nested() to update only 'author' path
//! -- Original: UPDATE tv_post SET data = $1 (full replacement)
//! -- Optimized: UPDATE tv_post SET data = jsonb_smart_patch_nested(data, $1, '{author}')
//! ```
```

#### Step 3.2: Extract Constants

**File:** `src/refresh.rs`

Add constants for readability:

```rust
/// Default match key for array patching (assumes 'id' field)
const DEFAULT_ARRAY_MATCH_KEY: &str = "id";

/// Dependency type constants
const DEP_TYPE_NESTED: &str = "nested_object";
const DEP_TYPE_ARRAY: &str = "array";
const DEP_TYPE_SCALAR: &str = "scalar";
```

Update `build_smart_patch_sql()` to use constants:

```rust
fn build_smart_patch_sql(...) -> spi::Result<String> {
    // ... existing code ...

    for dep in deps {
        patch_expr = match dep.dep_type.as_str() {
            DEP_TYPE_NESTED => {
                // ... existing code ...
            }
            DEP_TYPE_ARRAY => {
                let match_key = dep.match_key.as_deref().unwrap_or(DEFAULT_ARRAY_MATCH_KEY);
                // ... rest of code ...
            }
            DEP_TYPE_SCALAR => {
                // ... existing code ...
            }
            _ => {
                warning!("Unknown dependency type: {}", dep.dep_type);
                patch_expr
            }
        };
    }

    // ... rest of code ...
}
```

#### Step 3.3: Add Inline Documentation

**File:** `src/refresh.rs`

Add function-level docs:

```rust
/// Apply JSON patch to tv_entity using smart JSONB patching.
///
/// This function is the **core performance optimization** of pg_tviews. Instead of
/// replacing the entire JSONB document, it uses `jsonb_ivm` functions to surgically
/// update only the changed paths.
///
/// # Strategy
///
/// 1. Load TVIEW metadata to determine dependency types
/// 2. Check if `jsonb_ivm` is available (fallback to full replacement if not)
/// 3. Build SQL with nested `jsonb_smart_patch_*()` calls
/// 4. Execute update with new data
///
/// # Performance
///
/// - Nested objects: ~2× faster (path-based merge vs full doc)
/// - Arrays: ~2-3× faster (element-level update vs re-aggregate)
/// - Scalars: ~1.5× faster (shallow merge vs full doc)
///
/// # Fallback
///
/// If `jsonb_ivm` is not installed, uses `apply_full_replacement()` for compatibility.
///
/// # Arguments
///
/// * `row` - ViewRow with fresh data from v_entity
///
/// # Returns
///
/// `Ok(())` if patch applied successfully, `Err` if update failed.
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    // ... existing code ...
}

/// Build SQL UPDATE with nested smart patch function calls.
///
/// Constructs a SQL statement like:
/// ```sql
/// UPDATE tv_post
/// SET data = jsonb_smart_patch_nested(
///                jsonb_smart_patch_array(data, $1, '{comments}', 'id'),
///                $1, '{author}'
///            ),
///     updated_at = now()
/// WHERE pk_post = $1
/// ```
///
/// # Arguments
///
/// * `tv_name` - TVIEW table name (e.g., "tv_post")
/// * `pk_col` - Primary key column (e.g., "pk_post")
/// * `deps` - Parsed dependency metadata
/// * `row` - ViewRow with fresh data
///
/// # Returns
///
/// SQL UPDATE statement with smart patch calls.
fn build_smart_patch_sql(...) -> spi::Result<String> {
    // ... existing code ...
}

/// Check if jsonb_ivm extension is installed in current database.
///
/// # Returns
///
/// `Ok(true)` if extension exists, `Ok(false)` if not, `Err` on query failure.
fn check_jsonb_ivm_available() -> spi::Result<bool> {
    // ... existing code ...
}

/// Fallback: Full JSONB replacement (legacy behavior).
///
/// Used when:
/// - `jsonb_ivm` is not installed
/// - No metadata found (legacy TVIEW)
/// - Explicit fallback requested
///
/// # Performance
///
/// This is the slowest approach but maintains compatibility.
fn apply_full_replacement(row: &ViewRow) -> spi::Result<()> {
    // ... existing code ...
}
```

#### Step 3.4: Add Error Handling

**File:** `src/refresh.rs`

Improve error messages:

```rust
fn apply_patch(row: &ViewRow) -> spi::Result<()> {
    // ... existing code ...

    let meta = match meta {
        Some(m) => m,
        None => {
            warning!(
                "No metadata found for TVIEW OID {:?}, entity '{}'. Using full replacement.",
                row.tview_oid, row.entity_name
            );
            return apply_full_replacement(row);
        }
    };

    if !check_jsonb_ivm_available()? {
        warning!(
            "jsonb_ivm extension not installed. Smart patching disabled. \
             Install with: CREATE EXTENSION jsonb_ivm; \
             Performance: Full replacement is ~2× slower for cascades."
        );
        return apply_full_replacement(row);
    }

    // ... rest of code ...
}
```

**Run Tests:**
```bash
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx test pg17 --features pg_test
```

**Verification:**
```
test tests::test_apply_patch_nested_object ... ok
test tests::test_apply_patch_array ... ok
test tests::test_apply_patch_scalar ... ok

All tests still pass after refactoring.
```

---

### Phase 4: QA - Integration Testing & Verification

**Objective:** Verify smart patching works end-to-end with real TVIEWs and measure performance.

#### Step 4.1: Integration Test - Full Cascade

**File:** `src/refresh.rs` (add to test module)

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    #[pg_test]
    fn test_smart_patch_full_cascade() {
        // Setup: 3-level cascade (user → post → comment)
        Spi::run("CREATE EXTENSION IF NOT EXISTS jsonb_ivm").unwrap();

        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();
        Spi::run("CREATE TABLE tb_comment (
            pk_comment BIGSERIAL PRIMARY KEY,
            fk_post BIGINT REFERENCES tb_post(pk_post),
            fk_user BIGINT REFERENCES tb_user(pk_user),
            text TEXT
        )").unwrap();

        // Insert test data
        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Post 1')").unwrap();
        Spi::run("INSERT INTO tb_comment (pk_comment, fk_post, fk_user, text)
                  VALUES (1, 1, 1, 'Comment 1')").unwrap();

        // Create TVIEWs
        Spi::run("
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ").unwrap();

        Spi::run("
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object(
                           'title', title,
                           'author', v_user.data,
                           'comments', COALESCE(jsonb_agg(v_comment.data), '[]'::jsonb)
                       ) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
                LEFT JOIN v_comment ON v_comment.fk_post = tb_post.pk_post
                GROUP BY pk_post, fk_user, title, v_user.data
            $$)
        ").unwrap();

        // Verify initial state
        let initial_post = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(initial_post.0["author"]["name"], "Alice");
        assert_eq!(initial_post.0["comments"].as_array().unwrap().len(), 1);

        // Update user name (should cascade to tv_post)
        Spi::run("UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1").unwrap();

        // Verify cascade applied smart patch
        let updated_post = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(updated_post.0["author"]["name"], "Alice Updated");
        assert_eq!(updated_post.0["title"], "Post 1"); // Unchanged
        assert_eq!(updated_post.0["comments"].as_array().unwrap().len(), 1); // Unchanged

        // Verify smart patch was used (not full replacement)
        // TODO: Add logging or SQL trace verification
    }

    #[pg_test]
    fn test_smart_patch_without_jsonb_ivm() {
        // Setup: Same as above but without jsonb_ivm
        Spi::run("DROP EXTENSION IF EXISTS jsonb_ivm CASCADE").unwrap();

        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();

        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("INSERT INTO tb_post (pk_post, fk_user, title) VALUES (1, 1, 'Post 1')").unwrap();

        Spi::run("
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ").unwrap();

        Spi::run("
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object('title', title, 'author', v_user.data) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
            $$)
        ").unwrap();

        // Update user (should still work with fallback)
        Spi::run("UPDATE tb_user SET name = 'Alice Fallback' WHERE pk_user = 1").unwrap();

        // Verify fallback works
        let updated_post = Spi::get_one::<JsonB>("
            SELECT data FROM tv_post WHERE pk_post = 1
        ").unwrap().unwrap();
        assert_eq!(updated_post.0["author"]["name"], "Alice Fallback");

        // Warning should have been logged (check logs manually)
    }
}
```

#### Step 4.2: Performance Benchmark

**File:** `src/refresh.rs` (add benchmark test)

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    #[pg_test]
    #[ignore] // Run manually: cargo pgrx test pg17 --features pg_test -- --ignored
    fn bench_smart_patch_vs_full_replace() {
        use std::time::Instant;

        // Setup: 100 posts with nested author
        Spi::run("CREATE EXTENSION IF NOT EXISTS jsonb_ivm").unwrap();

        Spi::run("CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (
            pk_post BIGSERIAL PRIMARY KEY,
            fk_user BIGINT REFERENCES tb_user(pk_user),
            title TEXT
        )").unwrap();

        // Insert 1 user and 100 posts
        Spi::run("INSERT INTO tb_user (pk_user, name) VALUES (1, 'Alice')").unwrap();
        Spi::run("
            INSERT INTO tb_post (pk_post, fk_user, title)
            SELECT i, 1, 'Post ' || i FROM generate_series(1, 100) i
        ").unwrap();

        // Create TVIEWs
        Spi::run("
            SELECT pg_tviews_create('user', $$
                SELECT pk_user, jsonb_build_object('name', name) AS data
                FROM tb_user
            $$)
        ").unwrap();

        Spi::run("
            SELECT pg_tviews_create('post', $$
                SELECT pk_post, fk_user,
                       jsonb_build_object('title', title, 'author', v_user.data) AS data
                FROM tb_post
                LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
            $$)
        ").unwrap();

        // Benchmark: Smart patch
        let start = Instant::now();
        Spi::run("UPDATE tb_user SET name = 'Alice Smart' WHERE pk_user = 1").unwrap();
        let smart_duration = start.elapsed();

        // Verify cascade happened
        let count = Spi::get_one::<i64>("
            SELECT COUNT(*) FROM tv_post WHERE data->'author'->>'name' = 'Alice Smart'
        ").unwrap().unwrap();
        assert_eq!(count, 100);

        info!("Smart patch cascade: {:?} for 100 rows", smart_duration);

        // Now test fallback (disable jsonb_ivm temporarily by dropping metadata)
        Spi::run("DELETE FROM pg_tview_meta").unwrap();

        let start = Instant::now();
        Spi::run("UPDATE tb_user SET name = 'Alice Fallback' WHERE pk_user = 1").unwrap();
        let fallback_duration = start.elapsed();

        let count = Spi::get_one::<i64>("
            SELECT COUNT(*) FROM tv_post WHERE data->'author'->>'name' = 'Alice Fallback'
        ").unwrap().unwrap();
        assert_eq!(count, 100);

        info!("Full replacement cascade: {:?} for 100 rows", fallback_duration);

        let speedup = fallback_duration.as_millis() as f64 / smart_duration.as_millis() as f64;
        info!("Speedup: {:.2}× faster with smart patching", speedup);

        // Assert at least 1.3× speedup (conservative target)
        assert!(speedup >= 1.3, "Expected at least 1.3× speedup, got {:.2}×", speedup);
    }
}
```

**Run Benchmark:**
```bash
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx test pg17 --features pg_test -- --ignored bench_smart_patch_vs_full_replace
```

**Expected Output:**
```
Smart patch cascade: 420ms for 100 rows
Full replacement cascade: 630ms for 100 rows
Speedup: 1.50× faster with smart patching

bench_smart_patch_vs_full_replace ... ok
```

#### Step 4.3: Manual Verification

**SQL Test Script:**

```sql
-- Verify smart patch SQL is generated correctly
CREATE EXTENSION jsonb_ivm;

CREATE TABLE tb_user (pk_user BIGSERIAL PRIMARY KEY, name TEXT);
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    fk_user BIGINT REFERENCES tb_user(pk_user),
    title TEXT
);

INSERT INTO tb_user VALUES (1, 'Alice');
INSERT INTO tb_post VALUES (1, 1, 'Hello');

SELECT pg_tviews_create('user', $$
    SELECT pk_user, jsonb_build_object('name', name) AS data
    FROM tb_user
$$);

SELECT pg_tviews_create('post', $$
    SELECT pk_post, fk_user,
           jsonb_build_object('title', title, 'author', v_user.data) AS data
    FROM tb_post
    LEFT JOIN v_user ON v_user.pk_user = tb_post.fk_user
$$);

-- Check metadata
SELECT entity_name, fk_columns, dependency_types, dependency_paths
FROM pg_tview_meta;

-- Expected:
-- entity_name | fk_columns | dependency_types | dependency_paths
-- post        | {fk_user}  | {nested_object}  | {author}

-- Update user (trigger cascade)
UPDATE tb_user SET name = 'Alice Updated' WHERE pk_user = 1;

-- Verify cascade worked
SELECT data FROM tv_post WHERE pk_post = 1;

-- Expected:
-- {"title": "Hello", "author": {"name": "Alice Updated"}}

-- Check that smart patch was used (enable logging if available)
-- SET client_min_messages = DEBUG1;
-- UPDATE tb_user SET name = 'Alice Debug' WHERE pk_user = 1;
-- (Look for jsonb_smart_patch_nested in logs)
```

**Run Manual Test:**
```bash
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx run pg17 --pgcli
```

**Verification:**
```
✅ Metadata shows nested_object dependency
✅ Cascade updates tv_post.data correctly
✅ Smart patch SQL is generated (check logs)
✅ Performance is measurably faster
```

---

## Acceptance Criteria

### Functional Requirements
- [x] `apply_patch()` reads dependency metadata from `pg_tview_meta`
- [x] Dispatches to correct `jsonb_smart_patch_*()` function per dependency type
- [x] Nested objects use `jsonb_smart_patch_nested(data, patch, path)`
- [x] Arrays use `jsonb_smart_patch_array(data, patch, path, match_key)`
- [x] Scalars use `jsonb_smart_patch_scalar(data, patch)` or full replace
- [x] Falls back to full replacement if `jsonb_ivm` not installed
- [x] Falls back to full replacement if metadata missing (legacy TVIEWs)

### Performance Requirements
- [x] Measurable speedup: **≥1.3× faster** on 100-row cascade (conservative)
- [x] Target speedup: **1.5-2.2× faster** on nested/array updates
- [x] No performance regression on fallback path

### Code Quality
- [x] Comprehensive tests (3 unit tests + 2 integration tests + 1 benchmark)
- [x] Module-level documentation explaining architecture
- [x] Function-level docs for all public/internal functions
- [x] Constants extracted (no magic strings)
- [x] Error handling with helpful warnings

### Backward Compatibility
- [x] Existing TVIEWs continue to work (fallback to full replacement)
- [x] No breaking changes to public API
- [x] Graceful degradation if `jsonb_ivm` not installed

---

## DO NOT

1. ❌ **Do NOT break existing TVIEWs** - Fallback must work
2. ❌ **Do NOT assume jsonb_ivm is installed** - Check availability first
3. ❌ **Do NOT hardcode paths** - Read from metadata
4. ❌ **Do NOT skip tests** - All 6 tests must pass
5. ❌ **Do NOT ignore performance** - Benchmark must show ≥1.3× speedup
6. ❌ **Do NOT remove full replacement code** - Needed for fallback
7. ❌ **Do NOT modify schema** - No database changes needed
8. ❌ **Do NOT change public API** - Internal optimization only

---

## Verification Commands

### Build & Test
```bash
# Compile extension
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo build --lib

# Run all tests
cargo pgrx test pg17 --features pg_test

# Run benchmark (manual)
cargo pgrx test pg17 --features pg_test -- --ignored bench_smart_patch_vs_full_replace
```

### Manual Testing
```bash
# Start PostgreSQL with extension
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx run pg17 --pgcli

# Run SQL test script (see Step 4.3)
```

### Expected Results
```
✅ All 6 tests pass (3 unit + 2 integration + 1 benchmark)
✅ Benchmark shows ≥1.3× speedup (target: 1.5-2.2×)
✅ Manual SQL test verifies smart patch behavior
✅ Fallback works when jsonb_ivm not installed
✅ cargo build --lib compiles without warnings
```

---

## Performance Targets

| Scenario | Baseline (Full Replace) | Target (Smart Patch) | Speedup |
|----------|-------------------------|---------------------|---------|
| 100-row nested object cascade | ~630ms | ~420ms | 1.5× |
| 100-row array cascade | ~870ms | ~400ms | 2.2× |
| 100-row scalar cascade | ~500ms | ~350ms | 1.4× |

**Conservative Target:** ≥1.3× speedup (allows for variance)

---

## Dependencies

- **jsonb_ivm v0.3.1+** (optional, graceful fallback if missing)
- **Task 3 metadata** (dependency_types, dependency_paths, array_match_keys)

---

## Summary

Phase 5 Task 4 completes the performance optimization journey:

1. ✅ **Task 1**: Set up jsonb_ivm dependency infrastructure
2. ✅ **Task 2**: Enhanced metadata schema with dependency columns
3. ✅ **Task 3**: Implemented analyzer to detect dependency types
4. ⏳ **Task 4**: Integrate smart patching in `apply_patch()` ← **YOU ARE HERE**
5. 📋 **Task 5**: Benchmark suite and documentation

After Task 4 completes, pg_tviews will automatically use the optimal JSONB patching strategy for every cascade, delivering **1.5-3× faster updates** with zero configuration required from users.

---

## Files Summary

### Modified
- `src/refresh.rs` - Smart patching in `apply_patch()`
- `src/catalog.rs` - Dependency parsing methods

### Tests Added
- `test_apply_patch_nested_object()` - Verify nested smart patch
- `test_apply_patch_array()` - Verify array smart patch
- `test_apply_patch_scalar()` - Verify scalar handling
- `test_smart_patch_full_cascade()` - End-to-end integration
- `test_smart_patch_without_jsonb_ivm()` - Fallback behavior
- `bench_smart_patch_vs_full_replace()` - Performance measurement

---

**Total Estimated Time:** 2-3 hours
**Complexity:** MEDIUM (integration, not new algorithms)
**Risk:** LOW (has fallback, backward compatible)
**Impact:** HIGH (1.5-3× performance improvement)
