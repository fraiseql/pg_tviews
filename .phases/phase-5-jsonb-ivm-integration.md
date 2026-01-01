# Phase 5: jsonb_delta Integration for Performance Optimization

**Status:** Ready to implement
**Duration:** 7-10 days
**Complexity:** HIGH (Performance optimization with external dependency)
**Prerequisites:** ✅ Phase 4 complete (basic refresh & cascade working)

---

## Overview

Phase 5 integrates the **jsonb_delta v0.3.1** extension to provide **2-3× faster cascade updates** through surgical JSONB patching instead of full document replacement.

**Current Performance (Phase 4):**
- Single row refresh: Full document replacement
- Nested updates: Re-query and replace entire JSONB
- Array updates: Re-aggregate and replace entire array
- Cascade (100 rows): ~870ms (baseline)

**Target Performance (Phase 5):**
- Single row refresh: Surgical shallow merge with `jsonb_smart_patch_scalar()`
- Nested updates: Path-based merge with `jsonb_smart_patch_nested()`
- Array updates: Element-level updates with `jsonb_smart_patch_array()`
- Cascade (100 rows): **~600ms (1.45× faster)** to **~400ms (2.2× faster)**

---

## Current Status

### ✅ What's Working (Phase 4 Baseline)
- Basic refresh: `apply_patch()` does full JSONB replacement
- Cascade propagation: Walks dependency graph correctly
- FK extraction: Knows which entities changed
- Tests passing: All Phase 4 tests working

### ⏳ What Needs Implementation
1. **Install jsonb_delta dependency**
2. **Detect update type** (scalar vs nested vs array)
3. **Smart function dispatch** based on dependency metadata
4. **Metadata enhancement** to track JSONB paths
5. **Array handling** for jsonb_agg compositions
6. **Performance benchmarking**

---

## Implementation Tasks

### Task 1: Add jsonb_delta Dependency & Installation

**Goal:** Make jsonb_delta available to pg_tviews users.

#### Step 1a: Update Documentation

**File:** `README.md`

Add dependency section:
```markdown
## Dependencies

pg_tviews requires the **jsonb_delta** extension for optimal performance:

```bash
# Install jsonb_delta
git clone https://github.com/fraiseql/jsonb_delta.git
cd jsonb_delta
cargo pgrx install --release

# Then install pg_tviews
cd ../pg_tviews
cargo pgrx install --release

# In PostgreSQL:
CREATE EXTENSION jsonb_delta;  -- Required
CREATE EXTENSION pg_tviews;
```

**Performance Impact:**
- Without jsonb_delta: Basic functionality works (full document replacement)
- With jsonb_delta: **1.5-3× faster cascades** (surgical JSONB updates)
```

#### Step 1b: Add Runtime Dependency Check

**File:** `src/lib.rs`

Add function to check if jsonb_delta is installed:

```rust
/// Check if jsonb_delta extension is available
fn check_jsonb_delta_available() -> bool {
    Spi::connect(|client| {
        let result = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')",
            None,
            None,
        );

        match result {
            Ok(mut rows) => {
                if let Some(row) = rows.next() {
                    return Ok(row[1].value::<bool>().unwrap_or(Some(false)).unwrap_or(false));
                }
                Ok(false)
            }
            Err(_) => Ok(false),
        }
    }).unwrap_or(false)
}
```

Call in `_PG_init()`:
```rust
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing code ...

    if !check_jsonb_delta_available() {
        warning!(
            "jsonb_delta extension not found. \
             pg_tviews will work but with reduced performance. \
             Install jsonb_delta for 1.5-3× faster cascades: \
             https://github.com/fraiseql/jsonb_delta"
        );
    } else {
        info!("jsonb_delta extension detected - performance optimizations enabled");
    }
}
```

**Test:**
```sql
-- test/sql/50_jsonb_delta_detection.sql
BEGIN;
    -- Should warn if jsonb_delta not installed
    CREATE EXTENSION pg_tviews;

    -- Install jsonb_delta
    CREATE EXTENSION jsonb_delta;

    -- Reload pg_tviews (or check logs)
    -- Should show: "jsonb_delta extension detected"
ROLLBACK;
```

---

### Task 2: Enhance Metadata to Track Dependency Types

**Goal:** Store information about how each FK relationship manifests in JSONB structure.

#### Current Metadata Schema:
```sql
CREATE TABLE pg_tview_meta (
    entity text PRIMARY KEY,
    view_oid oid NOT NULL,
    tview_oid oid NOT NULL,
    definition text NOT NULL,
    dependencies oid[] NOT NULL,
    fk_columns text[] NOT NULL,
    uuid_fk_columns text[] NOT NULL
);
```

#### Enhanced Metadata Schema:

**File:** `sql/pg_tviews--0.2.0.sql` (migration)

```sql
-- Migration: Add dependency type tracking for jsonb_delta optimization
ALTER TABLE pg_tview_meta
ADD COLUMN dependency_types text[],     -- e.g. ['scalar', 'nested_object']
ADD COLUMN dependency_paths text[][],   -- e.g. [NULL, ['author']]
ADD COLUMN array_match_keys text[];     -- e.g. [NULL, NULL]

COMMENT ON COLUMN pg_tview_meta.dependency_types IS
'Type of each FK dependency: scalar, nested_object, or array';

COMMENT ON COLUMN pg_tview_meta.dependency_paths IS
'JSONB path for each FK (NULL for scalar, [key] for nested, [array_name] for arrays)';

COMMENT ON COLUMN pg_tview_meta.array_match_keys IS
'Match key for array dependencies (typically "id"), NULL for non-arrays';
```

#### Update Rust Metadata Struct

**File:** `src/catalog.rs`

```rust
pub struct TviewMeta {
    pub entity_name: String,
    pub view_oid: Oid,
    pub tview_oid: Oid,
    pub definition: String,
    pub dependencies: Vec<Oid>,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,

    // NEW: jsonb_delta optimization metadata
    pub dependency_types: Vec<DependencyType>,
    pub dependency_paths: Vec<Option<Vec<String>>>,
    pub array_match_keys: Vec<Option<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DependencyType {
    Scalar,         // Direct column from base table (no FK join)
    NestedObject,   // Embedded v_other.data in jsonb_build_object
    Array,          // jsonb_agg(v_child.data) creates array
}

impl DependencyType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "scalar" => DependencyType::Scalar,
            "nested_object" => DependencyType::NestedObject,
            "array" => DependencyType::Array,
            _ => DependencyType::Scalar, // default
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DependencyType::Scalar => "scalar",
            DependencyType::NestedObject => "nested_object",
            DependencyType::Array => "array",
        }
    }
}
```

---

### Task 3: Implement Dependency Type Detection

**Goal:** Analyze SELECT statement to determine how each FK manifests in JSONB.

**File:** `src/schema/analyzer.rs` (NEW)

```rust
use crate::catalog::DependencyType;

pub struct DependencyInfo {
    pub dep_type: DependencyType,
    pub jsonb_path: Option<Vec<String>>,
    pub array_match_key: Option<String>,
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

fn detect_dependency_type(select_sql: &str, fk_col: &str) -> DependencyInfo {
    // Heuristic 1: Check for jsonb_build_object with v_*.data pattern
    // Pattern: jsonb_build_object(..., 'key_name', v_something.data, ...)

    let nested_pattern = format!(r"'(\w+)',\s*v_\w+\.data");
    if let Some(captures) = regex::Regex::new(&nested_pattern)
        .ok()
        .and_then(|re| re.captures(select_sql))
    {
        let key_name = captures.get(1).map(|m| m.as_str().to_string());
        return DependencyInfo {
            dep_type: DependencyType::NestedObject,
            jsonb_path: key_name.map(|k| vec![k]),
            array_match_key: None,
        };
    }

    // Heuristic 2: Check for jsonb_agg pattern
    // Pattern: jsonb_agg(v_something.data ORDER BY ...)
    if select_sql.contains("jsonb_agg(") && select_sql.contains("v_") {
        // Extract array key from surrounding jsonb_build_object
        // Pattern: 'array_name', jsonb_agg(v_*.data)
        let array_pattern = r"'(\w+)',\s*jsonb_agg\(";
        if let Some(captures) = regex::Regex::new(array_pattern)
            .ok()
            .and_then(|re| re.captures(select_sql))
        {
            let array_name = captures.get(1).map(|m| m.as_str().to_string());
            return DependencyInfo {
                dep_type: DependencyType::Array,
                jsonb_path: array_name.map(|k| vec![k]),
                array_match_key: Some("id".to_string()), // convention
            };
        }
    }

    // Default: Scalar (direct column reference)
    DependencyInfo {
        dep_type: DependencyType::Scalar,
        jsonb_path: None,
        array_match_key: None,
    }
}
```

**Test:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_nested_object() {
        let sql = "SELECT pk_post, jsonb_build_object('id', id, 'author', v_user.data) AS data";
        let fk_cols = vec!["fk_user".to_string()];
        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps[0].dep_type, DependencyType::NestedObject);
        assert_eq!(deps[0].jsonb_path, Some(vec!["author".to_string()]));
    }

    #[test]
    fn test_detect_array() {
        let sql = "SELECT jsonb_build_object('posts', jsonb_agg(v_post.data)) AS data";
        let fk_cols = vec!["fk_post".to_string()];
        let deps = analyze_dependencies(sql, &fk_cols);

        assert_eq!(deps[0].dep_type, DependencyType::Array);
        assert_eq!(deps[0].jsonb_path, Some(vec!["posts".to_string()]));
        assert_eq!(deps[0].array_match_key, Some("id".to_string()));
    }
}
```

---

### Task 4: Update apply_patch() to Use jsonb_delta Functions

**Goal:** Replace full document replacement with surgical JSONB updates.

**File:** `src/refresh.rs`

Replace `apply_patch()` function:

```rust
use crate::catalog::{DependencyType, TviewMeta};

/// Apply JSON patch using jsonb_delta smart functions
fn apply_patch(row: &ViewRow, meta: &TviewMeta, changed_fk: Option<&str>) -> spi::Result<()> {
    let tv_name = relname_from_oid(row.tview_oid)?;
    let pk_col = format!("pk_{}", row.entity_name);

    // Check if jsonb_delta is available
    let has_jsonb_delta = check_jsonb_delta_available();

    if !has_jsonb_delta || changed_fk.is_none() {
        // Fallback: Full document replacement (Phase 4 behavior)
        return apply_patch_full_replace(row, &tv_name, &pk_col);
    }

    // Determine which jsonb_delta function to use
    let changed_fk = changed_fk.unwrap();
    let dep_idx = meta.fk_columns.iter().position(|fk| fk == changed_fk);

    let dep_idx = match dep_idx {
        Some(idx) => idx,
        None => {
            // FK not found, fall back to full replace
            return apply_patch_full_replace(row, &tv_name, &pk_col);
        }
    };

    let dep_type = &meta.dependency_types[dep_idx];
    let dep_path = &meta.dependency_paths[dep_idx];

    // Dispatch to appropriate jsonb_delta function
    match dep_type {
        DependencyType::Scalar => {
            apply_patch_scalar(row, &tv_name, &pk_col)
        }
        DependencyType::NestedObject => {
            apply_patch_nested(row, &tv_name, &pk_col, dep_path.as_ref().unwrap())
        }
        DependencyType::Array => {
            let match_key = &meta.array_match_keys[dep_idx];
            apply_patch_array(row, &tv_name, &pk_col, dep_path.as_ref().unwrap(), match_key.as_ref().unwrap())
        }
    }
}

/// Scalar update using jsonb_smart_patch_scalar
fn apply_patch_scalar(row: &ViewRow, tv_name: &str, pk_col: &str) -> spi::Result<()> {
    let sql = format!(
        "UPDATE {} \
         SET data = jsonb_smart_patch_scalar(data, $1), updated_at = now() \
         WHERE {} = $2",
        tv_name, pk_col
    );

    execute_update(&sql, &row.data, row.pk)
}

/// Nested object update using jsonb_smart_patch_nested
fn apply_patch_nested(
    row: &ViewRow,
    tv_name: &str,
    pk_col: &str,
    path: &[String],
) -> spi::Result<()> {
    let sql = format!(
        "UPDATE {} \
         SET data = jsonb_smart_patch_nested(data, $1, $2), updated_at = now() \
         WHERE {} = $3",
        tv_name, pk_col
    );

    // Convert path to PostgreSQL text array
    let path_array = format!("ARRAY[{}]", path.iter()
        .map(|p| format!("'{}'", p))
        .collect::<Vec<_>>()
        .join(","));

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), row.data.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTARRAYOID), path_array.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

/// Array element update using jsonb_smart_patch_array
fn apply_patch_array(
    row: &ViewRow,
    tv_name: &str,
    pk_col: &str,
    path: &[String],
    match_key: &str,
) -> spi::Result<()> {
    // Extract match value from row.data using match_key
    let match_value = extract_match_value(&row.data, match_key)?;

    let sql = format!(
        "UPDATE {} \
         SET data = jsonb_smart_patch_array(data, $1, $2, $3, $4), updated_at = now() \
         WHERE {} = $5",
        tv_name, pk_col
    );

    let array_path = &path[0]; // First element is array name

    Spi::connect(|mut client| {
        client.update(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), row.data.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), array_path.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), match_key.to_string().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), match_value.into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), row.pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

/// Fallback: Full document replacement (Phase 4 behavior)
fn apply_patch_full_replace(row: &ViewRow, tv_name: &str, pk_col: &str) -> spi::Result<()> {
    let sql = format!(
        "UPDATE {} \
         SET data = $1, updated_at = now() \
         WHERE {} = $2",
        tv_name, pk_col
    );

    execute_update(&sql, &row.data, row.pk)
}

fn execute_update(sql: &str, data: &JsonB, pk: i64) -> spi::Result<()> {
    Spi::connect(|mut client| {
        client.update(
            sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), data.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::INT8OID), pk.into_datum()),
            ]),
        )?;
        Ok(())
    })
}

fn extract_match_value(data: &JsonB, match_key: &str) -> spi::Result<JsonB> {
    // Use jsonb_extract_id from jsonb_delta
    let sql = format!("SELECT jsonb_extract_id($1, $2)");

    Spi::connect(|client| {
        let result = client.select(
            &sql,
            None,
            Some(vec![
                (PgOid::BuiltIn(PgBuiltInOids::JSONBOID), data.clone().into_datum()),
                (PgOid::BuiltIn(PgBuiltInOids::TEXTOID), match_key.to_string().into_datum()),
            ]),
        )?;

        // Extract text result and convert to JsonB
        for row in result {
            if let Some(id_text) = row[1].value::<String>()? {
                return Ok(JsonB(serde_json::json!(id_text)));
            }
        }

        error!("Could not extract match value for key: {}", match_key)
    })
}

fn check_jsonb_delta_available() -> bool {
    // Use cached result from _PG_init() check
    // For simplicity, re-check here (could be optimized with static cache)
    Spi::connect(|client| {
        let result = client.select(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'jsonb_delta')",
            None,
            None,
        );

        match result {
            Ok(mut rows) => {
                if let Some(row) = rows.next() {
                    return Ok(row[1].value::<bool>().unwrap_or(Some(false)).unwrap_or(false));
                }
                Ok(false)
            }
            Err(_) => Ok(false),
        }
    }).unwrap_or(false)
}
```

---

### Task 5: Update refresh_pk() to Pass Changed FK Context

**Goal:** Tell apply_patch() which FK triggered the refresh.

**File:** `src/refresh.rs`

Update function signature:

```rust
pub fn refresh_pk(source_oid: Oid, pk: i64, changed_fk: Option<&str>) -> spi::Result<()> {
    let meta = TviewMeta::load_for_source(source_oid)?;
    let meta = match meta {
        Some(m) => m,
        None => {
            error!("No TVIEW metadata for source_oid: {:?}", source_oid);
        }
    };

    let view_row = recompute_view_row(&meta, pk)?;
    apply_patch(&view_row, &meta, changed_fk)?;  // Pass changed_fk
    propagate_from_row(&view_row)?;

    Ok(())
}
```

Update callers in `src/lib.rs`:

```rust
#[pg_extern]
fn pg_tviews_cascade(
    source_table_oid: pg_sys::Oid,
    pk_new: Option<i64>,
    pk_old: Option<i64>,
    _cascade_depth: i32,
) -> spi::Result<()> {
    if let Some(pk) = pk_new {
        // TODO: Detect which FK changed from trigger context
        refresh_pk(source_table_oid, pk, None)?;
    }

    if let Some(pk) = pk_old {
        if Some(pk) != pk_new {
            refresh_pk(source_table_oid, pk, None)?;
        }
    }

    Ok(())
}
```

---

### Task 6: Performance Benchmarking & Testing

**Goal:** Verify 1.5-3× speedup with jsonb_delta.

#### Benchmark Test

**File:** `test/sql/51_jsonb_delta_performance.sql`

```sql
-- Phase 5 Task 6: Performance benchmark with jsonb_delta
BEGIN;
    SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;

    CREATE EXTENSION IF NOT EXISTS jsonb_delta;
    CREATE EXTENSION IF NOT EXISTS pg_tviews;

    -- Setup: Company → User → Post hierarchy
    CREATE TABLE tb_company (pk_company INT PRIMARY KEY, id UUID, name TEXT);
    CREATE TABLE tb_user (pk_user INT PRIMARY KEY, id UUID, fk_company INT, name TEXT, email TEXT);
    CREATE TABLE tb_post (pk_post INT PRIMARY KEY, id UUID, fk_user INT, title TEXT, content TEXT);

    -- Insert test data
    INSERT INTO tb_company VALUES (1, gen_random_uuid(), 'ACME Corp');
    INSERT INTO tb_user
    SELECT i, gen_random_uuid(), 1, 'User ' || i, 'user' || i || '@example.com'
    FROM generate_series(1, 10) i;
    INSERT INTO tb_post
    SELECT i, gen_random_uuid(), ((i-1) % 10) + 1, 'Post ' || i, 'Content ' || i
    FROM generate_series(1, 100) i;

    -- Create TVIEWs
    SELECT pg_tviews_create('company', $$
        SELECT pk_company, id, jsonb_build_object('id', id, 'name', name) AS data
        FROM tb_company
    $$);

    SELECT pg_tviews_create('user', $$
        SELECT u.pk_user, u.id, u.fk_company, c.id AS company_id,
               jsonb_build_object(
                   'id', u.id,
                   'name', u.name,
                   'email', u.email,
                   'company', v_company.data
               ) AS data
        FROM tb_user u
        JOIN v_company ON v_company.pk_company = u.fk_company
    $$);

    SELECT pg_tviews_create('post', $$
        SELECT p.pk_post, p.id, p.fk_user, u.id AS user_id,
               jsonb_build_object(
                   'id', p.id,
                   'title', p.title,
                   'content', p.content,
                   'author', v_user.data
               ) AS data
        FROM tb_post p
        JOIN v_user ON v_user.pk_user = p.fk_user
    $$);

    -- Benchmark: Update company name (cascades to 10 users + 100 posts)
    \timing on
    UPDATE tb_company SET name = 'ACME Corporation Updated' WHERE pk_company = 1;
    \timing off

    -- Verify cascade worked
    SELECT (data->>'name') AS company_name FROM tv_company WHERE pk_company = 1;
    -- Expected: 'ACME Corporation Updated'

    SELECT (data->'company'->>'name') AS company_in_user FROM tv_user WHERE pk_user = 1;
    -- Expected: 'ACME Corporation Updated'

    SELECT (data->'author'->'company'->>'name') AS company_in_post FROM tv_post WHERE pk_post = 1;
    -- Expected: 'ACME Corporation Updated'

    -- Performance check: Should be < 100ms with jsonb_delta (vs ~150ms without)

ROLLBACK;
```

#### Expected Results:

| Scenario | Without jsonb_delta | With jsonb_delta | Speedup |
|----------|------------------|----------------|---------|
| Single nested update | 2.5ms | 1.2ms | **2.1×** |
| 10-user cascade | 18ms | 11ms | **1.6×** |
| 100-post cascade | 150ms | 85ms | **1.8×** |
| Deep cascade (3 levels) | 220ms | 100ms | **2.2×** |

---

### Task 7: Array Handling (Advanced)

**Goal:** Support jsonb_agg() patterns for array compositions.

**Example:**
```sql
CREATE TABLE tv_feed AS
SELECT 1 AS pk_feed,
       jsonb_build_object(
           'posts', jsonb_agg(v_post.data ORDER BY v_post.id)
       ) AS data
FROM v_post;
```

**Challenge:** When a single post updates, need to update just that element in the array.

**Implementation:**
- Detect `jsonb_agg()` pattern in Task 3
- Store array path + match key in metadata
- Use `jsonb_smart_patch_array()` in Task 4

**Test:**
```sql
-- test/sql/52_jsonb_delta_array_update.sql
-- Test array element updates with jsonb_smart_patch_array
```

**Note:** This is an advanced optimization. Phase 5 can initially focus on scalar and nested object updates, with array handling as a Phase 5.1 enhancement.

---

## Acceptance Criteria

### Functional
- [ ] jsonb_delta dependency documented and checked at runtime
- [ ] Metadata stores dependency_types, dependency_paths, array_match_keys
- [ ] Dependency type detection working for scalar, nested_object
- [ ] apply_patch() uses jsonb_smart_patch_scalar() for scalar updates
- [ ] apply_patch() uses jsonb_smart_patch_nested() for nested object updates
- [ ] Fallback to full replacement when jsonb_delta not available
- [ ] All Phase 4 tests still passing

### Performance
- [ ] Single nested update: < 1.5ms (was ~2.5ms)
- [ ] 100-row cascade: < 100ms (was ~150ms)
- [ ] Deep cascade (3 levels): < 120ms (was ~220ms)
- [ ] Overall speedup: **1.5-2.2× faster**

### Quality
- [ ] Rust unit tests for dependency detection
- [ ] SQL integration test with performance benchmark
- [ ] Documentation updated with jsonb_delta installation
- [ ] Warning shown if jsonb_delta not installed
- [ ] No regressions in functionality

---

## Files to Create/Modify

### New Files
1. **`.phases/phase-5-jsonb-ivm-integration.md`** - This file
2. **`src/schema/analyzer.rs`** - Dependency type detection
3. **`sql/pg_tviews--0.2.0.sql`** - Migration for new metadata columns
4. **`test/sql/50_jsonb_delta_detection.sql`** - Extension detection test
5. **`test/sql/51_jsonb_delta_performance.sql`** - Performance benchmark
6. **`test/sql/52_jsonb_delta_array_update.sql`** - Array handling (Phase 5.1)

### Modified Files
1. **`README.md`** - Add jsonb_delta dependency documentation
2. **`Cargo.toml`** - Bump version to 0.2.0
3. **`src/lib.rs`** - Add jsonb_delta availability check
4. **`src/catalog.rs`** - Add dependency_types fields to TviewMeta
5. **`src/refresh.rs`** - Rewrite apply_patch() with jsonb_delta dispatch
6. **`src/ddl/create.rs`** - Call dependency analyzer, store metadata

---

## Rollback Plan

If Phase 5 fails or jsonb_delta causes issues:

1. **Keep fallback path**: apply_patch_full_replace() always available
2. **Disable optimization**: Set flag `use_jsonb_delta = false`
3. **Revert metadata**: Columns are optional, can be NULL
4. **Phase 4 still works**: All existing functionality preserved

Can rollback to Phase 4 behavior by simply not installing jsonb_delta.

---

## Timeline Estimate

| Task | Duration | Dependencies |
|------|----------|--------------|
| 1. Dependency setup | 1 day | None |
| 2. Metadata enhancement | 1 day | Task 1 |
| 3. Dependency detection | 2-3 days | Task 2 |
| 4. Smart apply_patch() | 2-3 days | Tasks 2, 3 |
| 5. Context passing | 1 day | Task 4 |
| 6. Benchmarking | 1-2 days | Task 5 |
| 7. Array handling (optional) | 2-3 days | Task 4 |

**Total: 7-10 days** (excluding optional array handling)

---

## Success Metrics

**Phase 5 is complete when:**

✅ jsonb_delta dependency documented and optional
✅ Metadata tracks dependency types (scalar, nested_object)
✅ apply_patch() uses smart functions when available
✅ Performance benchmark shows **1.5-2× speedup**
✅ All Phase 4 tests still pass
✅ Fallback works when jsonb_delta not installed
✅ No breaking changes to existing API

---

## Next Steps After Phase 5

**Phase 5.1:** Array handling optimization (jsonb_agg patterns)
**Phase 6:** Production hardening (error telemetry, monitoring, logging)
**Phase 7:** Schema change detection and auto-rebuild
**Phase 8:** Batch cascade optimization for large-scale updates

---

## References

- [jsonb_delta GitHub](https://github.com/fraiseql/jsonb_delta) - v0.3.1 documentation
- Phase 4 Plan: `docs/archive/PHASE_4_PLAN.md`
- PRD v2: `PRD_v2.md` - jsonb_delta integration strategy
- Benchmark expectations: 1.5-3× faster cascades (validated by jsonb_delta tests)
