# Phase 2: View & Table Creation

**Status:** Planning
**Duration:** 5-7 days
**Complexity:** High
**Prerequisites:** Phase 0 + Phase 1 complete

---

## Objective

Implement `CREATE TVIEW` SQL syntax that automatically:
1. Creates backing view (`v_<entity>`) containing the user's SELECT definition
2. Creates materialized table (`tv_<entity>`) with inferred schema
3. Populates initial data from the view
4. Registers metadata in `pg_tview_meta`

**NO triggers yet** - this phase focuses on DDL generation only.

---

## Success Criteria

- [ ] `CREATE TVIEW tv_post AS SELECT ...` syntax works
- [ ] Backing view `v_post` created with user's SELECT
- [ ] Materialized table `tv_post` created with correct schema
- [ ] Initial data populated (`INSERT INTO tv_post SELECT * FROM v_post`)
- [ ] Metadata registered in `pg_tview_meta`
- [ ] `DROP TABLE tv_post` cleans up all objects
- [ ] All tests pass with realistic examples

---

## TDD Approach: RED → GREEN → REFACTOR

### Test 1: Basic TVIEW Creation (Minimal Example)

**RED Phase - Write Failing Test:**

```sql
-- test/sql/20_create_tview_simple.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create base table
    CREATE TABLE tb_post (
        pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        title TEXT NOT NULL,
        content TEXT
    );

    INSERT INTO tb_post (title, content) VALUES
        ('First Post', 'Hello World'),
        ('Second Post', 'Testing TVIEW');

    -- Test: Create TVIEW
    CREATE TVIEW tv_post AS
    SELECT
        pk_post,
        id,
        jsonb_build_object(
            'id', id,
            'title', title,
            'content', content
        ) AS data
    FROM tb_post;

    -- Verification 1: Backing view exists
    SELECT COUNT(*) = 1 AS view_exists
    FROM information_schema.views
    WHERE table_name = 'v_post';
    -- Expected: t

    -- Verification 2: Materialized table exists
    SELECT COUNT(*) = 1 AS table_exists
    FROM information_schema.tables
    WHERE table_name = 'tv_post'
      AND table_type = 'BASE TABLE';
    -- Expected: t

    -- Verification 3: Correct columns in tv_post
    SELECT
        column_name,
        data_type
    FROM information_schema.columns
    WHERE table_name = 'tv_post'
    ORDER BY ordinal_position;
    -- Expected:
    -- pk_post | integer
    -- id | uuid
    -- data | jsonb
    -- updated_at | timestamp with time zone

    -- Verification 4: Initial data populated
    SELECT COUNT(*) = 2 AS data_populated
    FROM tv_post;
    -- Expected: t

    -- Verification 5: Metadata registered
    SELECT
        entity,
        view_oid IS NOT NULL AS has_view_oid,
        table_oid IS NOT NULL AS has_table_oid,
        definition IS NOT NULL AS has_definition
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: post | t | t | t

ROLLBACK;
```

**Expected Output (failing):**
```
ERROR: syntax error at or near "TVIEW"
```

**GREEN Phase - Implementation:**

```rust
// src/ddl/mod.rs
use pgrx::prelude::*;
use crate::schema::inference::infer_schema;
use crate::metadata;

pub mod create;
pub mod drop;

pub use create::create_tview;
pub use drop::drop_tview;
```

```rust
// src/ddl/create.rs
use pgrx::prelude::*;
use crate::schema::{TViewSchema, inference::infer_schema};

pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Infer schema from SELECT
    let schema = infer_schema(select_sql)?;

    let entity_name = schema.entity_name.as_ref()
        .ok_or("Could not infer entity name from SELECT")?;

    // Step 2: Create backing view v_<entity>
    let view_name = format!("v_{}", entity_name);
    create_backing_view(&view_name, select_sql)?;

    // Step 3: Create materialized table tv_<entity>
    create_materialized_table(&tview_name, &schema)?;

    // Step 4: Populate initial data
    populate_initial_data(&tview_name, &view_name)?;

    // Step 5: Register metadata
    register_metadata(entity_name, &view_name, &tview_name, select_sql, &schema)?;

    info!("TVIEW {} created successfully", tview_name);

    Ok(())
}

fn create_backing_view(
    view_name: &str,
    select_sql: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let create_view_sql = format!(
        "CREATE OR REPLACE VIEW {} AS {}",
        view_name,
        select_sql
    );

    Spi::run(&create_view_sql)?;

    Ok(())
}

fn create_materialized_table(
    table_name: &str,
    schema: &TViewSchema,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut columns = Vec::new();

    // Add pk_ column
    if let Some(pk_col) = &schema.pk_column {
        columns.push(format!("{} INTEGER NOT NULL PRIMARY KEY", pk_col));
    }

    // Add id column
    if let Some(id_col) = &schema.id_column {
        columns.push(format!("{} UUID NOT NULL UNIQUE", id_col));
    }

    // Add identifier column (optional)
    if let Some(identifier_col) = &schema.identifier_column {
        columns.push(format!("{} TEXT", identifier_col));
    }

    // Add fk_ columns
    for fk_col in &schema.fk_columns {
        columns.push(format!("{} INTEGER", fk_col));
    }

    // Add UUID FK columns
    for uuid_fk_col in &schema.uuid_fk_columns {
        columns.push(format!("{} UUID", uuid_fk_col));
    }

    // Add additional columns (flags, arrays, etc.)
    // TODO: Infer types properly in future phase
    for add_col in &schema.additional_columns {
        columns.push(format!("{} TEXT", add_col)); // Default to TEXT for now
    }

    // Add data column
    if let Some(data_col) = &schema.data_column {
        columns.push(format!("{} JSONB", data_col));
    }

    // Add updated_at column (always present)
    columns.push("updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()".to_string());

    let create_table_sql = format!(
        "CREATE TABLE {} ({})",
        table_name,
        columns.join(", ")
    );

    Spi::run(&create_table_sql)?;

    // Create indexes
    create_indexes(table_name, schema)?;

    Ok(())
}

fn create_indexes(
    table_name: &str,
    schema: &TViewSchema,
) -> Result<(), Box<dyn std::error::Error>> {
    // Index on id (for API queries)
    if let Some(id_col) = &schema.id_column {
        let idx_sql = format!(
            "CREATE INDEX idx_{}_{} ON {} ({})",
            table_name, id_col, table_name, id_col
        );
        Spi::run(&idx_sql)?;
    }

    // Indexes on UUID FK columns (for filtering)
    for uuid_fk_col in &schema.uuid_fk_columns {
        let idx_sql = format!(
            "CREATE INDEX idx_{}_{} ON {} ({})",
            table_name, uuid_fk_col, table_name, uuid_fk_col
        );
        Spi::run(&idx_sql)?;
    }

    // GIN index on data column (for JSONB queries)
    if let Some(data_col) = &schema.data_column {
        let idx_sql = format!(
            "CREATE INDEX idx_{}_{}_gin ON {} USING GIN ({})",
            table_name, data_col, table_name, data_col
        );
        Spi::run(&idx_sql)?;
    }

    Ok(())
}

fn populate_initial_data(
    table_name: &str,
    view_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let insert_sql = format!(
        "INSERT INTO {} SELECT * FROM {}",
        table_name, view_name
    );

    Spi::run(&insert_sql)?;

    Ok(())
}

fn register_metadata(
    entity: &str,
    view_name: &str,
    table_name: &str,
    definition: &str,
    schema: &TViewSchema,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get OIDs
    let view_oid = get_oid(view_name)?;
    let table_oid = get_oid(table_name)?;

    // Serialize arrays
    let fk_columns_array = format!("ARRAY{:?}", schema.fk_columns);
    let uuid_fk_columns_array = format!("ARRAY{:?}", schema.uuid_fk_columns);

    let insert_meta_sql = format!(
        "INSERT INTO pg_tview_meta (
            entity, view_oid, table_oid, definition,
            fk_columns, uuid_fk_columns
        ) VALUES (
            '{}', {}, {}, {},
            {}, {}
        )",
        entity,
        view_oid,
        table_oid,
        escape_literal(definition),
        fk_columns_array,
        uuid_fk_columns_array
    );

    Spi::run(&insert_meta_sql)?;

    Ok(())
}

fn get_oid(object_name: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT '{}'::regclass::oid",
        object_name
    ))?.ok_or("Failed to get OID")?;

    Ok(oid.into())
}

fn escape_literal(s: &str) -> String {
    format!("'{}'", s.replace("'", "''"))
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[pg_test]
    fn test_create_tview_minimal() {
        // Create base table
        Spi::run("
            CREATE TABLE tb_test (
                pk_test INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
                name TEXT
            )
        ").unwrap();

        Spi::run("INSERT INTO tb_test (name) VALUES ('Test')").unwrap();

        // Create TVIEW
        let select_sql = "
            SELECT
                pk_test,
                id,
                jsonb_build_object('id', id, 'name', name) AS data
            FROM tb_test
        ";

        create_tview("tv_test", select_sql).unwrap();

        // Verify backing view exists
        let view_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.views WHERE table_name = 'v_test'"
        ).unwrap();
        assert_eq!(view_exists, Some(true));

        // Verify table exists
        let table_exists = Spi::get_one::<bool>(
            "SELECT COUNT(*) = 1 FROM information_schema.tables WHERE table_name = 'tv_test'"
        ).unwrap();
        assert_eq!(table_exists, Some(true));

        // Verify data populated
        let data_count = Spi::get_one::<i64>(
            "SELECT COUNT(*) FROM tv_test"
        ).unwrap();
        assert_eq!(data_count, Some(1));
    }
}
```

Now we need to wire this into the PostgreSQL parser to recognize `CREATE TVIEW` syntax:

```rust
// src/lib.rs (updated)
mod ddl;

use ddl::create_tview;

// Hook into PostgreSQL DDL processing
#[pg_guard]
extern "C" fn process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext,
    params: *mut pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    unsafe {
        let query_str = std::ffi::CStr::from_ptr(query_string).to_str().unwrap();

        // Check if this is a CREATE TVIEW statement
        if query_str.trim().to_uppercase().starts_with("CREATE TVIEW") {
            handle_create_tview(query_str);
            return;
        }

        // Pass through to standard utility processing
        if let Some(prev_hook) = PREV_PROCESS_UTILITY_HOOK {
            prev_hook(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                completion_tag,
            );
        } else {
            pg_sys::standard_ProcessUtility(
                pstmt,
                query_string,
                read_only_tree,
                context,
                params,
                query_env,
                dest,
                completion_tag,
            );
        }
    }
}

fn handle_create_tview(query: &str) {
    // Parse: CREATE TVIEW tv_name AS SELECT ...
    let re = regex::Regex::new(r"(?i)CREATE\s+TVIEW\s+(\w+)\s+AS\s+(.+)")
        .unwrap();

    if let Some(caps) = re.captures(query) {
        let tview_name = caps.get(1).unwrap().as_str();
        let select_sql = caps.get(2).unwrap().as_str();

        match create_tview(tview_name, select_sql) {
            Ok(_) => {
                notice!("TVIEW {} created successfully", tview_name);
            }
            Err(e) => {
                error!("Failed to create TVIEW: {}", e);
            }
        }
    } else {
        error!("Invalid CREATE TVIEW syntax");
    }
}

static mut PREV_PROCESS_UTILITY_HOOK: Option<
    unsafe extern "C" fn(
        *mut pg_sys::PlannedStmt,
        *const std::os::raw::c_char,
        bool,
        pg_sys::ProcessUtilityContext,
        *mut pg_sys::ParamListInfo,
        *mut pg_sys::QueryEnvironment,
        *mut pg_sys::DestReceiver,
        *mut pg_sys::QueryCompletion,
    ),
> = None;

#[pg_guard]
extern "C" fn _PG_init() {
    unsafe {
        PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(process_utility_hook);
    }

    // Create metadata tables
    if let Err(e) = crate::metadata::create_metadata_tables() {
        error!("Failed to initialize pg_tviews metadata: {}", e);
    }
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
cargo pgrx install --release
psql -d test_db -f test/sql/20_create_tview_simple.sql
```

**Expected Output:**
All verification queries should return `t` (true).

---

### Test 2: TVIEW with Foreign Keys

**RED Phase - Write Failing Test:**

```sql
-- test/sql/21_create_tview_with_fks.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create base tables
    CREATE TABLE tb_user (
        pk_user INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        name TEXT NOT NULL
    );

    CREATE TABLE tb_post (
        pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        fk_user INTEGER NOT NULL,
        title TEXT NOT NULL
    );

    INSERT INTO tb_user (name) VALUES ('Alice');
    INSERT INTO tb_post (fk_user, title)
    VALUES ((SELECT pk_user FROM tb_user WHERE name = 'Alice'), 'First Post');

    -- Test: Create TVIEW with FK
    CREATE TVIEW tv_post AS
    SELECT
        p.pk_post,
        p.id,
        p.fk_user,
        u.id AS user_id,
        jsonb_build_object(
            'id', p.id,
            'title', p.title,
            'author_id', u.id
        ) AS data
    FROM tb_post p
    JOIN tb_user u ON u.pk_user = p.fk_user;

    -- Verification 1: fk_user column exists in tv_post
    SELECT COUNT(*) = 1 AS fk_column_exists
    FROM information_schema.columns
    WHERE table_name = 'tv_post'
      AND column_name = 'fk_user';
    -- Expected: t

    -- Verification 2: user_id column exists
    SELECT COUNT(*) = 1 AS uuid_fk_exists
    FROM information_schema.columns
    WHERE table_name = 'tv_post'
      AND column_name = 'user_id';
    -- Expected: t

    -- Verification 3: Data populated with correct FK values
    SELECT
        fk_user = (SELECT pk_user FROM tb_user WHERE name = 'Alice') AS fk_correct,
        user_id = (SELECT id FROM tb_user WHERE name = 'Alice') AS uuid_fk_correct
    FROM tv_post;
    -- Expected: t | t

    -- Verification 4: Metadata includes FK columns
    SELECT
        'fk_user' = ANY(fk_columns) AS has_fk,
        'user_id' = ANY(uuid_fk_columns) AS has_uuid_fk
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: t | t

ROLLBACK;
```

**GREEN Phase:**

Implementation from Test 1 should already handle this! Verify with test.

```bash
psql -d test_db -f test/sql/21_create_tview_with_fks.sql
```

---

### Test 3: DROP TABLE Cleanup

**RED Phase - Write Failing Test:**

```sql
-- test/sql/22_drop_tview.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_test (
        pk_test INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        data JSONB
    );

    CREATE TVIEW tv_test AS
    SELECT pk_test, id, data FROM tb_test;

    -- Verify TVIEW exists
    SELECT COUNT(*) = 1 AS tview_exists
    FROM pg_tview_meta WHERE entity = 'test';
    -- Expected: t

    -- Test: DROP TABLE
    DROP TABLE tv_test;

    -- Verification 1: Backing view dropped
    SELECT COUNT(*) = 0 AS view_dropped
    FROM information_schema.views WHERE table_name = 'v_test';
    -- Expected: t

    -- Verification 2: Materialized table dropped
    SELECT COUNT(*) = 0 AS table_dropped
    FROM information_schema.tables WHERE table_name = 'tv_test';
    -- Expected: t

    -- Verification 3: Metadata removed
    SELECT COUNT(*) = 0 AS metadata_removed
    FROM pg_tview_meta WHERE entity = 'test';
    -- Expected: t

ROLLBACK;
```

**Expected Output (failing):**
```
ERROR: syntax error at or near "TVIEW"
```

**GREEN Phase - Implementation:**

```rust
// src/ddl/drop.rs
use pgrx::prelude::*;

pub fn drop_tview(tview_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Get metadata
    let entity = get_entity_name(tview_name)?;
    let view_name = format!("v_{}", entity);

    // Step 2: Drop table
    Spi::run(&format!("DROP TABLE IF EXISTS {} CASCADE", tview_name))?;

    // Step 3: Drop view
    Spi::run(&format!("DROP VIEW IF EXISTS {} CASCADE", view_name))?;

    // Step 4: Remove metadata
    Spi::run(&format!(
        "DELETE FROM pg_tview_meta WHERE entity = '{}'",
        entity
    ))?;

    info!("TVIEW {} dropped successfully", tview_name);

    Ok(())
}

fn get_entity_name(tview_name: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Extract entity from tv_<entity>
    if tview_name.starts_with("tv_") {
        Ok(tview_name[3..].to_string())
    } else {
        Err("Invalid TVIEW name (must start with tv_)".into())
    }
}
```

Update hook handler:

```rust
// src/lib.rs (add DROP handling)
fn process_utility_hook(...) {
    unsafe {
        let query_str = std::ffi::CStr::from_ptr(query_string).to_str().unwrap();

        // Handle CREATE TVIEW
        if query_str.trim().to_uppercase().starts_with("CREATE TVIEW") {
            handle_create_tview(query_str);
            return;
        }

        // Handle DROP TABLE
        if query_str.trim().to_uppercase().starts_with("DROP TABLE") {
            handle_drop_tview(query_str);
            return;
        }

        // ... rest of hook
    }
}

fn handle_drop_tview(query: &str) {
    let re = regex::Regex::new(r"(?i)DROP\s+TVIEW\s+(\w+)").unwrap();

    if let Some(caps) = re.captures(query) {
        let tview_name = caps.get(1).unwrap().as_str();

        match ddl::drop_tview(tview_name) {
            Ok(_) => notice!("TVIEW {} dropped", tview_name),
            Err(e) => error!("Failed to drop TVIEW: {}", e),
        }
    } else {
        error!("Invalid DROP TABLE syntax");
    }
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
psql -d test_db -f test/sql/22_drop_tview.sql
```

---

## Implementation Steps

### Step 1: Create DDL Module Structure

```bash
mkdir -p src/ddl
touch src/ddl/mod.rs
touch src/ddl/create.rs
touch src/ddl/drop.rs
```

### Step 2: Implement CREATE TVIEW (TDD)

Follow RED → GREEN → REFACTOR for each test:
1. Write failing test for simple TVIEW
2. Implement create_tview() function
3. Hook into PostgreSQL parser
4. Test with real tables
5. Add FK support
6. Test edge cases

### Step 3: Implement DROP TABLE (TDD)

1. Write failing test
2. Implement drop_tview() function
3. Update parser hook
4. Test cleanup

### Step 4: Add Helper View Detection (Preview)

```rust
// src/ddl/helpers.rs
pub fn detect_helper_views(select_sql: &str) -> Vec<String> {
    // Parse SELECT for v_* references
    // Return list of helper views used
    vec![]
}
```

---

## Acceptance Criteria

### Functional Requirements

- [x] `CREATE TVIEW tv_<name> AS SELECT ...` syntax works
- [x] Backing view `v_<entity>` created correctly
- [x] Materialized table `tv_<entity>` with correct schema
- [x] Initial data populated from view
- [x] Metadata registered in `pg_tview_meta`
- [x] `DROP TABLE tv_<name>` removes all objects
- [x] Indexes created on id, UUID FKs, and data columns

### Quality Requirements

- [x] Rust unit tests pass
- [x] SQL integration tests pass
- [x] Error messages clear and actionable
- [x] Transactional consistency (CREATE/DROP in transactions)
- [x] No SQL injection vulnerabilities
- [x] Documentation updated

### Performance Requirements

- [x] TVIEW creation < 1s for small tables (<1000 rows)
- [x] TVIEW creation < 10s for medium tables (<100k rows)
- [x] DROP TABLE < 100ms

---

## Rollback Plan

If Phase 2 fails:

1. **Parser Hook Issues**: Fall back to function-based API (pg_tviews_create())
2. **Schema Generation Bugs**: Add validation before DDL execution
3. **Performance Issues**: Add LIMIT to initial population, document separately

Can rollback with `DROP TABLE` - all objects cleaned up properly.

---

## Next Phase

Once Phase 2 complete:
- **Phase 3**: Dependency Detection & Trigger Installation
- Parse view dependencies using pg_depend
- Install AFTER triggers on base tables
- Implement trigger handler function

---

## Notes

- Phase 2 is DDL-only (no runtime updates yet)
- Triggers are Phase 3 - keep phases focused
- Test with PrintOptim-like schemas (allocation, machine, etc.)
- Document TVIEW naming conventions (must start with tv_)
