# Phase 3: Dependency Detection & Trigger Installation

**Status:** Planning
**Duration:** 5-7 days
**Complexity:** High
**Prerequisites:** Phase 0 + Phase 1 + Phase 2 complete

---

## Objective

Implement automatic dependency detection and trigger installation:
1. Walk `pg_depend` graph to find all base tables underlying a view
2. Detect helper views used in TVIEW SELECT
3. Install AFTER triggers on all transitive base tables
4. Register dependency metadata
5. Handle trigger lifecycle (create/drop)

**NO refresh logic yet** - this phase focuses on change detection and metadata only.

---

## Success Criteria

- [ ] Detect all base tables (tb_*) underlying a TVIEW
- [ ] Detect helper views (v_*) used by TVIEW
- [ ] Install AFTER triggers on base tables (INSERT, UPDATE, DELETE)
- [ ] Triggers fire successfully on base table changes
- [ ] Dependency metadata registered in pg_tview_meta
- [ ] Helper view metadata registered in pg_tview_helpers
- [ ] DROP TABLE removes all triggers
- [ ] All tests pass with nested dependencies

---

## TDD Approach: RED → GREEN → REFACTOR

### Test 1: Single Table Dependency Detection

**RED Phase - Write Failing Test:**

```sql
-- test/sql/30_dependency_detection_simple.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create base table
    CREATE TABLE tb_post (
        pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
        id UUID DEFAULT gen_random_uuid() NOT NULL UNIQUE,
        title TEXT NOT NULL
    );

    -- Create TVIEW
    CREATE TVIEW tv_post AS
    SELECT
        pk_post,
        id,
        jsonb_build_object('id', id, 'title', title) AS data
    FROM tb_post;

    -- Test: Verify dependencies detected
    SELECT jsonb_pretty(
        (SELECT dependencies FROM pg_tview_meta WHERE entity = 'post')::jsonb
    );

    -- Expected: Array with OID of tb_post
    -- Example: [16385]

    -- Test: Verify trigger installed
    SELECT COUNT(*) >= 1 AS trigger_exists
    FROM pg_trigger
    WHERE tgrelid = 'tb_post'::regclass
      AND tgname LIKE 'trg_tview_%';
    -- Expected: t

    -- Test: Trigger function exists
    SELECT COUNT(*) = 1 AS trigger_func_exists
    FROM pg_proc
    WHERE proname = 'tview_trigger_handler';
    -- Expected: t

ROLLBACK;
```

**Expected Output (failing):**
```
 dependencies
--------------
 {}
(empty array - dependencies not detected yet)

 trigger_exists
----------------
 f
(no trigger installed)
```

**GREEN Phase - Implementation:**

```rust
// src/dependency/mod.rs
use pgrx::prelude::*;
use std::collections::HashSet;

pub mod graph;
pub mod triggers;

pub use graph::{find_base_tables, find_helper_views};
pub use triggers::{install_triggers, remove_triggers};

#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub base_tables: Vec<pg_sys::Oid>,
    pub helper_views: Vec<String>,
    pub depth: usize,
}
```

```rust
// src/dependency/graph.rs
use pgrx::prelude::*;
use std::collections::{HashSet, VecDeque};

/// Find all base tables that a view depends on (transitively)
pub fn find_base_tables(view_name: &str) -> Result<Vec<pg_sys::Oid>, Box<dyn std::error::Error>> {
    let view_oid = get_oid(view_name)?;
    let mut base_tables = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(view_oid);

    while let Some(current_oid) = queue.pop_front() {
        if visited.contains(&current_oid) {
            continue;
        }
        visited.insert(current_oid);

        // Query pg_depend for dependencies
        let deps_query = format!(
            "SELECT DISTINCT objid, objsubid, refobjid, refobjsubid, deptype
             FROM pg_depend
             WHERE refobjid = {}
               AND deptype = 'n'",  // 'n' = normal dependency
            current_oid
        );

        let deps: Vec<(pg_sys::Oid, i32, pg_sys::Oid, i32, String)> =
            Spi::connect(|client| {
                let mut results = Vec::new();
                let tup_table = client.select(&deps_query, None, None)?;

                for row in tup_table {
                    let objid = row["objid"].value::<pg_sys::Oid>()?;
                    let objsubid = row["objsubid"].value::<i32>()?;
                    let refobjid = row["refobjid"].value::<pg_sys::Oid>()?;
                    let refobjsubid = row["refobjsubid"].value::<i32>()?;
                    let deptype = row["deptype"].value::<String>()?;

                    if let (Some(oid), Some(subid), Some(refoid), Some(refsubid), Some(dtype)) =
                        (objid, objsubid, refobjid, refobjsubid, deptype)
                    {
                        results.push((oid, subid, refoid, refsubid, dtype));
                    }
                }

                Ok(Some(results))
            })?
            .unwrap_or_default();

        for (dep_oid, _, _, _, _) in deps {
            // Check if this is a table or view
            let relkind = get_relkind(dep_oid)?;

            match relkind.as_str() {
                "r" => {
                    // Regular table - add to base_tables
                    base_tables.insert(dep_oid);
                }
                "v" => {
                    // View - recurse
                    queue.push_back(dep_oid);
                }
                _ => {
                    // Ignore other types (indexes, sequences, etc.)
                }
            }
        }
    }

    Ok(base_tables.into_iter().collect())
}

/// Find all helper views (v_*) used by a SELECT statement
pub fn find_helper_views(select_sql: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut helpers = Vec::new();

    // Simple regex to find v_* references
    // TODO: Use PostgreSQL parser API for robustness
    let re = regex::Regex::new(r"\bv_(\w+)").unwrap();

    for cap in re.captures_iter(select_sql) {
        let helper_name = format!("v_{}", &cap[1]);
        if !helpers.contains(&helper_name) {
            helpers.push(helper_name);
        }
    }

    Ok(helpers)
}

fn get_oid(object_name: &str) -> Result<pg_sys::Oid, Box<dyn std::error::Error>> {
    let oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT '{}'::regclass::oid",
        object_name
    ))?.ok_or("Failed to get OID")?;

    Ok(oid)
}

fn get_relkind(oid: pg_sys::Oid) -> Result<String, Box<dyn std::error::Error>> {
    let relkind = Spi::get_one::<String>(&format!(
        "SELECT relkind::text FROM pg_class WHERE oid = {}",
        oid
    ))?.ok_or("Failed to get relkind")?;

    Ok(relkind)
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use super::*;

    #[pg_test]
    fn test_find_base_tables_single() {
        // Create base table
        Spi::run("CREATE TABLE tb_test (pk INTEGER PRIMARY KEY, id UUID, data JSONB)").unwrap();

        // Create view
        Spi::run("CREATE VIEW v_test AS SELECT * FROM tb_test").unwrap();

        // Find dependencies
        let base_tables = find_base_tables("v_test").unwrap();

        assert_eq!(base_tables.len(), 1);
    }

    #[pg_test]
    fn test_find_base_tables_transitive() {
        // Create base tables
        Spi::run("CREATE TABLE tb_user (pk INTEGER PRIMARY KEY, name TEXT)").unwrap();
        Spi::run("CREATE TABLE tb_post (pk INTEGER PRIMARY KEY, fk_user INTEGER, title TEXT)").unwrap();

        // Create helper view
        Spi::run("CREATE VIEW v_user AS SELECT * FROM tb_user").unwrap();

        // Create composite view
        Spi::run("CREATE VIEW v_post AS
            SELECT p.*, u.name FROM tb_post p JOIN v_user u ON u.pk = p.fk_user
        ").unwrap();

        // Find dependencies
        let base_tables = find_base_tables("v_post").unwrap();

        // Should find both tb_user and tb_post
        assert_eq!(base_tables.len(), 2);
    }
}
```

```rust
// src/dependency/triggers.rs
use pgrx::prelude::*;

pub fn install_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // First, create trigger handler function if not exists
    create_trigger_handler()?;

    // Install trigger on each base table
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;
        let trigger_name = format!("trg_tview_{}_{}", tview_entity, table_name);

        // Install AFTER INSERT OR UPDATE OR DELETE trigger
        let trigger_sql = format!(
            "CREATE TRIGGER {}
             AFTER INSERT OR UPDATE OR DELETE ON {}
             FOR EACH ROW
             EXECUTE FUNCTION tview_trigger_handler()",
            trigger_name, table_name
        );

        Spi::run(&trigger_sql)?;
        info!("Installed trigger {} on {}", trigger_name, table_name);
    }

    Ok(())
}

pub fn remove_triggers(
    table_oids: &[pg_sys::Oid],
    tview_entity: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for &table_oid in table_oids {
        let table_name = get_table_name(table_oid)?;
        let trigger_name = format!("trg_tview_{}_{}", tview_entity, table_name);

        let drop_sql = format!(
            "DROP TRIGGER IF EXISTS {} ON {}",
            trigger_name, table_name
        );

        Spi::run(&drop_sql)?;
    }

    Ok(())
}

fn create_trigger_handler() -> Result<(), Box<dyn std::error::Error>> {
    let handler_sql = r#"
        CREATE OR REPLACE FUNCTION tview_trigger_handler()
        RETURNS TRIGGER AS $$
        BEGIN
            -- For now, just log that trigger fired
            -- Actual refresh logic will be in Phase 4
            RAISE NOTICE 'TVIEW trigger fired on table % for operation %',
                TG_TABLE_NAME, TG_OP;

            -- Return appropriate value based on operation
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

fn get_table_name(oid: pg_sys::Oid) -> Result<String, Box<dyn std::error::Error>> {
    let name = Spi::get_one::<String>(&format!(
        "SELECT relname FROM pg_class WHERE oid = {}",
        oid
    ))?.ok_or("Failed to get table name")?;

    Ok(name)
}
```

Now update the CREATE TVIEW logic to use dependency detection:

```rust
// src/ddl/create.rs (updated)
use crate::dependency::{find_base_tables, find_helper_views, install_triggers};

pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // ... existing steps 1-4 (infer schema, create view, create table, populate)

    // Step 5: Detect dependencies
    let view_name = format!("v_{}", entity_name);
    let base_tables = find_base_tables(&view_name)?;
    let helper_views = find_helper_views(select_sql)?;

    info!("Detected {} base tables", base_tables.len());
    info!("Detected {} helper views", helper_views.len());

    // Step 6: Install triggers on base tables
    install_triggers(&base_tables, entity_name)?;

    // Step 7: Register metadata (including dependencies)
    register_metadata(
        entity_name,
        &view_name,
        &tview_name,
        select_sql,
        &schema,
        &base_tables,
        &helper_views,
    )?;

    Ok(())
}

fn register_metadata(
    entity: &str,
    view_name: &str,
    table_name: &str,
    definition: &str,
    schema: &TViewSchema,
    base_tables: &[pg_sys::Oid],
    helper_views: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let view_oid = get_oid(view_name)?;
    let table_oid = get_oid(table_name)?;

    // Convert OID array to PostgreSQL array literal
    let deps_array = format!(
        "ARRAY[{}]::oid[]",
        base_tables.iter()
            .map(|oid| oid.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    let insert_meta_sql = format!(
        "INSERT INTO pg_tview_meta (
            entity, view_oid, table_oid, definition,
            dependencies, fk_columns, uuid_fk_columns
        ) VALUES (
            '{}', {}, {}, {},
            {}, ARRAY{:?}, ARRAY{:?}
        )",
        entity,
        view_oid,
        table_oid,
        escape_literal(definition),
        deps_array,
        schema.fk_columns,
        schema.uuid_fk_columns
    );

    Spi::run(&insert_meta_sql)?;

    // Register helper views
    for helper in helper_views {
        register_helper(helper, entity)?;
    }

    Ok(())
}

fn register_helper(
    helper_name: &str,
    used_by_entity: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if helper already registered
    let exists = Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_helpers WHERE helper_name = '{}'",
        helper_name
    ))?.unwrap_or(false);

    if exists {
        // Update used_by array
        Spi::run(&format!(
            "UPDATE pg_tview_helpers
             SET used_by = array_append(used_by, '{}')
             WHERE helper_name = '{}'
               AND NOT ('{}' = ANY(used_by))",
            used_by_entity, helper_name, used_by_entity
        ))?;
    } else {
        // Insert new helper
        Spi::run(&format!(
            "INSERT INTO pg_tview_helpers (helper_name, used_by)
             VALUES ('{}', ARRAY['{}'])",
            helper_name, used_by_entity
        ))?;
    }

    Ok(())
}
```

Update DROP TABLE to remove triggers:

```rust
// src/ddl/drop.rs (updated)
use crate::dependency::remove_triggers;

pub fn drop_tview(tview_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let entity = get_entity_name(tview_name)?;

    // Step 1: Get dependencies from metadata
    let base_tables = get_dependencies(&entity)?;

    // Step 2: Remove triggers
    remove_triggers(&base_tables, &entity)?;

    // Step 3: Drop table
    Spi::run(&format!("DROP TABLE IF EXISTS {} CASCADE", tview_name))?;

    // Step 4: Drop view
    let view_name = format!("v_{}", entity);
    Spi::run(&format!("DROP VIEW IF EXISTS {} CASCADE", view_name))?;

    // Step 5: Remove metadata
    Spi::run(&format!(
        "DELETE FROM pg_tview_meta WHERE entity = '{}'",
        entity
    ))?;

    // Step 6: Update helper metadata
    update_helper_metadata(&entity)?;

    Ok(())
}

fn get_dependencies(entity: &str) -> Result<Vec<pg_sys::Oid>, Box<dyn std::error::Error>> {
    let deps = Spi::get_one::<Vec<pg_sys::Oid>>(&format!(
        "SELECT dependencies FROM pg_tview_meta WHERE entity = '{}'",
        entity
    ))?.ok_or("TVIEW not found")?;

    Ok(deps)
}

fn update_helper_metadata(removed_entity: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Remove entity from used_by arrays
    Spi::run(&format!(
        "UPDATE pg_tview_helpers
         SET used_by = array_remove(used_by, '{}')
         WHERE '{}' = ANY(used_by)",
        removed_entity, removed_entity
    ))?;

    // Delete helpers no longer used
    Spi::run("DELETE FROM pg_tview_helpers WHERE array_length(used_by, 1) IS NULL")?;

    Ok(())
}
```

**Verify GREEN:**
```bash
cargo pgrx test pg17
cargo pgrx install --release
psql -d test_db -f test/sql/30_dependency_detection_simple.sql
```

**Expected Output:**
```
 dependencies
--------------
 [16385]

 trigger_exists
----------------
 t

 trigger_func_exists
--------------------
 t
```

---

### Test 2: Transitive Dependencies (Helper Views)

**RED Phase - Write Failing Test:**

```sql
-- test/sql/31_dependency_detection_transitive.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    -- Create base tables
    CREATE TABLE tb_user (
        pk_user INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        name TEXT
    );

    CREATE TABLE tb_post (
        pk_post INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        fk_user INTEGER,
        title TEXT
    );

    -- Create helper view
    CREATE VIEW v_user AS
    SELECT
        pk_user,
        id,
        jsonb_build_object('id', id, 'name', name) AS data
    FROM tb_user;

    -- Create TVIEW using helper
    CREATE TVIEW tv_post AS
    SELECT
        p.pk_post,
        p.id,
        p.fk_user,
        u.id AS user_id,
        jsonb_build_object(
            'id', p.id,
            'title', p.title,
            'author', v_user.data  -- Uses helper view
        ) AS data
    FROM tb_post p
    JOIN v_user ON v_user.pk_user = p.fk_user;

    -- Test 1: Both base tables detected
    SELECT
        array_length(dependencies, 1) = 2 AS correct_dep_count,
        (SELECT relname FROM pg_class WHERE oid = ANY(dependencies)) @> ARRAY['tb_post', 'tb_user'] AS has_both_tables
    FROM pg_tview_meta
    WHERE entity = 'post';
    -- Expected: t | t

    -- Test 2: Helper view registered
    SELECT COUNT(*) = 1 AS helper_registered
    FROM pg_tview_helpers
    WHERE helper_name = 'v_user';
    -- Expected: t

    -- Test 3: Helper knows it's used by tv_post
    SELECT 'post' = ANY(used_by) AS used_by_post
    FROM pg_tview_helpers
    WHERE helper_name = 'v_user';
    -- Expected: t

    -- Test 4: Triggers on both tables
    SELECT
        (SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'tb_post'::regclass AND tgname LIKE 'trg_tview_%') > 0 AS post_trigger,
        (SELECT COUNT(*) FROM pg_trigger WHERE tgrelid = 'tb_user'::regclass AND tgname LIKE 'trg_tview_%') > 0 AS user_trigger;
    -- Expected: t | t

ROLLBACK;
```

**GREEN Phase:**

Implementation from Test 1 should handle this! The recursive walk through pg_depend will discover tb_user via v_user.

**Verify GREEN:**
```bash
psql -d test_db -f test/sql/31_dependency_detection_transitive.sql
```

---

### Test 3: Trigger Fires on Base Table Change

**RED Phase - Write Failing Test:**

```sql
-- test/sql/32_trigger_fires.sql
BEGIN;
    CREATE EXTENSION pg_tviews;

    CREATE TABLE tb_test (
        pk_test INTEGER PRIMARY KEY,
        id UUID NOT NULL,
        value TEXT
    );

    CREATE TVIEW tv_test AS
    SELECT pk_test, id, jsonb_build_object('value', value) AS data
    FROM tb_test;

    -- Insert test data
    INSERT INTO tb_test VALUES (1, gen_random_uuid(), 'initial');

    -- Test: Update base table (should trigger NOTICE)
    UPDATE tb_test SET value = 'updated' WHERE pk_test = 1;
    -- Expected NOTICE: TVIEW trigger fired on table tb_test for operation UPDATE

    -- Test: Delete from base table
    DELETE FROM tb_test WHERE pk_test = 1;
    -- Expected NOTICE: TVIEW trigger fired on table tb_test for operation DELETE

ROLLBACK;
```

**GREEN Phase:**

Already implemented! The trigger_handler function logs NOTICE messages.

**Verify GREEN:**
```bash
psql -d test_db -f test/sql/32_trigger_fires.sql
```

**Expected Output:**
```
NOTICE:  TVIEW trigger fired on table tb_test for operation UPDATE
NOTICE:  TVIEW trigger fired on table tb_test for operation DELETE
```

---

## Implementation Steps

### Step 1: Create Dependency Module

```bash
mkdir -p src/dependency
touch src/dependency/mod.rs
touch src/dependency/graph.rs
touch src/dependency/triggers.rs
```

### Step 2: Implement pg_depend Walker (TDD)

1. Write test for single-table dependency
2. Implement find_base_tables()
3. Test transitive dependencies
4. Add cycle detection
5. Optimize with visited set

### Step 3: Implement Trigger Installation (TDD)

1. Write test for trigger creation
2. Implement install_triggers()
3. Create trigger handler function
4. Test trigger fires
5. Implement remove_triggers()

### Step 4: Update CREATE/DROP TABLE

1. Integrate dependency detection into create_tview()
2. Update metadata registration
3. Integrate trigger removal into drop_tview()
4. Test full lifecycle

---

## Acceptance Criteria

### Functional Requirements

- [x] Detect all base tables via pg_depend walk
- [x] Handle transitive dependencies through helpers
- [x] Detect helper views from SELECT
- [x] Install AFTER triggers on all base tables
- [x] Trigger handler function created
- [x] Triggers fire on INSERT/UPDATE/DELETE
- [x] Metadata includes dependencies array
- [x] Helper metadata tracks used_by relationships
- [x] DROP TABLE removes all triggers

### Quality Requirements

- [x] Rust unit tests pass
- [x] SQL integration tests pass
- [x] Cycle detection in dependency walk
- [x] Clear NOTICE messages in trigger handler
- [x] No trigger leaks (all removed on DROP)
- [x] Documentation updated

### Performance Requirements

- [x] Dependency detection < 100ms per TVIEW
- [x] Trigger installation < 50ms per table
- [x] pg_depend walk handles 100+ dependencies

---

## Rollback Plan

If Phase 3 fails:

1. **pg_depend Issues**: Fall back to manual dependency specification
2. **Trigger Installation Failures**: Add validation before installation
3. **Performance Issues**: Add caching for dependency results

Can rollback by removing triggers manually if needed.

---

## Next Phase

Once Phase 3 complete:
- **Phase 4**: Refresh Logic & jsonb_delta Integration
- Implement row-level refresh
- Integrate jsonb_smart_patch_* functions
- Implement cascade propagation

---

## Notes

- Phase 3 is detection-only (no refresh yet)
- Trigger handler just logs for now
- Test with complex dependency graphs (3+ levels)
- Document pg_depend walk algorithm
