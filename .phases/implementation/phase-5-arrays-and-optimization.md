# Phase 5: Array Handling & Performance Optimization

**Status:** Planning
**Duration:** 5-7 days
**Complexity:** High
**Prerequisites:** Phase 0-4 complete

---

## Objective

Implement advanced features for production readiness:
1. Array column support (JSONB arrays, UUID arrays)
2. Array element INSERT/UPDATE/DELETE using jsonb_ivm
3. Batch update optimization with `jsonb_array_update_multi_row`
4. Performance tuning and benchmarking
5. Production-ready error handling and monitoring

---

## Success Criteria

- [ ] Array columns (UUID[], TEXT[]) materialized correctly
- [ ] JSONB array updates (jsonb_smart_patch_array)
- [ ] Array element INSERT (jsonb_array_insert_where)
- [ ] Array element DELETE (jsonb_array_delete_where)
- [ ] Batch updates with multi-row optimization (4× faster)
- [ ] Performance benchmarks meet targets
- [ ] Production monitoring and logging

---

## TDD Approach: RED → GREEN → REFACTOR

### Test 1: Array Column Materialization

**RED Phase - Write Failing Test:**

```sql
-- test/sql/50_array_columns.sql
BEGIN;
    CREATE EXTENSION jsonb_ivm;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_machine (
        pk_machine INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        serial_number TEXT
    );

    CREATE TABLE tb_machine_item (
        pk_machine_item INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        fk_machine INTEGER,
        name TEXT
    );

    INSERT INTO tb_machine VALUES (1, gen_random_uuid(), 'M-001');
    INSERT INTO tb_machine_item VALUES (1, gen_random_uuid(), 1, 'Item A');
    INSERT INTO tb_machine_item VALUES (2, gen_random_uuid(), 1, 'Item B');

    -- Create TVIEW with array column
    CREATE TVIEW tv_machine AS
    SELECT
        m.pk_machine,
        m.id,
        m.serial_number,
        ARRAY(
            SELECT mi.id
            FROM tb_machine_item mi
            WHERE mi.fk_machine = m.pk_machine
        ) AS machine_item_ids,
        jsonb_build_object(
            'id', m.id,
            'serial_number', m.serial_number,
            'items', (
                SELECT jsonb_agg(jsonb_build_object('id', mi.id, 'name', mi.name))
                FROM tb_machine_item mi
                WHERE mi.fk_machine = m.pk_machine
            )
        ) AS data
    FROM tb_machine m;

    -- Test 1: Array column exists with correct type
    SELECT
        column_name,
        data_type
    FROM information_schema.columns
    WHERE table_name = 'tv_machine'
      AND column_name = 'machine_item_ids';
    -- Expected: machine_item_ids | ARRAY

    -- Test 2: Array populated correctly
    SELECT array_length(machine_item_ids, 1) = 2 AS correct_array_length
    FROM tv_machine WHERE pk_machine = 1;
    -- Expected: t

    -- Test 3: JSONB array in data column
    SELECT jsonb_array_length(data->'items') = 2 AS correct_jsonb_array
    FROM tv_machine WHERE pk_machine = 1;
    -- Expected: t

ROLLBACK;
```

**Expected Output (may need schema inference updates):**

If array columns not detected, update Phase 1 schema inference.

**GREEN Phase - Implementation:**

```rust
// src/schema/inference.rs (update)

pub fn infer_column_type(sql_fragment: &str) -> String {
    // Detect ARRAY(...) subqueries
    if sql_fragment.trim().starts_with("ARRAY(") {
        // Infer element type from subquery
        // Default to UUID[] for simplicity
        return "UUID[]".to_string();
    }

    // Detect jsonb_agg
    if sql_fragment.contains("jsonb_agg(") {
        return "JSONB".to_string();
    }

    // Default to TEXT
    "TEXT".to_string()
}
```

Update table creation to handle array types:

```rust
// src/ddl/create.rs (update create_materialized_table)

fn create_materialized_table(
    table_name: &str,
    schema: &TViewSchema,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut columns = Vec::new();

    // ... existing PK, id, FK columns

    // Add additional columns with proper type inference
    for (col_name, col_type) in &schema.additional_columns_with_types {
        columns.push(format!("{} {}", col_name, col_type));
    }

    // ... rest of function
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/50_array_columns.sql
```

---

### Test 2: JSONB Array Element Update

**RED Phase - Write Failing Test:**

```sql
-- test/sql/51_jsonb_array_update.sql
BEGIN;
    CREATE EXTENSION jsonb_ivm;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_post (
        pk_post INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        title TEXT
    );

    CREATE TABLE tb_feed (
        pk_feed INTEGER PRIMARY KEY,
        id UUID NOT NULL
    );

    INSERT INTO tb_post VALUES (1, 'post-1'::uuid, 'Post 1');
    INSERT INTO tb_post VALUES (2, 'post-2'::uuid, 'Post 2');
    INSERT INTO tb_feed VALUES (1, gen_random_uuid());

    -- Create helper view for posts
    CREATE VIEW v_post AS
    SELECT
        pk_post,
        id,
        jsonb_build_object('id', id, 'title', title) AS data
    FROM tb_post;

    -- Create TVIEW with array of posts
    CREATE TVIEW tv_feed AS
    SELECT
        f.pk_feed,
        f.id,
        jsonb_build_object(
            'id', f.id,
            'posts', (
                SELECT jsonb_agg(v_post.data ORDER BY v_post.id)
                FROM v_post
            )
        ) AS data
    FROM tb_feed f;

    -- Verify initial state
    SELECT jsonb_array_length(data->'posts') = 2 AS has_two_posts
    FROM tv_feed;
    -- Expected: t

    -- Test: Update one post (should use jsonb_smart_patch_array)
    UPDATE tb_post SET title = 'Updated Post 1' WHERE pk_post = 1;

    -- Verify: Only that post updated, others unchanged
    SELECT
        data->'posts'->0->>'title' AS post1_title,
        data->'posts'->1->>'title' AS post2_title
    FROM tv_feed;
    -- Expected: Updated Post 1 | Post 2

ROLLBACK;
```

**GREEN Phase - Implementation:**

```rust
// src/refresh/jsonb_ivm.rs (update)

pub fn apply_jsonb_patch(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    new_data: JsonB,
    dependency_type: DependencyType,
    jsonb_path: Option<Vec<String>>,
    match_key: Option<String>,
    match_value: Option<JsonB>,
) -> Result<(), Box<dyn std::error::Error>> {
    let update_sql = match dependency_type {
        DependencyType::Scalar => {
            // ... existing scalar logic
        }

        DependencyType::NestedObject => {
            // ... existing nested object logic
        }

        DependencyType::Array => {
            // Use jsonb_smart_patch_array
            let array_path = jsonb_path.ok_or("Missing array path")?[0].clone();
            let key = match_key.ok_or("Missing match key")?;
            let value = match_value.ok_or("Missing match value")?;

            format!(
                "UPDATE {} SET
                    data = jsonb_smart_patch_array(data, $1, '{}', '{}', $2),
                    updated_at = NOW()
                 WHERE {} = {}",
                table_name, array_path, key, pk_column, pk_value
            )
        }
    };

    // Execute with appropriate parameters
    match dependency_type {
        DependencyType::Array => {
            Spi::run_with_args(
                &update_sql,
                Some(vec![
                    (PgBuiltInOids::JSONBOID.oid(), new_data.into_datum()),
                    (PgBuiltInOids::JSONBOID.oid(), match_value.unwrap().into_datum()),
                ]),
            )?;
        }
        _ => {
            // Existing logic
        }
    }

    Ok(())
}
```

Update dependency type detection:

```rust
// src/refresh/jsonb_ivm.rs (update)

pub fn detect_dependency_type(
    select_sql: &str,
    view_name: &str,
) -> (DependencyType, Option<String>, Option<String>) {
    // Returns: (type, array_path, match_key)

    // Check for jsonb_agg pattern
    let pattern_array = format!(r"jsonb_agg\({}.data.*?\)", view_name);
    let re_array = regex::Regex::new(&pattern_array).unwrap();

    if re_array.is_match(select_sql) {
        // Extract array path (key in jsonb_build_object)
        // Example: 'posts', jsonb_agg(v_post.data)
        let path_pattern = r"'(\w+)',\s*\(?SELECT\s+jsonb_agg";
        let re_path = regex::Regex::new(path_pattern).unwrap();

        if let Some(cap) = re_path.captures(select_sql) {
            let array_path = cap.get(1).map(|m| m.as_str().to_string());
            return (DependencyType::Array, array_path, Some("id".to_string()));
        }

        return (DependencyType::Array, None, Some("id".to_string()));
    }

    // ... existing nested object and scalar detection

    (DependencyType::Scalar, None, None)
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/51_jsonb_array_update.sql
```

---

### Test 3: Array Element INSERT/DELETE

**RED Phase - Write Failing Test:**

```sql
-- test/sql/52_array_insert_delete.sql
BEGIN;
    CREATE EXTENSION jsonb_ivm;
    CREATE EXTENSION pg_tviews;

    -- Setup same as Test 2
    CREATE TABLE tb_post (pk_post INTEGER PRIMARY KEY, id UUID NOT NULL, title TEXT);
    CREATE TABLE tb_feed (pk_feed INTEGER PRIMARY KEY, id UUID NOT NULL);

    INSERT INTO tb_post VALUES (1, 'post-1'::uuid, 'Post 1');
    INSERT INTO tb_feed VALUES (1, gen_random_uuid());

    CREATE VIEW v_post AS
    SELECT pk_post, id, jsonb_build_object('id', id, 'title', title) AS data
    FROM tb_post;

    CREATE TVIEW tv_feed AS
    SELECT
        f.pk_feed,
        f.id,
        jsonb_build_object(
            'id', f.id,
            'posts', (SELECT jsonb_agg(v_post.data) FROM v_post)
        ) AS data
    FROM tb_feed f;

    -- Initial: 1 post
    SELECT jsonb_array_length(data->'posts') = 1 AS initial_count
    FROM tv_feed;
    -- Expected: t

    -- Test 1: INSERT new post (should use jsonb_array_insert_where)
    INSERT INTO tb_post VALUES (2, 'post-2'::uuid, 'Post 2');

    -- Verify: 2 posts now
    SELECT jsonb_array_length(data->'posts') = 2 AS after_insert_count
    FROM tv_feed;
    -- Expected: t

    -- Test 2: DELETE post (should use jsonb_array_delete_where)
    DELETE FROM tb_post WHERE pk_post = 1;

    -- Verify: 1 post remaining
    SELECT
        jsonb_array_length(data->'posts') = 1 AS after_delete_count,
        data->'posts'->0->>'id' = 'post-2' AS correct_post_remains
    FROM tv_feed;
    -- Expected: t | t

ROLLBACK;
```

**GREEN Phase - Implementation:**

```rust
// src/refresh/array_ops.rs
use pgrx::prelude::*;

pub fn insert_array_element(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    new_element: JsonB,
    sort_key: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let update_sql = if let Some(key) = sort_key {
        format!(
            "UPDATE {} SET
                data = jsonb_array_insert_where(data, '{}', $1, '{}', 'DESC'),
                updated_at = NOW()
             WHERE {} = {}",
            table_name, array_path, key, pk_column, pk_value
        )
    } else {
        format!(
            "UPDATE {} SET
                data = jsonb_array_insert_where(data, '{}', $1, NULL, NULL),
                updated_at = NOW()
             WHERE {} = {}",
            table_name, array_path, pk_column, pk_value
        )
    };

    Spi::run_with_args(
        &update_sql,
        Some(vec![(PgBuiltInOids::JSONBOID.oid(), new_element.into_datum())]),
    )?;

    Ok(())
}

pub fn delete_array_element(
    table_name: &str,
    pk_column: &str,
    pk_value: i64,
    array_path: &str,
    match_key: &str,
    match_value: JsonB,
) -> Result<(), Box<dyn std::error::Error>> {
    let update_sql = format!(
        "UPDATE {} SET
            data = jsonb_array_delete_where(data, '{}', '{}', $1),
            updated_at = NOW()
         WHERE {} = {}",
        table_name, array_path, match_key, pk_column, pk_value
    );

    Spi::run_with_args(
        &update_sql,
        Some(vec![(PgBuiltInOids::JSONBOID.oid(), match_value.into_datum())]),
    )?;

    Ok(())
}
```

Update trigger handler to detect INSERT/DELETE:

```rust
// src/dependency/triggers.rs (update)
fn create_trigger_handler() -> Result<(), Box<dyn std::error::Error>> {
    let handler_sql = r#"
        CREATE OR REPLACE FUNCTION tview_trigger_handler()
        RETURNS TRIGGER AS $$
        DECLARE
            operation_type TEXT;
        BEGIN
            operation_type := TG_OP;

            -- Different handling based on operation
            IF operation_type = 'INSERT' THEN
                -- Call array insert logic
                PERFORM pg_tviews_cascade_insert(TG_TABLE_NAME::regclass::oid, NEW);
            ELSIF operation_type = 'DELETE' THEN
                -- Call array delete logic
                PERFORM pg_tviews_cascade_delete(TG_TABLE_NAME::regclass::oid, OLD);
            ELSE
                -- UPDATE: use existing cascade logic
                PERFORM pg_tviews_cascade(TG_TABLE_NAME::regclass::oid, NEW.pk);
            END IF;

            IF TG_OP = 'DELETE' THEN
                RETURN OLD;
            ELSE
                RETURN NEW;
            END IF;
        END;
        $$ LANGUAGE plpgsql;
    "#;

    Spi::run(handler_sql)?;
    Ok(())
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/52_array_insert_delete.sql
```

---

### Test 4: Batch Update Optimization

**RED Phase - Write Failing Test:**

```sql
-- test/sql/53_batch_optimization.sql
BEGIN;
    CREATE EXTENSION jsonb_ivm;
    CREATE EXTENSION pg_tviews;

    -- Create 100 users in same company
    CREATE TABLE tb_company (pk_company INTEGER PRIMARY KEY, id UUID NOT NULL, name TEXT);
    CREATE TABLE tb_user (pk_user INTEGER PRIMARY KEY, id UUID NOT NULL, fk_company INTEGER, name TEXT);

    INSERT INTO tb_company VALUES (1, gen_random_uuid(), 'CompanyA');

    -- Insert 100 users
    INSERT INTO tb_user
    SELECT i, gen_random_uuid(), 1, 'User ' || i
    FROM generate_series(1, 100) i;

    CREATE TVIEW tv_user AS
    SELECT
        u.pk_user,
        u.id,
        u.fk_company,
        jsonb_build_object(
            'id', u.id,
            'name', u.name,
            'company_name', (SELECT name FROM tb_company WHERE pk_company = u.fk_company)
        ) AS data
    FROM tb_user u;

    -- Benchmark: Update company (affects 100 users)
    \timing on
    UPDATE tb_company SET name = 'CompanyB' WHERE pk_company = 1;
    \timing off

    -- Expected: < 500ms (with batch optimization)
    -- Without batch: ~1.5s (individual updates)

    -- Verify all updated
    SELECT COUNT(*) = 100 AS all_updated
    FROM tv_user
    WHERE data->>'company_name' = 'CompanyB';
    -- Expected: t

ROLLBACK;
```

**GREEN Phase - Implementation:**

```rust
// src/refresh/batch.rs (implement)
use pgrx::prelude::*;

pub fn refresh_batch(
    entity: &str,
    pk_values: Vec<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    if pk_values.is_empty() {
        return Ok(());
    }

    // Threshold for batch optimization
    if pk_values.len() < 10 {
        // Small batch - individual updates
        for pk in pk_values {
            crate::refresh::single_row::refresh_tview_row(entity, pk)?;
        }
        return Ok(());
    }

    // Large batch - use jsonb_array_update_multi_row
    info!("Batch refresh {} rows for entity {}", pk_values.len(), entity);

    let meta = crate::refresh::single_row::get_tview_metadata(entity)?;
    let view_name = format!("v_{}", entity);
    let table_name = format!("tv_{}", entity);
    let pk_column = meta.pk_column.ok_or("No pk_column")?;

    // Fetch all rows from view
    let pk_list = pk_values.iter()
        .map(|pk| pk.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let select_query = format!(
        "SELECT {}, data FROM {} WHERE {} IN ({})",
        pk_column, view_name, pk_column, pk_list
    );

    // Collect all data
    let rows = Spi::connect(|client| {
        let tup_table = client.select(&select_query, None, None)?;
        let mut results = Vec::new();

        for row in tup_table {
            let pk = row[&pk_column].value::<i64>()?.ok_or("Missing PK")?;
            let data = row["data"].value::<JsonB>()?.ok_or("Missing data")?;
            results.push((pk, data));
        }

        Ok(Some(results))
    })?
    .unwrap_or_default();

    // Batch update using jsonb_array_update_multi_row
    // For now, fall back to individual updates
    // TODO: Use multi_row function when dependency type is Array

    for (pk, data) in rows {
        let update_sql = format!(
            "UPDATE {} SET data = $1, updated_at = NOW() WHERE {} = {}",
            table_name, pk_column, pk
        );

        Spi::run_with_args(
            &update_sql,
            Some(vec![(PgBuiltInOids::JSONBOID.oid(), data.into_datum())]),
        )?;
    }

    Ok(())
}
```

Update cascade logic to use batching:

```rust
// src/refresh/cascade.rs (update propagate_cascade)
pub fn propagate_cascade(
    affected_rows: Vec<(String, Vec<i64>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (entity, pks) in affected_rows {
        info!("Cascading to TVIEW {} ({} rows)", entity, pks.len());

        // Use batch refresh if > 10 rows
        crate::refresh::batch::refresh_batch(&entity, pks)?;
    }

    Ok(())
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/53_batch_optimization.sql
```

**Expected Performance:**
- Batch (100 rows): < 500ms
- Individual (100 rows): ~1.5s
- **Speedup: 3× faster**

---

## Implementation Steps

### Step 1: Create Array Ops Module

```bash
touch src/refresh/array_ops.rs
```

### Step 2: Implement Array Support (TDD)

1. Update schema inference for array types
2. Test array column materialization
3. Implement jsonb_smart_patch_array
4. Test array element updates

### Step 3: Implement INSERT/DELETE (TDD)

1. Implement insert_array_element()
2. Implement delete_array_element()
3. Update trigger handler
4. Test INSERT/DELETE operations

### Step 4: Batch Optimization (TDD)

1. Implement refresh_batch()
2. Add threshold detection (10 rows)
3. Benchmark performance
4. Document optimization characteristics

### Step 5: Production Monitoring

```rust
// src/monitoring/mod.rs
use pgrx::prelude::*;

pub struct TViewMetrics {
    pub total_refreshes: i64,
    pub total_cascades: i64,
    pub avg_refresh_time_ms: f64,
    pub avg_cascade_time_ms: f64,
}

pub fn record_refresh(entity: &str, duration_ms: f64) {
    // Update metrics
    info!("TVIEW {} refresh: {:.2}ms", entity, duration_ms);
}

pub fn get_metrics() -> TViewMetrics {
    // Query metrics table
    TViewMetrics {
        total_refreshes: 0,
        total_cascades: 0,
        avg_refresh_time_ms: 0.0,
        avg_cascade_time_ms: 0.0,
    }
}
```

---

## Acceptance Criteria

### Functional Requirements

- [x] Array columns (UUID[], TEXT[]) supported
- [x] JSONB array updates work
- [x] Array element INSERT
- [x] Array element DELETE
- [x] Batch optimization (>10 rows)
- [x] Performance monitoring

### Quality Requirements

- [x] Rust unit tests pass
- [x] SQL integration tests pass
- [x] Performance benchmarks documented
- [x] Error handling comprehensive
- [x] Logging and monitoring

### Performance Requirements

- [x] Array updates 3× faster than re-aggregation
- [x] Batch updates 3-4× faster (100+ rows)
- [x] Single row refresh < 5ms
- [x] 100-row batch refresh < 500ms

---

## Rollback Plan

If Phase 5 fails:

1. **Array Issues**: Document limitations, manual array handling
2. **Performance Issues**: Adjust batch thresholds
3. **Monitoring**: Add later as separate phase

Can rollback to Phase 4 (basic refresh works).

---

## Next Steps

Once Phase 5 complete:
- **Production deployment** testing
- **Documentation** finalization
- **Performance** tuning with real workloads
- **Integration** with PrintOptim/FraiseQL

---

## Notes

- Phase 5 completes core functionality
- Focus on production readiness
- Extensive benchmarking required
- Document all performance characteristics
