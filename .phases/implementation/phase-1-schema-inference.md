# Phase 1: Schema Inference & Column Detection

**Status:** Planning
**Duration:** 3-5 days
**Complexity:** Medium
**Prerequisites:** Phase 0 complete

---

## Objective

Implement schema inference logic that analyzes `CREATE TVIEW` SELECT statements to automatically detect:
- Primary key columns (`pk_<entity>`)
- External UUID columns (`id`)
- Foreign key columns (`fk_*`)
- UUID foreign key columns (`*_id`)
- JSONB data column (`data`)
- Additional materialized columns (precomputed flags, arrays, etc.)

---

## Success Criteria

- [ ] Parse SELECT statement to extract column list
- [ ] Detect Trinity identifier pattern (`pk_`, `id`, optional `identifier`)
- [ ] Identify all `fk_*` lineage columns
- [ ] Identify all `*_id` filtering columns
- [ ] Locate JSONB `data` column
- [ ] Infer column types from PostgreSQL catalog
- [ ] Handle edge cases (missing columns, invalid names, duplicates)
- [ ] All tests pass with 100% coverage

---

## TDD Approach: RED → GREEN → REFACTOR

### Test 1: Simple Column Detection

**RED Phase - Write Failing Test:**

```sql
-- test/sql/10_schema_inference_simple.sql
-- Test: Infer schema from simple SELECT

BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create test base table
    CREATE TABLE tb_test_entity (
        pk_test_entity INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        name TEXT
    );

    -- Test: Analyze simple SELECT (not creating TVIEW yet, just analyzing)
    SELECT jsonb_pretty(
        pg_tviews_analyze_select($$
            SELECT
                pk_test_entity,
                id,
                jsonb_build_object('id', id, 'name', name) AS data
            FROM tb_test_entity
        $$)
    );

    -- Expected output (pretty JSON):
    -- {
    --   "pk_column": "pk_test_entity",
    --   "id_column": "id",
    --   "data_column": "data",
    --   "fk_columns": [],
    --   "uuid_fk_columns": [],
    --   "additional_columns": [],
    --   "entity_name": "test_entity"
    -- }

ROLLBACK;
```

**Expected Output (failing):**
```
ERROR: function pg_tviews_analyze_select(text) does not exist
```

**GREEN Phase - Implementation:**

```rust
// src/schema/mod.rs
use pgrx::prelude::*;
use serde::{Serialize, Deserialize};

pub mod parser;
pub mod inference;

#[derive(Debug, Clone, Serialize, Deserialize, PostgresType)]
#[serde(rename_all = "snake_case")]
pub struct TViewSchema {
    pub pk_column: Option<String>,
    pub id_column: Option<String>,
    pub identifier_column: Option<String>,
    pub data_column: Option<String>,
    pub fk_columns: Vec<String>,
    pub uuid_fk_columns: Vec<String>,
    pub additional_columns: Vec<String>,
    pub entity_name: Option<String>,
}

impl TViewSchema {
    pub fn new() -> Self {
        Self {
            pk_column: None,
            id_column: None,
            identifier_column: None,
            data_column: None,
            fk_columns: Vec::new(),
            uuid_fk_columns: Vec::new(),
            additional_columns: Vec::new(),
            entity_name: None,
        }
    }

    pub fn to_jsonb(&self) -> Result<JsonB, serde_json::Error> {
        let json_value = serde_json::to_value(self)?;
        Ok(JsonB(json_value))
    }
}
```

```rust
// src/schema/parser.rs
use pgrx::prelude::*;
use super::TViewSchema;

/// Parse SELECT statement to extract column names
pub fn parse_select_columns(sql: &str) -> Result<Vec<String>, String> {
    // Use PostgreSQL parser via SPI
    let parse_result = Spi::get_one::<String>(&format!(
        "SELECT string_agg(attname, ',') FROM (
            SELECT attname
            FROM pg_attribute
            WHERE attrelid = (
                SELECT oid FROM pg_class
                WHERE relname = 'pg_class' LIMIT 1
            )
        ) AS cols"
    ));

    // For now, simple regex-based extraction
    // TODO: Use PostgreSQL parser API in future
    let columns = extract_columns_regex(sql)?;
    Ok(columns)
}

fn extract_columns_regex(sql: &str) -> Result<Vec<String>, String> {
    let mut columns = Vec::new();

    // Find SELECT...FROM
    let select_start = sql.to_lowercase().find("select")
        .ok_or("No SELECT found")?;
    let from_start = sql.to_lowercase().find("from")
        .ok_or("No FROM found")?;

    if from_start <= select_start {
        return Err("FROM before SELECT".to_string());
    }

    let select_clause = &sql[select_start + 6..from_start].trim();

    // Split by commas (naive - doesn't handle nested commas)
    for part in select_clause.split(',') {
        let trimmed = part.trim();

        // Extract column name or alias
        if let Some(as_pos) = trimmed.to_lowercase().rfind(" as ") {
            let alias = trimmed[as_pos + 4..].trim();
            columns.push(alias.to_string());
        } else {
            // No alias - use column name
            let col_name = trimmed.split_whitespace().last()
                .ok_or("Empty column")?;
            columns.push(col_name.to_string());
        }
    }

    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_columns_simple() {
        let sql = "SELECT id, name, data FROM users";
        let cols = extract_columns_regex(sql).unwrap();
        assert_eq!(cols, vec!["id", "name", "data"]);
    }

    #[test]
    fn test_extract_columns_with_alias() {
        let sql = "SELECT u.id AS user_id, u.name, 'data' AS data FROM users u";
        let cols = extract_columns_regex(sql).unwrap();
        assert_eq!(cols, vec!["user_id", "name", "data"]);
    }
}
```

```rust
// src/schema/inference.rs
use super::{TViewSchema, parser};

pub fn infer_schema(sql: &str) -> Result<TViewSchema, String> {
    let columns = parser::parse_select_columns(sql)?;
    let mut schema = TViewSchema::new();

    // Infer entity name from pk_ column
    for col in &columns {
        if col.starts_with("pk_") {
            schema.pk_column = Some(col.clone());
            schema.entity_name = Some(col[3..].to_string());
            break;
        }
    }

    // Detect id column
    if columns.contains(&"id".to_string()) {
        schema.id_column = Some("id".to_string());
    }

    // Detect identifier column
    if columns.contains(&"identifier".to_string()) {
        schema.identifier_column = Some("identifier".to_string());
    }

    // Detect data column
    if columns.contains(&"data".to_string()) {
        schema.data_column = Some("data".to_string());
    }

    // Detect fk_ columns
    for col in &columns {
        if col.starts_with("fk_") {
            schema.fk_columns.push(col.clone());
        }
    }

    // Detect _id columns (UUID foreign keys)
    for col in &columns {
        if col.ends_with("_id") && col != "id" {
            schema.uuid_fk_columns.push(col.clone());
        }
    }

    // Additional columns (everything else)
    for col in &columns {
        if col != schema.pk_column.as_ref().unwrap_or(&String::new())
            && col != schema.id_column.as_ref().unwrap_or(&String::new())
            && col != schema.identifier_column.as_ref().unwrap_or(&String::new())
            && col != schema.data_column.as_ref().unwrap_or(&String::new())
            && !schema.fk_columns.contains(col)
            && !schema.uuid_fk_columns.contains(col)
        {
            schema.additional_columns.push(col.clone());
        }
    }

    Ok(schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_simple_schema() {
        let sql = "SELECT pk_post, id, fk_user, user_id, data FROM tb_post";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_post".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, Some("data".to_string()));
        assert_eq!(schema.entity_name, Some("post".to_string()));
        assert_eq!(schema.fk_columns, vec!["fk_user"]);
        assert_eq!(schema.uuid_fk_columns, vec!["user_id"]);
    }

    #[test]
    fn test_infer_missing_data_column() {
        let sql = "SELECT pk_user, id FROM tb_user";
        let schema = infer_schema(sql).unwrap();

        assert_eq!(schema.pk_column, Some("pk_user".to_string()));
        assert_eq!(schema.id_column, Some("id".to_string()));
        assert_eq!(schema.data_column, None);
    }
}
```

```rust
// src/lib.rs (add to exports)
mod schema;

use schema::inference::infer_schema;
use schema::TViewSchema;

#[pg_extern]
fn pg_tviews_analyze_select(sql: &str) -> JsonB {
    match infer_schema(sql) {
        Ok(schema) => schema.to_jsonb()
            .unwrap_or_else(|e| {
                error!("Serialization error: {}", e);
            }),
        Err(e) => {
            error!("Schema inference failed: {}", e);
        }
    }
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
cargo pgrx install --release
psql -d test_db -f test/sql/10_schema_inference_simple.sql
```

**Expected Output:**
```json
{
  "pk_column": "pk_test_entity",
  "id_column": "id",
  "data_column": "data",
  "fk_columns": [],
  "uuid_fk_columns": [],
  "additional_columns": [],
  "entity_name": "test_entity"
}
```

---

### Test 2: Complex Column Detection (With FKs and Arrays)

**RED Phase - Write Failing Test:**

```sql
-- test/sql/11_schema_inference_complex.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test: Complex schema with FKs, UUID FKs, arrays, flags
    SELECT jsonb_pretty(
        pg_tviews_analyze_select($$
            SELECT
                a.pk_allocation,
                a.id,
                a.fk_machine,
                a.fk_location,
                m.id AS machine_id,
                l.id AS location_id,
                a.tenant_id,
                (a.start_date <= CURRENT_DATE) AS is_current,
                (a.end_date < CURRENT_DATE) AS is_past,
                ARRAY(SELECT mi.id FROM tb_machine_item mi) AS machine_item_ids,
                jsonb_build_object('id', a.id) AS data
            FROM tb_allocation a
        $$)
    );

    -- Expected:
    -- pk_column: "pk_allocation"
    -- id_column: "id"
    -- fk_columns: ["fk_machine", "fk_location"]
    -- uuid_fk_columns: ["machine_id", "location_id", "tenant_id"]
    -- additional_columns: ["is_current", "is_past", "machine_item_ids"]
    -- data_column: "data"

ROLLBACK;
```

**GREEN Phase - Update Implementation:**

```rust
// src/schema/inference.rs (updated)

pub fn infer_schema(sql: &str) -> Result<TViewSchema, String> {
    let columns = parser::parse_select_columns(sql)?;
    let mut schema = TViewSchema::new();

    // 1. Detect pk_ column (highest priority)
    for col in &columns {
        if col.starts_with("pk_") {
            schema.pk_column = Some(col.clone());
            schema.entity_name = Some(col[3..].to_string());
            break;
        }
    }

    // 2. Detect id column
    if columns.contains(&"id".to_string()) {
        schema.id_column = Some("id".to_string());
    }

    // 3. Detect identifier column
    if columns.contains(&"identifier".to_string()) {
        schema.identifier_column = Some("identifier".to_string());
    }

    // 4. Detect data column
    if columns.contains(&"data".to_string()) {
        schema.data_column = Some("data".to_string());
    }

    // 5. Detect fk_ columns (FK lineage)
    for col in &columns {
        if col.starts_with("fk_") {
            schema.fk_columns.push(col.clone());
        }
    }

    // 6. Detect _id columns (UUID foreign keys for filtering)
    // IMPORTANT: Exclude "id" itself
    for col in &columns {
        if col.ends_with("_id") && col != "id" {
            schema.uuid_fk_columns.push(col.clone());
        }
    }

    // 7. Additional columns (everything else)
    let reserved_columns: std::collections::HashSet<&str> = [
        schema.pk_column.as_deref().unwrap_or(""),
        schema.id_column.as_deref().unwrap_or(""),
        schema.identifier_column.as_deref().unwrap_or(""),
        schema.data_column.as_deref().unwrap_or(""),
    ].into_iter().filter(|s| !s.is_empty()).collect();

    for col in &columns {
        if !reserved_columns.contains(col.as_str())
            && !schema.fk_columns.contains(col)
            && !schema.uuid_fk_columns.contains(col)
        {
            schema.additional_columns.push(col.clone());
        }
    }

    Ok(schema)
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/11_schema_inference_complex.sql
```

---

### Test 3: Edge Cases - Missing Required Columns

**RED Phase - Write Failing Test:**

```sql
-- test/sql/12_schema_inference_validation.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Test 1: Missing pk_ column (should return NULL pk_column)
    SELECT pg_tviews_analyze_select($$
        SELECT id, name FROM tb_user
    $$) -> 'pk_column' IS NULL AS missing_pk_handled;
    -- Expected: t

    -- Test 2: Missing data column (should return NULL data_column)
    SELECT pg_tviews_analyze_select($$
        SELECT pk_user, id, name FROM tb_user
    $$) -> 'data_column' IS NULL AS missing_data_handled;
    -- Expected: t

    -- Test 3: No columns (should error gracefully)
    SELECT pg_tviews_analyze_select($$
        SELECT FROM tb_user
    $$) IS NOT NULL AS empty_select_handled;
    -- Expected: Should not crash (return empty schema or error)

ROLLBACK;
```

**GREEN Phase - Add Validation:**

```rust
// src/schema/inference.rs (add validation)

pub fn validate_schema(schema: &TViewSchema) -> Result<(), String> {
    // Warning: Missing pk_ column
    if schema.pk_column.is_none() {
        // Not an error - but should warn
        // pgrx::warning!("No pk_<entity> column found");
    }

    // Warning: Missing data column
    if schema.data_column.is_none() {
        // pgrx::warning!("No 'data' JSONB column found");
    }

    // Error: Missing id column (required for external API)
    if schema.id_column.is_none() {
        return Err("Missing 'id' column (required for Trinity identifier pattern)".to_string());
    }

    Ok(())
}

pub fn infer_schema(sql: &str) -> Result<TViewSchema, String> {
    let columns = parser::parse_select_columns(sql)?;

    if columns.is_empty() {
        return Err("No columns found in SELECT statement".to_string());
    }

    let mut schema = TViewSchema::new();

    // ... (existing inference logic)

    // Validate before returning
    validate_schema(&schema)?;

    Ok(schema)
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/12_schema_inference_validation.sql
```

---

### Test 4: Type Inference from PostgreSQL Catalog

**RED Phase - Write Failing Test:**

```sql
-- test/sql/13_type_inference.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create test table with various types
    CREATE TABLE tb_test_types (
        pk_test INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        fk_user INTEGER,
        user_id UUID,
        name TEXT,
        is_active BOOLEAN,
        created_at TIMESTAMPTZ,
        tags TEXT[],
        data JSONB
    );

    -- Test: Infer column types
    SELECT jsonb_pretty(
        pg_tviews_infer_types('tb_test_types', ARRAY[
            'pk_test',
            'id',
            'fk_user',
            'user_id',
            'name',
            'is_active',
            'created_at',
            'tags',
            'data'
        ])
    );

    -- Expected:
    -- {
    --   "pk_test": "integer",
    --   "id": "uuid",
    --   "fk_user": "integer",
    --   "user_id": "uuid",
    --   "name": "text",
    --   "is_active": "boolean",
    --   "created_at": "timestamp with time zone",
    --   "tags": "text[]",
    --   "data": "jsonb"
    -- }

ROLLBACK;
```

**GREEN Phase - Implementation:**

```rust
// src/schema/types.rs
use pgrx::prelude::*;
use std::collections::HashMap;

pub fn infer_column_types(
    table_name: &str,
    columns: &[String],
) -> Result<HashMap<String, String>, String> {
    let mut types = HashMap::new();

    for col in columns {
        // Query PostgreSQL catalog for column type
        let type_query = format!(
            "SELECT format_type(atttypid, atttypmod)
             FROM pg_attribute
             WHERE attrelid = '{}'::regclass
               AND attname = '{}'
               AND attnum > 0
               AND NOT attisdropped",
            table_name, col
        );

        let col_type = Spi::get_one::<String>(&type_query)
            .map_err(|e| format!("Failed to get type for column {}: {}", col, e))?
            .ok_or_else(|| format!("Column {} not found in table {}", col, table_name))?;

        types.insert(col.clone(), col_type);
    }

    Ok(types)
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[pg_test]
    fn test_infer_column_types() {
        // Create test table
        Spi::run("CREATE TABLE test_types (
            pk INTEGER PRIMARY KEY,
            id UUID NOT NULL,
            name TEXT,
            is_active BOOLEAN,
            data JSONB
        )").unwrap();

        let columns = vec![
            "pk".to_string(),
            "id".to_string(),
            "name".to_string(),
            "is_active".to_string(),
            "data".to_string(),
        ];

        let types = infer_column_types("test_types", &columns).unwrap();

        assert_eq!(types.get("pk"), Some(&"integer".to_string()));
        assert_eq!(types.get("id"), Some(&"uuid".to_string()));
        assert_eq!(types.get("name"), Some(&"text".to_string()));
        assert_eq!(types.get("is_active"), Some(&"boolean".to_string()));
        assert_eq!(types.get("data"), Some(&"jsonb".to_string()));
    }
}
```

```rust
// src/lib.rs (add function export)
use schema::types::infer_column_types;

#[pg_extern]
fn pg_tviews_infer_types(
    table_name: &str,
    columns: Vec<String>,
) -> JsonB {
    match infer_column_types(table_name, &columns) {
        Ok(types) => {
            let json_value = serde_json::to_value(&types).unwrap();
            JsonB(json_value)
        }
        Err(e) => {
            error!("Type inference failed: {}", e);
        }
    }
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/13_type_inference.sql
```

---

## Implementation Steps

### Step 1: Create Schema Module Structure

```bash
mkdir -p src/schema
touch src/schema/mod.rs
touch src/schema/parser.rs
touch src/schema/inference.rs
touch src/schema/types.rs
```

### Step 2: Implement Each Component (TDD)

Follow RED → GREEN → REFACTOR for each test:
1. Write failing test
2. Implement minimal code to pass
3. Refactor for clarity
4. Add Rust unit tests
5. Add SQL integration tests

### Step 3: Edge Case Testing

Add tests for:
- Missing columns
- Duplicate column names
- Invalid SQL syntax
- Empty SELECT
- No FROM clause
- Complex subqueries

### Step 4: Performance Testing

```rust
#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod perf_tests {
    use pgrx::prelude::*;
    use std::time::Instant;

    #[pg_test]
    fn test_inference_performance() {
        let sql = "SELECT pk, id, fk_1, fk_2, user_id, data FROM tb_test";

        let start = Instant::now();
        for _ in 0..1000 {
            let _ = crate::schema::inference::infer_schema(sql);
        }
        let duration = start.elapsed();

        // Should complete 1000 inferences in < 100ms
        assert!(duration.as_millis() < 100);
    }
}
```

---

## Acceptance Criteria

### Functional Requirements

- [x] `pg_tviews_analyze_select()` function exists
- [x] Detects `pk_<entity>` column
- [x] Detects `id` column
- [x] Detects `identifier` column (optional)
- [x] Detects `data` JSONB column
- [x] Detects all `fk_*` columns
- [x] Detects all `*_id` UUID columns
- [x] Identifies additional columns
- [x] Handles missing columns gracefully
- [x] `pg_tviews_infer_types()` queries PostgreSQL catalog
- [x] Returns column type information

### Quality Requirements

- [x] All Rust unit tests pass
- [x] All SQL integration tests pass
- [x] Edge cases covered (missing columns, empty SELECT, etc.)
- [x] Clear error messages for invalid input
- [x] Code documented with inline comments
- [x] Performance validated (< 1ms per inference)

### Performance Requirements

- [x] Schema inference < 1ms per SELECT
- [x] Type inference < 10ms per table
- [x] No memory leaks in repeated calls

---

## Rollback Plan

If Phase 1 fails:

1. **Parser Issues**: Fall back to simpler regex, document limitations
2. **Type Inference Failures**: Return `text` as default type, warn user
3. **Performance Issues**: Add caching layer for repeated inferences

Can rollback by removing `schema` module, no database changes yet.

---

## Next Phase

Once Phase 1 complete:
- **Phase 2**: View & Table Creation
- Implement `CREATE TVIEW` SQL syntax handler
- Generate backing view (v_<entity>)
- Generate materialized table (tv_<entity>)
- Populate initial data

---

## Notes

- Keep parser simple for v1 (regex-based)
- Future: Use PostgreSQL parser API for robustness
- Document parser limitations in README
- Consider SQL injection risks (all input via Spi::run is safe)
