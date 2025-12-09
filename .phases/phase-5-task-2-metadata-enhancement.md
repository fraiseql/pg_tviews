# Phase 5 Task 2: Metadata Enhancement for jsonb_ivm

**Status:** Ready to implement
**Duration:** 1-2 days
**Parent:** Phase 5 - jsonb_ivm Integration
**TDD Phase:** RED → GREEN → REFACTOR

---

## Objective

Update the Rust `TviewMeta` struct and loader functions to fetch and use the existing metadata columns (`dependency_types`, `dependency_paths`, `array_match_keys`) that are already present in the SQL schema.

**Success Criteria:**
- ✅ `DependencyType` enum defined with Scalar/NestedObject/Array variants
- ✅ `TviewMeta` struct includes new fields
- ✅ Loader methods fetch new columns from database
- ✅ Default values populated for existing TVIEWs
- ✅ Tests verify fields are accessible
- ✅ No breaking changes to existing functionality

---

## Context

The SQL schema **already has** these columns (added in Phase 4):
```sql
CREATE TABLE pg_tview_meta (
    -- ... existing columns ...
    dependency_types TEXT[] NOT NULL DEFAULT '{}',
    dependency_paths TEXT[][] NOT NULL DEFAULT '{}',
    array_match_keys TEXT[] NOT NULL DEFAULT '{}',
);
```

**Problem:** The Rust code doesn't use them yet!

**Current Rust struct:**
```rust
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,
    // ❌ Missing: dependency_types, dependency_paths, array_match_keys
}
```

**Why This Matters:**
- Phase 5 Tasks 3-4 need to know *how* each FK manifests in JSONB
- `dependency_types` tells us: is it scalar, nested object, or array?
- `dependency_paths` tells us: where in JSONB to find it (e.g., `["author"]`)
- `array_match_keys` tells us: for arrays, what key to match on (e.g., `"id"`)

---

## RED Phase: Write Failing Tests First

### Test 1: DependencyType Enum

**File:** `src/catalog.rs` (add test module at bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_type_from_str() {
        assert_eq!(DependencyType::from_str("scalar"), DependencyType::Scalar);
        assert_eq!(DependencyType::from_str("nested_object"), DependencyType::NestedObject);
        assert_eq!(DependencyType::from_str("array"), DependencyType::Array);
        assert_eq!(DependencyType::from_str("unknown"), DependencyType::Scalar); // default
    }

    #[test]
    fn test_dependency_type_to_str() {
        assert_eq!(DependencyType::Scalar.as_str(), "scalar");
        assert_eq!(DependencyType::NestedObject.as_str(), "nested_object");
        assert_eq!(DependencyType::Array.as_str(), "array");
    }
}
```

**Expected Result:** Tests FAIL because `DependencyType` enum doesn't exist.

### Test 2: TviewMeta Struct Fields

**File:** `src/catalog.rs` (add to test module)

```rust
#[cfg(test)]
mod tests {
    // ... previous tests ...

    #[test]
    fn test_tview_meta_has_new_fields() {
        let meta = TviewMeta {
            tview_oid: pg_sys::Oid::from(1234),
            view_oid: pg_sys::Oid::from(5678),
            entity_name: "test".to_string(),
            sync_mode: 's',
            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![DependencyType::Scalar],
            dependency_paths: vec![None],
            array_match_keys: vec![None],
        };

        assert_eq!(meta.dependency_types.len(), 1);
        assert_eq!(meta.dependency_paths.len(), 1);
        assert_eq!(meta.array_match_keys.len(), 1);
    }
}
```

**Expected Result:** Tests FAIL because `TviewMeta` doesn't have those fields.

### Test 3: SQL Integration Test

**File:** `test/sql/51_metadata_enhancement.sql` (NEW)

```sql
-- Phase 5 Task 2 RED: Test metadata enhancement
-- This test verifies that new metadata fields are populated

BEGIN;
    SET client_min_messages TO WARNING;

    -- Cleanup
    DROP EXTENSION IF EXISTS pg_tviews CASCADE;
    CREATE EXTENSION pg_tviews;

    -- Test Case 1: Create TVIEW and verify metadata includes new fields
    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, name TEXT);
    INSERT INTO tb_user VALUES (1, 'Alice');

    CREATE TABLE tb_post (
        pk_post INT PRIMARY KEY,
        fk_user INT REFERENCES tb_user(pk_user),
        title TEXT
    );
    INSERT INTO tb_post VALUES (1, 1, 'First Post');

    -- Create TVIEW with nested object (user data embedded in post)
    SELECT pg_tviews_create('post', $$
        SELECT
            p.pk_post,
            p.fk_user,
            jsonb_build_object(
                'title', p.title,
                'author', jsonb_build_object('name', u.name)
            ) AS data
        FROM tb_post p
        LEFT JOIN tb_user u ON p.fk_user = u.pk_user
    $$);

    -- Verify metadata row exists
    SELECT COUNT(*) = 1 AS meta_exists FROM pg_tview_meta WHERE entity = 'post';
    -- Expected: t

    -- Verify new columns have content (not just empty arrays)
    SELECT
        array_length(dependency_types, 1) > 0 AS has_dep_types,
        array_length(fk_columns, 1) > 0 AS has_fk_cols
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: t, t

    -- Test Case 2: Verify we can query nested dependency info
    SELECT dependency_types[1] AS first_dep_type
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: 'nested_object' or 'scalar' (depending on detection logic in Task 3)
    -- For now, we just check it's not NULL

ROLLBACK;
```

**Expected Result:** Test may partially work (table exists) but new fields will be empty or NULL because detection logic doesn't exist yet (that's Task 3).

---

## GREEN Phase: Make Tests Pass (Minimal Implementation)

### Step 1: Define DependencyType Enum

**File:** `src/catalog.rs`

**Location:** Add after imports, before `TviewMeta` struct

```rust
use pgrx::prelude::*;
use pgrx::pg_sys::Oid;
use pgrx::IntoDatum;
use serde::{Deserialize, Serialize};

/// Type of dependency relationship for jsonb_ivm optimization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyType {
    /// Direct column from base table (no nested JSONB)
    Scalar,
    /// Embedded object via jsonb_build_object in nested key
    NestedObject,
    /// Array created via jsonb_agg
    Array,
}

impl DependencyType {
    /// Parse from database string representation
    pub fn from_str(s: &str) -> Self {
        match s {
            "scalar" => DependencyType::Scalar,
            "nested_object" => DependencyType::NestedObject,
            "array" => DependencyType::Array,
            _ => DependencyType::Scalar, // default fallback
        }
    }

    /// Convert to database string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            DependencyType::Scalar => "scalar",
            DependencyType::NestedObject => "nested_object",
            DependencyType::Array => "array",
        }
    }
}
```

### Step 2: Update TviewMeta Struct

**File:** `src/catalog.rs`

**Location:** Modify existing struct (lines 8-15)

```rust
/// Represents a row in pg_tview_meta (your own catalog table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TviewMeta {
    pub tview_oid: Oid,
    pub view_oid: Oid,
    pub entity_name: String,
    pub sync_mode: char, // 's' = sync (default), 'a' = async (future)
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,

    // NEW: jsonb_ivm optimization metadata
    pub dependency_types: Vec<DependencyType>,
    pub dependency_paths: Vec<Option<Vec<String>>>,  // NULL or array of path parts
    pub array_match_keys: Vec<Option<String>>,       // NULL or match key name
}
```

### Step 3: Update load_for_source() Method

**File:** `src/catalog.rs`

**Location:** Modify `load_for_source()` method (lines 19-47)

```rust
pub fn load_for_source(source_oid: Oid) -> spi::Result<Option<Self>> {
    Spi::connect(|client| {
        let rows = client.select(
            "SELECT table_oid AS tview_oid, view_oid, entity, \
                    fk_columns, uuid_fk_columns, \
                    dependency_types, dependency_paths, array_match_keys \
             FROM pg_tview_meta \
             WHERE view_oid = $1 OR table_oid = $1",
            None,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::OIDOID), source_oid.into_datum())]),
        )?;

        let mut result = None;
        for row in rows {
            // Extract existing arrays
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            // Extract NEW arrays
            let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
            let dep_types = dep_types_raw
                .unwrap_or_default()
                .into_iter()
                .map(|s| DependencyType::from_str(&s))
                .collect();

            // dependency_paths is TEXT[][] (array of arrays)
            // PostgreSQL returns this as Vec<Option<Vec<Option<String>>>>
            let dep_paths_raw: Option<Vec<Option<Vec<Option<String>>>>> =
                row["dependency_paths"].value().unwrap_or(None);
            let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
                .unwrap_or_default()
                .into_iter()
                .map(|opt_arr| {
                    opt_arr.map(|arr| {
                        arr.into_iter()
                            .filter_map(|opt_s| opt_s)
                            .collect()
                    })
                })
                .collect();

            let array_keys: Option<Vec<Option<String>>> =
                row["array_match_keys"].value().unwrap_or(None);

            result = Some(Self {
                tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                view_oid: row["view_oid"].value().unwrap().unwrap(),
                entity_name: row["entity"].value().unwrap().unwrap(),
                sync_mode: 's', // Default to synchronous
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                dependency_types: dep_types,
                dependency_paths: dep_paths,
                array_match_keys: array_keys.unwrap_or_default(),
            });
            break; // Only get first row
        }
        Ok(result)
    })
}
```

### Step 4: Update load_by_entity() Method

**File:** `src/catalog.rs`

**Location:** Modify `load_by_entity()` method (lines 49-78)

Apply the same pattern as Step 3:
```rust
pub fn load_by_entity(entity_name: &str) -> spi::Result<Option<Self>> {
    Spi::connect(|client| {
        let rows = client.select(
            "SELECT table_oid AS tview_oid, view_oid, entity, \
                    fk_columns, uuid_fk_columns, \
                    dependency_types, dependency_paths, array_match_keys \
             FROM pg_tview_meta \
             WHERE entity = $1",
            None,
            Some(vec![(PgOid::BuiltIn(PgBuiltInOids::TEXTOID), entity_name.into_datum())]),
        )?;

        let mut result = None;
        for row in rows {
            // [Same extraction logic as load_for_source()]
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
            let dep_types = dep_types_raw
                .unwrap_or_default()
                .into_iter()
                .map(|s| DependencyType::from_str(&s))
                .collect();

            let dep_paths_raw: Option<Vec<Option<Vec<Option<String>>>>> =
                row["dependency_paths"].value().unwrap_or(None);
            let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
                .unwrap_or_default()
                .into_iter()
                .map(|opt_arr| {
                    opt_arr.map(|arr| {
                        arr.into_iter()
                            .filter_map(|opt_s| opt_s)
                            .collect()
                    })
                })
                .collect();

            let array_keys: Option<Vec<Option<String>>> =
                row["array_match_keys"].value().unwrap_or(None);

            result = Some(Self {
                tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                view_oid: row["view_oid"].value().unwrap().unwrap(),
                entity_name: row["entity"].value().unwrap().unwrap(),
                sync_mode: 's',
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                dependency_types: dep_types,
                dependency_paths: dep_paths,
                array_match_keys: array_keys.unwrap_or_default(),
            });
            break;
        }
        Ok(result)
    })
}
```

### Step 5: Update find_dependent_tviews() in lib.rs

**File:** `src/lib.rs`

**Location:** Modify `find_dependent_tviews()` function (lines 193-224)

The query needs to fetch new columns and populate the struct:

```rust
fn find_dependent_tviews(base_table_oid: pg_sys::Oid) -> spi::Result<Vec<catalog::TviewMeta>> {
    let query = format!(
        "SELECT m.table_oid AS tview_oid, m.view_oid, m.entity, \
                m.fk_columns, m.uuid_fk_columns, \
                m.dependency_types, m.dependency_paths, m.array_match_keys \
         FROM pg_tview_meta m \
         WHERE {:?} = ANY(m.dependencies)",
        base_table_oid.as_u32()
    );

    Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut result = Vec::new();

        for row in rows {
            let fk_cols_val: Option<Vec<String>> = row["fk_columns"].value().unwrap_or(None);
            let uuid_fk_cols_val: Option<Vec<String>> = row["uuid_fk_columns"].value().unwrap_or(None);

            let dep_types_raw: Option<Vec<String>> = row["dependency_types"].value().unwrap_or(None);
            let dep_types = dep_types_raw
                .unwrap_or_default()
                .into_iter()
                .map(|s| catalog::DependencyType::from_str(&s))
                .collect();

            let dep_paths_raw: Option<Vec<Option<Vec<Option<String>>>>> =
                row["dependency_paths"].value().unwrap_or(None);
            let dep_paths: Vec<Option<Vec<String>>> = dep_paths_raw
                .unwrap_or_default()
                .into_iter()
                .map(|opt_arr| {
                    opt_arr.map(|arr| {
                        arr.into_iter()
                            .filter_map(|opt_s| opt_s)
                            .collect()
                    })
                })
                .collect();

            let array_keys: Option<Vec<Option<String>>> =
                row["array_match_keys"].value().unwrap_or(None);

            result.push(catalog::TviewMeta {
                tview_oid: row["tview_oid"].value().unwrap().unwrap(),
                view_oid: row["view_oid"].value().unwrap().unwrap(),
                entity_name: row["entity"].value().unwrap().unwrap(),
                sync_mode: 's',
                fk_columns: fk_cols_val.unwrap_or_default(),
                uuid_fk_columns: uuid_fk_cols_val.unwrap_or_default(),
                dependency_types: dep_types,
                dependency_paths: dep_paths,
                array_match_keys: array_keys.unwrap_or_default(),
            });
        }

        Ok(result)
    })
}
```

---

## Verification Commands

After implementing GREEN phase:

```bash
# 1. Build and install
export PATH="$HOME/.pgrx/17.7/pgrx-install/bin:$PATH"
cargo pgrx install --release

# 2. Run Rust unit tests
cargo test --lib

# 3. Run SQL integration test
psql -h localhost -d postgres <<EOF
DROP DATABASE IF EXISTS test_phase5_task2;
CREATE DATABASE test_phase5_task2;
\c test_phase5_task2
\i test/sql/51_metadata_enhancement.sql
EOF

# 4. Check that code compiles without warnings
cargo clippy --all-targets --all-features
```

**Expected Output:**
- ✅ All Rust tests pass
- ✅ SQL test passes (metadata columns exist and are queryable)
- ✅ No compile errors or warnings
- ✅ Existing Phase 4 tests still pass

---

## REFACTOR Phase: Improve Code Quality

### Refactor 1: Extract Array Parsing Helper

**Problem:** Parsing `TEXT[][]` from PostgreSQL is verbose and repeated 3 times.

**Solution:** Create helper function.

**File:** `src/catalog.rs`

```rust
impl TviewMeta {
    /// Helper: Parse TEXT[][] (array of array of text) from PostgreSQL SPI
    fn parse_text_array_array(
        row_value: Option<Vec<Option<Vec<Option<String>>>>>
    ) -> Vec<Option<Vec<String>>> {
        row_value
            .unwrap_or_default()
            .into_iter()
            .map(|opt_arr| {
                opt_arr.map(|arr| {
                    arr.into_iter()
                        .filter_map(|opt_s| opt_s)
                        .collect()
                })
            })
            .collect()
    }

    /// Helper: Parse TEXT[] to Vec<DependencyType>
    fn parse_dependency_types(row_value: Option<Vec<String>>) -> Vec<DependencyType> {
        row_value
            .unwrap_or_default()
            .into_iter()
            .map(|s| DependencyType::from_str(&s))
            .collect()
    }

    // Use in load_for_source() and load_by_entity():
    // let dep_types = Self::parse_dependency_types(row["dependency_types"].value().unwrap_or(None));
    // let dep_paths = Self::parse_text_array_array(row["dependency_paths"].value().unwrap_or(None));
}
```

### Refactor 2: Add Documentation Comments

**File:** `src/catalog.rs`

Add detailed comments to new fields:

```rust
pub struct TviewMeta {
    // ... existing fields ...

    /// Type of each dependency: Scalar (direct column), NestedObject (embedded JSONB),
    /// or Array (jsonb_agg aggregation).
    ///
    /// Length matches `fk_columns` and `dependencies` arrays.
    /// Used by jsonb_ivm to choose patch function (scalar/nested/array).
    pub dependency_types: Vec<DependencyType>,

    /// JSONB path for each dependency, if nested.
    /// - Scalar: None
    /// - NestedObject: Some(vec!["author"]) for { "author": {...} }
    /// - Array: Some(vec!["comments"]) for { "comments": [...] }
    ///
    /// Length matches `dependency_types`.
    pub dependency_paths: Vec<Option<Vec<String>>>,

    /// For Array dependencies, the key used to match elements (e.g., "id").
    /// Used by `jsonb_smart_patch_array(target, 'comments', '{...}', 'id')`.
    ///
    /// - Scalar/NestedObject: None
    /// - Array: Some("id") or Some("pk_comment")
    ///
    /// Length matches `dependency_types`.
    pub array_match_keys: Vec<Option<String>>,
}
```

### Refactor 3: Add Default Implementation

**File:** `src/catalog.rs`

```rust
impl Default for TviewMeta {
    fn default() -> Self {
        Self {
            tview_oid: pg_sys::Oid::INVALID,
            view_oid: pg_sys::Oid::INVALID,
            entity_name: String::new(),
            sync_mode: 's',
            fk_columns: vec![],
            uuid_fk_columns: vec![],
            dependency_types: vec![],
            dependency_paths: vec![],
            array_match_keys: vec![],
        }
    }
}
```

---

## Acceptance Criteria Checklist

After REFACTOR phase, verify:

- [ ] `DependencyType` enum exists with Scalar/NestedObject/Array
- [ ] `DependencyType::from_str()` and `as_str()` work correctly
- [ ] `TviewMeta` struct has 3 new fields
- [ ] `load_for_source()` fetches and parses new columns
- [ ] `load_by_entity()` fetches and parses new columns
- [ ] `find_dependent_tviews()` in lib.rs fetches new columns
- [ ] Rust unit tests pass (enum conversion, struct fields)
- [ ] SQL integration test passes (columns queryable)
- [ ] No breaking changes (existing Phase 4 tests still pass)
- [ ] Code compiles without warnings
- [ ] Documentation comments added

---

## Files Modified

### Modified Files:
1. `src/catalog.rs` - Add enum, update struct, update loader methods
2. `src/lib.rs` - Update `find_dependent_tviews()` query
3. `test/sql/51_metadata_enhancement.sql` (NEW) - SQL integration test

### No Changes Needed:
- `sql/pg_tviews--0.1.0.sql` - Columns already exist!
- Database schema - Already correct

---

## Rollback Plan

If Task 2 fails:
1. Remove new fields from `TviewMeta` struct
2. Revert loader queries to exclude new columns
3. Default to empty vectors in all uses
4. Phase 5 can continue with Tasks 3-4 doing detection but not storing results

---

## Next Task

After Task 2 complete → **Task 3: Dependency Type Detection**
- Implement logic to analyze SELECT statements
- Detect scalar vs nested_object vs array dependencies
- Extract JSONB paths from `jsonb_build_object()` calls
- Populate the new metadata columns during CREATE TVIEW

---

## DO NOT

- ❌ Modify SQL schema (columns already exist!)
- ❌ Break existing Phase 4 functionality
- ❌ Add detection logic yet (that's Task 3)
- ❌ Add jsonb_ivm function calls yet (that's Task 4)
- ❌ Change default values in database (use Rust defaults for empty fields)

---

## Notes

- **TEXT[][] Parsing:** PostgreSQL returns `Vec<Option<Vec<Option<String>>>>` for TEXT[][]
- **Performance:** These are small arrays (typically 1-3 elements), no optimization needed
- **Backward Compat:** Existing TVIEWs will have empty arrays (defaults from SQL schema)
- **Testing Strategy:** Unit tests for enum, integration test for SQL columns, Phase 4 tests for regression
