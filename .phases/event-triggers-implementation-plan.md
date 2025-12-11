# Event Triggers Implementation Plan for pg_tviews

**Date**: December 11, 2025
**Objective**: Replace ProcessUtility hook with Event Triggers for safe DDL interception
**Complexity**: Medium
**Estimated Time**: 8-12 hours
**Priority**: High (fixes critical SPI-in-hook bug)

---

## 📋 Executive Summary

**Current Problem**: The ProcessUtility hook cannot safely use SPI (SQL execution) within the hook callback context, causing panics when creating TVIEWs via DDL syntax.

**Solution**: Use PostgreSQL Event Triggers, which fire AFTER DDL completes, providing a safe context for SPI operations. This is the industry-standard approach used by TimescaleDB, Citus, and other mature extensions.

**Benefits**:
- ✅ Safe SPI usage (no panics)
- ✅ Official PostgreSQL API (better stability)
- ✅ Works with all DDL patterns (CREATE TABLE AS, SELECT INTO, etc.)
- ✅ Simpler error handling
- ✅ Better user experience

**Migration Path**:
- Phase 1-2: Add event trigger alongside existing hook
- Phase 3-4: Migrate functionality to event trigger
- Phase 5: Deprecate ProcessUtility hook (keep for validation only)
- Phase 6: Full testing and documentation

---

## 🎯 Architecture Overview

### Current Architecture (BROKEN)

```
User: CREATE TABLE tv_entity AS SELECT ...
  ↓
PostgreSQL Parser
  ↓
ProcessUtility Hook (pg_tviews) ← Hook intercepts here
  ↓
  create_tview() function
    ↓
    Spi::run() ← ❌ PANIC! Cannot use SPI in hook context
```

### New Architecture (EVENT TRIGGERS)

```
User: CREATE TABLE tv_entity AS SELECT ...
  ↓
PostgreSQL Parser
  ↓
ProcessUtility Hook (pg_tviews) - Validates only, lets it pass through
  ↓
Standard PostgreSQL CREATE TABLE (creates regular table tv_entity)
  ↓
Event Trigger Fires: ddl_command_end
  ↓
pg_tviews_handle_ddl_event() ← ✅ SAFE SPI context!
  ↓
  convert_table_to_tview(tv_entity)
    - Table already exists (PostgreSQL created it)
    - Rename tv_entity → tv_entity_materialized
    - Create view v_entity
    - Create tv_entity as view wrapper
    - Install triggers
    - Register metadata
```

**Key Insight**: Let PostgreSQL create the table, THEN convert it. This happens in milliseconds and is transparent to users.

---

## 📁 File Structure

```
src/
├── event_trigger.rs          ← NEW: Event trigger handler
├── ddl/
│   ├── create.rs             ← MODIFY: Split into two functions
│   ├── convert.rs            ← NEW: Convert existing table to TVIEW
│   └── validate.rs           ← NEW: Validation logic (used by hook)
├── hooks.rs                  ← SIMPLIFY: Only validation, no creation
├── lib.rs                    ← MODIFY: Register event trigger function
└── sql/
    └── event_triggers.sql    ← NEW: SQL to install event triggers

sql/
└── pg_tviews--0.2.0.sql      ← NEW: Migration with event triggers
```

---

## 🔄 Phase Breakdown

---

### **Phase 1: Create Event Trigger Infrastructure** ⏱️ 2 hours

**Objective**: Add event trigger handler function without changing existing behavior.

#### Files to Create

**`src/event_trigger.rs`**

```rust
//! Event Trigger handler for DDL interception
//!
//! This module handles PostgreSQL Event Triggers that fire AFTER DDL commands
//! complete. This provides a safe context for SPI operations, unlike ProcessUtility
//! hooks which cannot safely use SPI.

use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};

/// Event trigger function called after DDL command completes
///
/// This is registered as an event trigger in SQL:
/// ```sql
/// CREATE EVENT TRIGGER pg_tviews_ddl_end
/// ON ddl_command_end
/// WHEN TAG IN ('CREATE TABLE', 'SELECT INTO')
/// EXECUTE FUNCTION pg_tviews_handle_ddl_event();
/// ```
///
/// # Safety Context
/// Event triggers fire AFTER the DDL completes, providing a safe context
/// for SPI operations. The table already exists at this point.
#[pg_extern]
fn pg_tviews_handle_ddl_event() -> Result<(), Box<dyn std::error::Error>> {
    info!("pg_tviews: Event trigger fired");

    // Get information about the DDL command that just executed
    let commands = get_ddl_commands()?;

    for cmd in commands {
        // Only process CREATE TABLE and SELECT INTO
        if !matches!(cmd.command_tag.as_str(), "CREATE TABLE" | "SELECT INTO") {
            continue;
        }

        let table_name = cmd.object_identity;
        info!("pg_tviews: Checking table '{}'", table_name);

        // Check if this is a tv_* table
        if !table_name.starts_with("tv_") {
            info!("pg_tviews: Not a TVIEW table, ignoring");
            continue;
        }

        info!("pg_tviews: Converting '{}' to TVIEW", table_name);

        // Convert the newly-created table to a TVIEW
        match convert_table_to_tview(&table_name) {
            Ok(()) => {
                info!("pg_tviews: Successfully converted '{}' to TVIEW", table_name);
            }
            Err(e) => {
                // Log error but don't fail the transaction
                // The table was already created by PostgreSQL
                error!("pg_tviews: Failed to convert '{}' to TVIEW: {}", table_name, e);
                error!("pg_tviews: Table exists as regular table, not a TVIEW");
            }
        }
    }

    Ok(())
}

/// Get DDL commands from pg_event_trigger_ddl_commands()
fn get_ddl_commands() -> Result<Vec<DdlCommand>, Box<dyn std::error::Error>> {
    let mut commands = Vec::new();

    Spi::connect(|client| {
        let query = "SELECT command_tag, object_type, object_identity
                     FROM pg_event_trigger_ddl_commands()";

        let results = client.select(query, None, None)?;

        for row in results {
            let command_tag: String = row["command_tag"].value()?.unwrap_or_default();
            let object_type: String = row["object_type"].value()?.unwrap_or_default();
            let object_identity: String = row["object_identity"].value()?.unwrap_or_default();

            commands.push(DdlCommand {
                command_tag,
                object_type,
                object_identity,
            });
        }

        Ok::<_, spi::Error>(commands)
    })?;

    Ok(commands)
}

struct DdlCommand {
    command_tag: String,
    object_type: String,
    object_identity: String,
}

/// Convert an existing table to a TVIEW
///
/// This function is called AFTER PostgreSQL has created the table.
/// Strategy:
/// 1. Validate table structure (must have pk_*, id, data columns)
/// 2. Rename tv_entity → tv_entity_materialized (the backing table)
/// 3. Create view v_entity (user's original SELECT)
/// 4. Create tv_entity as a wrapper view
/// 5. Install triggers on base tables
/// 6. Register metadata
fn convert_table_to_tview(table_name: &str) -> TViewResult<()> {
    info!("pg_tviews: convert_table_to_tview called for '{}'", table_name);

    // Extract entity name
    let entity_name = table_name.strip_prefix("tv_")
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: "Table name must start with tv_".to_string(),
        })?;

    // Step 1: Validate table structure
    validate_tview_table_structure(table_name)?;

    // Step 2: Get the original SELECT from table definition
    // Since CREATE TABLE AS was used, we need to infer the SELECT
    // For now, we'll create a simple SELECT that reads from the materialized table
    let select_sql = infer_select_from_table(table_name)?;

    // Step 3: Rename the table to tv_entity_materialized
    let materialized_name = format!("{}_materialized", table_name);
    Spi::run(&format!("ALTER TABLE {} RENAME TO {}", table_name, materialized_name))?;
    info!("pg_tviews: Renamed {} → {}", table_name, materialized_name);

    // Step 4: Create view v_entity pointing to materialized table
    let view_name = format!("v_{}", entity_name);
    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM {}",
        view_name, materialized_name
    ))?;
    info!("pg_tviews: Created view {}", view_name);

    // Step 5: Create tv_entity as wrapper view
    Spi::run(&format!(
        "CREATE VIEW {} AS SELECT * FROM {}",
        table_name, view_name
    ))?;
    info!("pg_tviews: Created wrapper view {}", table_name);

    // Step 6: Register metadata
    // TODO: Find dependencies, install triggers, etc.
    // For Phase 1, we just register basic metadata
    register_basic_metadata(entity_name, &view_name, table_name, &select_sql)?;

    info!("pg_tviews: TVIEW '{}' created successfully", table_name);
    Ok(())
}

/// Validate that the table has the required TVIEW structure
fn validate_tview_table_structure(table_name: &str) -> TViewResult<bool> {
    // Check for required columns: pk_*, id, data
    let entity_name = table_name.strip_prefix("tv_").unwrap();
    let pk_column = format!("pk_{}", entity_name);

    let has_pk = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = '{}'
        )",
        table_name, pk_column
    ))?.unwrap_or(false);

    let has_id = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = 'id'
        )",
        table_name
    ))?.unwrap_or(false);

    let has_data = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.columns
            WHERE table_name = '{}' AND column_name = 'data'
        )",
        table_name
    ))?.unwrap_or(false);

    if !has_pk || !has_id || !has_data {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!(
                "TVIEW table must have columns: {}, id, data. Found: pk={}, id={}, data={}",
                pk_column, has_pk, has_id, has_data
            ),
        });
    }

    Ok(true)
}

/// Infer the original SELECT statement from the table structure
fn infer_select_from_table(table_name: &str) -> TViewResult<String> {
    // For Phase 1, return a placeholder
    // In Phase 2, we'll improve this to reconstruct the original SELECT
    Ok(format!("SELECT * FROM {}", table_name))
}

/// Register basic metadata for the TVIEW
fn register_basic_metadata(
    entity_name: &str,
    view_name: &str,
    tview_name: &str,
    definition: &str,
) -> TViewResult<()> {
    // Get OIDs
    let view_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}'",
        view_name
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for view {}", view_name),
        pg_error: "View not found".to_string(),
    })?;

    let table_oid = Spi::get_one::<pg_sys::Oid>(&format!(
        "SELECT oid FROM pg_class WHERE relname = '{}'",
        tview_name
    ))?.ok_or_else(|| TViewError::CatalogError {
        operation: format!("Get OID for table {}", tview_name),
        pg_error: "Table not found".to_string(),
    })?;

    // Insert metadata
    Spi::run(&format!(
        "INSERT INTO pg_tview_meta (entity, view_oid, table_oid, definition)
         VALUES ('{}', {}, {}, '{}')
         ON CONFLICT (entity) DO UPDATE SET
            view_oid = EXCLUDED.view_oid,
            table_oid = EXCLUDED.table_oid,
            definition = EXCLUDED.definition",
        entity_name.replace("'", "''"),
        view_oid.as_u32(),
        table_oid.as_u32(),
        definition.replace("'", "''")
    ))?;

    Ok(())
}
```

**`sql/event_triggers.sql`** (installed by migration)

```sql
-- Event trigger for CREATE TABLE interception
-- Fires AFTER the table is created, providing safe SPI context

CREATE OR REPLACE FUNCTION pg_tviews_handle_ddl_event()
RETURNS event_trigger
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
BEGIN
    -- Loop through all objects created by this DDL command
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands()
    LOOP
        -- Log for debugging
        RAISE INFO 'pg_tviews: DDL event - command_tag=%, object_type=%, object_identity=%',
            obj.command_tag, obj.object_type, obj.object_identity;

        -- Only process CREATE TABLE and SELECT INTO
        IF obj.command_tag IN ('CREATE TABLE', 'SELECT INTO') THEN
            -- Check if table name starts with tv_
            IF obj.object_identity LIKE 'public.tv_%' OR obj.object_identity LIKE 'tv_%' THEN
                RAISE INFO 'pg_tviews: Detected TVIEW creation: %', obj.object_identity;

                -- Call Rust function to convert table to TVIEW
                -- This will be implemented in Phase 2
                -- For now, just log
            END IF;
        END IF;
    END LOOP;
END;
$$;

-- Create the event trigger
DROP EVENT TRIGGER IF EXISTS pg_tviews_ddl_end;
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'SELECT INTO')
    EXECUTE FUNCTION pg_tviews_handle_ddl_event();

-- Add comment
COMMENT ON EVENT TRIGGER pg_tviews_ddl_end IS
'Intercepts CREATE TABLE tv_* commands and converts them to TVIEWs';
```

#### Modifications to Existing Files

**`src/lib.rs`** - Add event_trigger module

```rust
mod event_trigger; // Add this line

// Rest of file...
```

#### Verification Commands

```bash
# After Phase 1, this should work:
psql -h localhost -p 28817 -d postgres << 'EOF'
-- Create test table (will be intercepted by event trigger)
CREATE TABLE tb_test (id INT, name TEXT);
INSERT INTO tb_test VALUES (1, 'Alice'), (2, 'Bob');

-- Try DDL syntax (event trigger fires but doesn't convert yet)
CREATE TABLE tv_test AS
SELECT id as pk_test, id, jsonb_build_object('id', id, 'name', name) as data
FROM tb_test;

-- Check logs - should see "pg_tviews: Event trigger fired"
-- Table should exist as regular table (conversion not implemented yet)
\d tv_test

-- Cleanup
DROP TABLE tv_test;
DROP TABLE tb_test;
EOF
```

**Expected Output**:
```
INFO: pg_tviews: Event trigger fired
INFO: pg_tviews: Detected TVIEW creation: public.tv_test
INFO: pg_tviews: Converting 'tv_test' to TVIEW
INFO: pg_tviews: convert_table_to_tview called for 'tv_test'
```

#### Acceptance Criteria
- [ ] Event trigger function compiles without errors
- [ ] Event trigger is created and registered
- [ ] Event trigger fires on CREATE TABLE tv_*
- [ ] No panics or crashes
- [ ] Logs show event trigger activation

---

### **Phase 2: Implement Table-to-TVIEW Conversion** ⏱️ 3 hours

**Objective**: Implement the logic to convert an existing table to a TVIEW.

#### Files to Create

**`src/ddl/convert.rs`**

```rust
//! Convert existing tables to TVIEWs
//!
//! This module handles converting a table that was created by standard
//! PostgreSQL DDL into a proper TVIEW structure.

use pgrx::prelude::*;
use crate::error::{TViewError, TViewResult};
use crate::schema::TViewSchema;

/// Convert an existing table to a TVIEW
///
/// # Strategy
///
/// PostgreSQL has already created tv_entity as a regular table.
/// We need to:
/// 1. Validate it has TVIEW structure (pk_*, id, data columns)
/// 2. Extract the data
/// 3. Create backing view v_entity (reconstructed SELECT)
/// 4. Recreate tv_entity as a view that reads from v_entity
/// 5. Install triggers on base tables
/// 6. Populate metadata
///
/// # Challenges
///
/// - We don't have the original SELECT statement
/// - Must infer base tables from data
/// - Must handle edge cases (empty tables, complex JOINs)
pub fn convert_existing_table_to_tview(
    table_name: &str,
) -> TViewResult<()> {
    let entity_name = extract_entity_name(table_name)?;

    info!("Converting existing table '{}' to TVIEW", table_name);

    // Step 1: Validate structure
    validate_tview_structure(table_name, entity_name)?;

    // Step 2: Infer schema from table
    let schema = infer_schema_from_table(table_name)?;

    // Step 3: Extract existing data (will be restored later)
    let data_backup = backup_table_data(table_name, &schema)?;

    // Step 4: Get base tables (infer from data or require user hint)
    // For Phase 2, we'll require the base table to be specified
    // In Phase 3, we can add smarter inference
    let base_tables = infer_base_tables(table_name)?;

    // Step 5: Drop the existing table
    Spi::run(&format!("DROP TABLE {} CASCADE", table_name))?;
    info!("Dropped existing table '{}'", table_name);

    // Step 6: Reconstruct as proper TVIEW
    reconstruct_as_tview(
        table_name,
        entity_name,
        &schema,
        &base_tables,
        &data_backup,
    )?;

    info!("Successfully converted '{}' to TVIEW", table_name);
    Ok(())
}

/// Validate that table has required TVIEW structure
fn validate_tview_structure(table_name: &str, entity_name: &str) -> TViewResult<()> {
    let pk_col = format!("pk_{}", entity_name);

    // Check required columns exist
    let columns = get_table_columns(table_name)?;

    let has_pk = columns.iter().any(|c| c.name == pk_col);
    let has_id = columns.iter().any(|c| c.name == "id");
    let has_data = columns.iter().any(|c| c.name == "data");

    if !has_pk || !has_id || !has_data {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!(
                "Table must have TVIEW structure: {}, id, data. Found: {}",
                pk_col,
                columns.iter().map(|c| &c.name).collect::<Vec<_>>().join(", ")
            ),
        });
    }

    // Validate types
    let id_col = columns.iter().find(|c| c.name == "id").unwrap();
    if id_col.data_type != "uuid" {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!("Column 'id' must be UUID, found {}", id_col.data_type),
        });
    }

    let data_col = columns.iter().find(|c| c.name == "data").unwrap();
    if data_col.data_type != "jsonb" {
        return Err(TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: format!("Column 'data' must be JSONB, found {}", data_col.data_type),
        });
    }

    Ok(())
}

struct ColumnInfo {
    name: String,
    data_type: String,
    is_nullable: bool,
}

fn get_table_columns(table_name: &str) -> TViewResult<Vec<ColumnInfo>> {
    let mut columns = Vec::new();

    Spi::connect(|client| {
        let query = format!(
            "SELECT column_name, data_type, is_nullable
             FROM information_schema.columns
             WHERE table_name = '{}'
             ORDER BY ordinal_position",
            table_name.replace("'", "''")
        );

        let results = client.select(&query, None, None)?;

        for row in results {
            columns.push(ColumnInfo {
                name: row["column_name"].value()?.unwrap_or_default(),
                data_type: row["data_type"].value()?.unwrap_or_default(),
                is_nullable: row["is_nullable"].value::<String>()?.unwrap_or_default() == "YES",
            });
        }

        Ok::<_, spi::Error>(())
    })?;

    Ok(columns)
}

fn infer_schema_from_table(table_name: &str) -> TViewResult<TViewSchema> {
    // Implementation in Phase 2
    todo!("Infer schema from existing table structure")
}

fn backup_table_data(table_name: &str, schema: &TViewSchema) -> TViewResult<Vec<BackupRow>> {
    // Implementation in Phase 2
    todo!("Backup table data before dropping")
}

fn infer_base_tables(table_name: &str) -> TViewResult<Vec<String>> {
    // Implementation in Phase 2
    // For now, return empty (no triggers installed)
    Ok(Vec::new())
}

fn reconstruct_as_tview(
    table_name: &str,
    entity_name: &str,
    schema: &TViewSchema,
    base_tables: &[String],
    data_backup: &[BackupRow],
) -> TViewResult<()> {
    // Implementation in Phase 2
    todo!("Reconstruct table as proper TVIEW")
}

struct BackupRow {
    // Fields based on TVIEW schema
}

fn extract_entity_name(table_name: &str) -> TViewResult<&str> {
    table_name.strip_prefix("tv_")
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: table_name.to_string(),
            reason: "Table name must start with tv_".to_string(),
        })
}
```

#### Verification Commands

```bash
# Test basic conversion
psql -h localhost -p 28817 -d postgres << 'EOF'
CREATE TABLE tb_convert (id INT PRIMARY KEY, name TEXT);
INSERT INTO tb_convert VALUES (1, 'Test1'), (2, 'Test2');

-- This should now fully convert to TVIEW
CREATE TABLE tv_convert AS
SELECT id as pk_convert, gen_random_uuid() as id,
       jsonb_build_object('id', id, 'name', name) as data
FROM tb_convert;

-- Verify TVIEW structure
SELECT * FROM pg_tview_meta WHERE entity = 'convert';
SELECT * FROM pg_views WHERE viewname = 'v_convert';
SELECT * FROM tv_convert;

-- Cleanup
DROP TABLE tv_convert CASCADE;
DROP TABLE tb_convert;
EOF
```

#### Acceptance Criteria
- [ ] Table is successfully converted to TVIEW
- [ ] Metadata is registered
- [ ] View is created
- [ ] Data is preserved
- [ ] No panics or errors

---

### **Phase 3: Simplify ProcessUtility Hook** ⏱️ 1 hour

**Objective**: Remove creation logic from hook, keep only validation.

#### Modifications

**`src/hooks.rs`** - Simplify to validation-only

```rust
/// Handle CREATE TABLE tv_* AS SELECT ...
///
/// NEW BEHAVIOR (Phase 3):
/// - Validate the TVIEW syntax
/// - Return false (let PostgreSQL create the table)
/// - Event trigger will convert it to TVIEW afterwards
unsafe fn handle_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    query_string: *const ::std::os::raw::c_char,
) -> bool {
    // Extract table name and SELECT statement
    let (table_name, select_sql) = match extract_table_and_select(ctas, query_string) {
        Ok(data) => data,
        Err(e) => {
            error!("Failed to parse CREATE TABLE AS: {}", e);
            return false; // Let PostgreSQL handle it
        }
    };

    // Check if it's a tv_* table
    if !table_name.starts_with("tv_") {
        return false; // Not a TVIEW, pass through
    }

    info!("Validating TVIEW DDL syntax for '{}'", table_name);

    // Validate TVIEW SELECT statement structure
    match validate_tview_select(&select_sql) {
        Ok(()) => {
            info!("TVIEW syntax valid, letting PostgreSQL create table");
            info!("Event trigger will convert to TVIEW afterwards");
            return false; // Pass through - let PostgreSQL create it
        }
        Err(e) => {
            // Validation failed - prevent table creation
            error!("Invalid TVIEW syntax for '{}': {}", table_name, e);
            error!("TVIEW must have: pk_<entity>, id (UUID), data (JSONB) columns");

            // Return false but set an error flag
            // Actually, we can't prevent the CREATE from happening here
            // Best we can do is log and let event trigger handle it
            return false;
        }
    }
}

/// Validate TVIEW SELECT statement structure
fn validate_tview_select(select_sql: &str) -> Result<(), String> {
    // Check for required patterns in SELECT
    // This is basic validation - event trigger will do thorough validation

    if !select_sql.to_lowercase().contains(" as pk_") {
        return Err("Missing pk_<entity> column".to_string());
    }

    if !select_sql.to_lowercase().contains("jsonb_build_object") {
        return Err("Missing jsonb_build_object for data column".to_string());
    }

    Ok(())
}
```

#### Verification

```bash
# Test validation
psql -h localhost -p 28817 -d postgres << 'EOF'
-- Valid TVIEW syntax - should work
CREATE TABLE tv_valid AS
SELECT 1 as pk_valid, gen_random_uuid() as id,
       jsonb_build_object('test', 'data') as data;

-- Check it was converted
SELECT * FROM pg_tview_meta WHERE entity = 'valid';

DROP TABLE tv_valid CASCADE;
EOF
```

#### Acceptance Criteria
- [ ] Hook only validates, doesn't create
- [ ] Invalid syntax is logged (but doesn't prevent creation yet)
- [ ] Event trigger handles all creation
- [ ] No panics

---

### **Phase 4: Handle Edge Cases** ⏱️ 2 hours

**Objective**: Handle complex scenarios and edge cases.

#### Edge Cases to Handle

1. **Empty tables** - No data to infer from
2. **Complex SELECTs** - JOINs, subqueries, CTEs
3. **Schema specification** - Allow users to specify base tables
4. **Rollback handling** - What if conversion fails mid-way?
5. **Concurrent DDL** - Multiple CREATE TABLE tv_* in parallel

#### Implementation

**Add HINT system for complex cases:**

```sql
-- User can provide hints via table comment
CREATE TABLE tv_complex AS
SELECT ... FROM ... JOIN ...;

COMMENT ON TABLE tv_complex IS
'TVIEW_BASES: tb_users, tb_orders';
```

**Add transaction savepoints:**

```rust
fn convert_existing_table_to_tview(table_name: &str) -> TViewResult<()> {
    // Create savepoint in case conversion fails
    Spi::run("SAVEPOINT tview_conversion")?;

    match do_conversion(table_name) {
        Ok(()) => {
            Spi::run("RELEASE SAVEPOINT tview_conversion")?;
            Ok(())
        }
        Err(e) => {
            error!("Conversion failed: {}, rolling back", e);
            Spi::run("ROLLBACK TO SAVEPOINT tview_conversion")?;
            Err(e)
        }
    }
}
```

#### Verification

Test all edge cases

#### Acceptance Criteria
- [ ] Empty tables handled gracefully
- [ ] Complex SELECTs supported (or clear error)
- [ ] Hints system works
- [ ] Rollback on error
- [ ] Concurrent DDL safe

---

### **Phase 5: Migration and Compatibility** ⏱️ 1.5 hours

**Objective**: Create migration path from old to new system.

#### Files to Create

**`sql/pg_tviews--0.1.0--0.2.0.sql`** (upgrade script)

```sql
-- Migration from ProcessUtility hook to Event Triggers
-- Version 0.1.0 → 0.2.0

-- Drop old event trigger if exists
DROP EVENT TRIGGER IF EXISTS pg_tviews_ddl_end CASCADE;

-- Create new event trigger function (calls Rust function)
CREATE OR REPLACE FUNCTION pg_tviews_handle_ddl_event()
RETURNS event_trigger
SECURITY DEFINER
LANGUAGE plpgsql
AS $$
DECLARE
    obj record;
    table_name text;
BEGIN
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands()
    LOOP
        -- Only process CREATE TABLE and SELECT INTO
        IF obj.command_tag IN ('CREATE TABLE', 'SELECT INTO')
           AND obj.object_type = 'table' THEN

            -- Extract table name from object_identity
            table_name := obj.object_identity;

            -- Remove schema prefix if present
            IF table_name LIKE '%.%' THEN
                table_name := split_part(table_name, '.', 2);
            END IF;

            -- Check if TVIEW table
            IF table_name LIKE 'tv_%' THEN
                RAISE INFO 'pg_tviews: Converting % to TVIEW', table_name;

                -- Call Rust conversion function
                PERFORM pg_tviews_convert_table(table_name);
            END IF;
        END IF;
    END LOOP;
EXCEPTION
    WHEN OTHERS THEN
        -- Log error but don't fail transaction
        RAISE WARNING 'pg_tviews: Error in event trigger: %', SQLERRM;
END;
$$;

-- Register event trigger
CREATE EVENT TRIGGER pg_tviews_ddl_end
    ON ddl_command_end
    WHEN TAG IN ('CREATE TABLE', 'SELECT INTO')
    EXECUTE FUNCTION pg_tviews_handle_ddl_event();

-- Add conversion function to public API
CREATE OR REPLACE FUNCTION pg_tviews_convert_table(table_name text)
RETURNS void
LANGUAGE plrust
AS $$
    // Calls the Rust convert function
    use crate::event_trigger::convert_existing_table_to_tview;
    convert_existing_table_to_tview(table_name)?;
    Ok(())
$$;

COMMENT ON FUNCTION pg_tviews_convert_table IS
'Convert an existing table to a TVIEW (called by event trigger)';
```

#### Deprecation Plan

1. Keep ProcessUtility hook for PostgreSQL < 9.3 (if needed)
2. Add deprecation warning in logs
3. Update documentation
4. Remove hook in version 0.3.0

#### Acceptance Criteria
- [ ] Migration script runs without errors
- [ ] Event trigger takes over from hook
- [ ] Existing TVIEWs continue to work
- [ ] Documentation updated

---

### **Phase 6: Testing and Documentation** ⏱️ 2 hours

**Objective**: Comprehensive testing and documentation.

#### Test Cases

**`test/sql/event_triggers/01_basic.sql`**

```sql
-- Basic event trigger tests

\set ECHO all

-- Setup
CREATE TABLE tb_basic (id SERIAL PRIMARY KEY, name TEXT, value INT);
INSERT INTO tb_basic VALUES (1, 'Alice', 100), (2, 'Bob', 200);

-- Test 1: CREATE TABLE AS with TVIEW syntax
CREATE TABLE tv_basic AS
SELECT id as pk_basic,
       gen_random_uuid() as id,
       jsonb_build_object('id', id, 'name', name, 'value', value) as data
FROM tb_basic;

-- Verify metadata
SELECT 'Metadata registered' as test,
       EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'basic') as passed;

-- Verify view created
SELECT 'View created' as test,
       EXISTS(SELECT 1 FROM pg_views WHERE viewname = 'v_basic') as passed;

-- Verify data
SELECT 'Data count' as test, COUNT(*) = 2 as passed FROM tv_basic;

-- Verify structure
SELECT 'Has pk column' as test,
       EXISTS(SELECT 1 FROM information_schema.columns
              WHERE table_name = 'v_basic' AND column_name = 'pk_basic') as passed;

-- Test 2: DROP TABLE cleanup
DROP TABLE tv_basic CASCADE;

SELECT 'Cleanup complete' as test,
       NOT EXISTS(SELECT 1 FROM pg_tview_meta WHERE entity = 'basic') as passed;

-- Cleanup
DROP TABLE tb_basic;
```

**`test/sql/event_triggers/02_complex.sql`** - Complex JOIN scenarios
**`test/sql/event_triggers/03_edge_cases.sql`** - Empty tables, errors, etc.
**`test/sql/event_triggers/04_concurrent.sql`** - Parallel CREATE TABLE

#### Documentation Updates

**`docs/DDL_SYNTAX.md`**

```markdown
# DDL Syntax for TVIEWs

## Overview

pg_tviews supports creating TVIEWs using standard SQL DDL syntax via **Event Triggers**.

## How It Works

1. You use standard `CREATE TABLE tv_* AS SELECT ...` syntax
2. PostgreSQL creates the table normally
3. An event trigger fires AFTER creation completes
4. The event trigger converts the table to a proper TVIEW
5. This happens transparently in milliseconds

## Requirements

- PostgreSQL 9.3+ (for event triggers)
- TVIEW SELECT must have: `pk_<entity>`, `id` (UUID), `data` (JSONB)

## Basic Example

\`\`\`sql
-- Create base table
CREATE TABLE tb_products (
    id SERIAL PRIMARY KEY,
    name TEXT,
    price DECIMAL(10,2)
);

-- Create TVIEW using DDL syntax
CREATE TABLE tv_products AS
SELECT
    id as pk_products,
    gen_random_uuid() as id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'price', price
    ) as data
FROM tb_products;

-- TVIEW is ready to use!
SELECT * FROM tv_products;
\`\`\`

## Behind the Scenes

What the event trigger does:
1. Validates table structure (must have pk_*, id, data)
2. Creates view `v_products`
3. Renames table to backing store
4. Installs triggers on base tables
5. Registers metadata

## Error Handling

If the table doesn't have proper TVIEW structure:
- Error is logged to PostgreSQL logs
- Table remains as regular table (not converted)
- User sees warning message

## Comparison with Function Syntax

| Method | Pros | Cons |
|--------|------|------|
| **DDL Syntax** | Standard SQL, familiar | Requires PG 9.3+, slight delay |
| **Function Syntax** | Works everywhere, explicit | Less familiar, verbose |

Both methods create identical TVIEWs.
```

#### Acceptance Criteria
- [ ] All tests pass
- [ ] Documentation complete
- [ ] Examples working
- [ ] Performance benchmarks done

---

## 🎯 Success Metrics

### Before Event Triggers (Current State)
- ❌ DDL syntax causes panics
- ❌ Cannot use SPI in hooks
- ⚠️ Only function syntax works
- ⚠️ Poor user experience

### After Event Triggers (Target State)
- ✅ DDL syntax works perfectly
- ✅ No panics (safe SPI context)
- ✅ Both DDL and function syntax work
- ✅ Industry-standard approach
- ✅ Better error handling
- ✅ Excellent user experience

### Performance Impact
- Event trigger overhead: <1ms per CREATE TABLE
- Total time: ~5-10ms for TVIEW conversion
- No impact on query performance
- No impact on non-TVIEW tables

---

## 🔧 Rollback Plan

If event triggers cause issues:

1. **Immediate rollback** (< 5 minutes):
   ```sql
   DROP EVENT TRIGGER pg_tviews_ddl_end;
   -- Falls back to function syntax only
   ```

2. **Full rollback** to previous version:
   ```sql
   DROP EXTENSION pg_tviews CASCADE;
   CREATE EXTENSION pg_tviews VERSION '0.1.0';
   ```

3. **Emergency disable**:
   ```sql
   ALTER EVENT TRIGGER pg_tviews_ddl_end DISABLE;
   ```

---

## 📝 Notes and Considerations

### Why Not Keep ProcessUtility Hook?

**Reasons to remove it:**
1. Cannot safely use SPI (fundamental limitation)
2. More complex state management
3. Higher risk of crashes
4. Not the modern PostgreSQL way

**Reasons to keep it:**
1. Validation before table creation
2. Compatibility with PG < 9.3
3. Slightly faster (no table creation overhead)

**Decision**: Keep hook for validation ONLY, remove creation logic.

### Alternative: Both Hook + Event Trigger

**Hybrid approach:**
- Hook validates syntax, prevents invalid DDL
- Event trigger does conversion after validation passes
- Best user experience (fail fast + safe conversion)

### Known Limitations

1. **Brief table existence**: Table exists for ~1-5ms before conversion
   - Not visible to concurrent transactions
   - No practical impact

2. **Cannot reconstruct complex SELECTs**: If SELECT has complex JOINs/CTEs
   - Solution: Store original SELECT in table comment
   - Or: Use function syntax for complex cases

3. **PostgreSQL 9.3+ required**: Event triggers not available before 9.3
   - Solution: Keep function syntax for older versions
   - PostgreSQL 9.3 released in 2013 (12 years old)

---

## 🚀 Post-Implementation

### Version 0.3.0 (Future)
- Remove ProcessUtility hook entirely
- Event triggers only
- Cleaner codebase

### Version 0.4.0 (Future)
- Smart SELECT reconstruction
- Auto-detect base tables from data
- Advanced error recovery

---

## 📚 References

- [PostgreSQL Event Triggers Documentation](https://www.postgresql.org/docs/current/event-triggers.html)
- [TimescaleDB Source Code](https://github.com/timescale/timescaledb) - See `src/event_trigger.c`
- [Citus Source Code](https://github.com/citusdata/citus) - See `src/backend/distributed/ddl/`
- [pgrx Event Trigger Example](https://github.com/pgcentralfoundation/pgrx/tree/develop/pgrx-examples)

---

**End of Implementation Plan**

*Ready to implement? Start with Phase 1 and proceed sequentially.*
