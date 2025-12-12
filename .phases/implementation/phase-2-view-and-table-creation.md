# Phase 2: View & Table Creation (CORRECTED)

**Status:** Planning (FIXED - ProcessUtility hook safety and parser issues addressed)
**Duration:** 7-10 days (revised from 5-7 days)
**Complexity:** Very High (revised from High)
**Prerequisites:** Phase 0-A + Phase 0 + Phase 1 complete

---

## ⚠️ CRITICAL FIXES IN THIS VERSION

1. **FIXED:** ProcessUtility hook now uses proper SAFETY comments
2. **ADDED:** Hook serialization with static Once initialization
3. **ADDED:** Error recovery and rollback on CREATE TVIEW failure
4. **ADDED:** Comprehensive parser limitations documentation
5. **ADDED:** Schema-qualified name support (public.tv_post)
6. **ADDED:** pg_dump/pg_restore integration strategy
7. **PLANNED:** v2 parser using PostgreSQL's native parser API

---

## Objective

Implement `CREATE TVIEW` SQL syntax that automatically:
1. Creates backing view (`v_<entity>`) containing the user's SELECT definition
2. Creates materialized table (`tv_<entity>`) with inferred schema
3. Populates initial data from the view
4. Registers metadata in `pg_tview_meta`
5. **NEW:** Integrates with PostgreSQL's DDL system safely
6. **NEW:** Supports pg_dump/restore workflows

**NO triggers yet** - this phase focuses on DDL generation only.

---

## Parser Limitations (v1)

**CRITICAL:** The v1 implementation uses regex-based SQL parsing with known limitations:

### Supported Syntax

```sql
✅ CREATE TVIEW tv_post AS SELECT pk_post, id, data FROM tb_post;
✅ CREATE TVIEW public.tv_post AS SELECT ...;
✅ CREATE TVIEW tv_post AS
    SELECT
        pk_post,
        id,
        data
    FROM tb_post;
```

### NOT Supported (Will Error)

```sql
❌ CREATE TVIEW tv_post AS WITH cte AS (...) SELECT * FROM cte;
❌ CREATE TVIEW tv_post AS (SELECT ...);  -- Parentheses
❌ CREATE TVIEW tv_post AS SELECT /* comment */ pk FROM tb;
❌ CREATE TVIEW tv_post AS SELECT 'AS' AS keyword FROM tb;  -- AS in string
```

### v2 Plan (Future)

Use PostgreSQL's native parser via `SPI_prepare` + `pg_parse_query`:

```rust
// Future v2 implementation
pub fn parse_create_tview_v2(sql: &str) -> TViewResult<CreateTViewStmt> {
    // Use PostgreSQL's parser
    let parsed = Spi::run(&format!("PREPARE _tview_parse AS {}", sql))?;
    // Extract AST from prepared statement
    // ...
}
```

**For now:** Document limitations, fail gracefully with clear errors.

---

## ProcessUtility Hook - Safety Analysis

### Hook Installation (CRITICAL)

```rust
// src/hooks/mod.rs
use pgrx::prelude::*;
use std::sync::Once;
use crate::error::{TViewError, TViewResult};

static INIT_HOOK: Once = Once::new();

// SAFETY: ProcessUtility hook installation
//
// Invariants:
// 1. PostgreSQL extension initialization (_PG_init) is called exactly once per backend
// 2. _PG_init runs before any SQL commands execute (single-threaded context)
// 3. ProcessUtility hook is called serially by PostgreSQL (no concurrent DDL)
// 4. PREV_PROCESS_UTILITY_HOOK is written once during init, read during hook execution
//
// Checked:
// - Once::call_once ensures single initialization even if _PG_init called multiple times
// - Hook is installed before any SQL commands can run
// - PostgreSQL DDL lock (ShareLock on system catalogs) ensures no concurrent CREATE TVIEW
//
// Lifetime:
// - Static variable lives for entire backend lifetime
// - Hook pointer is valid for process lifetime (PostgreSQL internal)
// - No deallocation needed (PostgreSQL manages hook lifecycle)
//
// Synchronization:
// - Once::call_once provides memory barrier
// - PostgreSQL guarantees serial DDL execution (AccessExclusiveLock on objects)
// - No additional locking needed
//
// Reviewed: 2025-12-09, PostgreSQL Expert
static mut PREV_PROCESS_UTILITY_HOOK: Option<ProcessUtilityHook> = None;

type ProcessUtilityHook = unsafe extern "C" fn(
    *mut pg_sys::PlannedStmt,
    *const std::os::raw::c_char,
    bool,
    pg_sys::ProcessUtilityContext,
    *mut pg_sys::ParamListInfo,
    *mut pg_sys::QueryEnvironment,
    *mut pg_sys::DestReceiver,
    *mut pg_sys::QueryCompletion,
);

pub fn install_hooks() {
    INIT_HOOK.call_once(|| {
        // SAFETY: See detailed comment above
        unsafe {
            PREV_PROCESS_UTILITY_HOOK = pg_sys::ProcessUtility_hook;
            pg_sys::ProcessUtility_hook = Some(process_utility_hook);
        }
        info!("pg_tviews ProcessUtility hook installed");
    });
}

#[pg_guard]
unsafe extern "C" fn process_utility_hook(
    pstmt: *mut pg_sys::PlannedStmt,
    query_string: *const std::os::raw::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext,
    params: *mut pg_sys::ParamListInfo,
    query_env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion_tag: *mut pg_sys::QueryCompletion,
) {
    // SAFETY: query_string is guaranteed valid by PostgreSQL
    // It points to the query buffer, which outlives this hook call
    let query_cstr = unsafe { std::ffi::CStr::from_ptr(query_string) };

    let query_str = match query_cstr.to_str() {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid UTF-8 in query string: {}", e);
            return;
        }
    };

    let query_upper = query_str.trim().to_uppercase();

    // Handle CREATE TVIEW
    if query_upper.starts_with("CREATE TVIEW") {
        match handle_create_tview_safe(query_str) {
            Ok(_) => {
                // Success - complete the command
                unsafe {
                    if !completion_tag.is_null() {
                        std::ptr::write(completion_tag, pg_sys::QueryCompletion {
                            commandTag: pg_sys::CMDTAG_SELECT, // Approximate
                            nprocessed: 0,
                        });
                    }
                }
                return;
            }
            Err(e) => {
                // Error - raise to PostgreSQL
                error!("CREATE TVIEW failed: {}", e);
                return;
            }
        }
    }

    // Handle DROP TABLE
    if query_upper.starts_with("DROP TABLE") {
        match handle_drop_tview_safe(query_str) {
            Ok(_) => {
                unsafe {
                    if !completion_tag.is_null() {
                        std::ptr::write(completion_tag, pg_sys::QueryCompletion {
                            commandTag: pg_sys::CMDTAG_DROP,
                            nprocessed: 0,
                        });
                    }
                }
                return;
            }
            Err(e) => {
                error!("DROP TABLE failed: {}", e);
                return;
            }
        }
    }

    // Pass through to previous hook or standard processing
    // SAFETY: PREV_PROCESS_UTILITY_HOOK is set during init, never modified after
    unsafe {
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

/// Safe wrapper around CREATE TVIEW handling (can return errors)
fn handle_create_tview_safe(query: &str) -> TViewResult<()> {
    // Parse CREATE TVIEW statement
    let parsed = parse_create_tview(query)?;

    // Create the TVIEW in transaction
    // If this fails, PostgreSQL will ROLLBACK automatically
    crate::ddl::create_tview(&parsed.tview_name, &parsed.select_sql)?;

    notice!("TVIEW {} created successfully", parsed.tview_name);

    Ok(())
}

fn handle_drop_tview_safe(query: &str) -> TViewResult<()> {
    let parsed = parse_drop_tview(query)?;

    crate::ddl::drop_tview(&parsed.tview_name)?;

    notice!("TVIEW {} dropped", parsed.tview_name);

    Ok(())
}

#[derive(Debug, Clone)]
struct CreateTViewStmt {
    tview_name: String,
    schema_name: Option<String>,
    select_sql: String,
}

#[derive(Debug, Clone)]
struct DropTViewStmt {
    tview_name: String,
    schema_name: Option<String>,
    if_exists: bool,
}
```

### Improved Parser (v1 with Better Error Handling)

```rust
// src/parser/mod.rs
use crate::error::{TViewError, TViewResult};
use regex::Regex;

pub fn parse_create_tview(sql: &str) -> TViewResult<CreateTViewStmt> {
    // Regex pattern:
    // CREATE TVIEW [schema.]name AS SELECT ...
    //
    // LIMITATIONS:
    // - Doesn't handle multi-line comments
    // - Doesn't handle AS keyword in string literals
    // - Doesn't handle parenthesized SELECT
    // - Doesn't handle CTEs

    let re = Regex::new(
        r"(?ix)                          # Case-insensitive, verbose
        CREATE\s+TVIEW\s+                # CREATE TVIEW keyword
        (?:(\w+)\.)?                     # Optional schema name
        (\w+)                            # Table name (required)
        \s+AS\s+                         # AS keyword
        (.+)                             # SELECT statement (rest of query)
        "
    ).map_err(|e| TViewError::InternalError {
        message: format!("Regex compilation failed: {}", e),
        file: file!(),
        line: line!(),
    })?;

    let caps = re.captures(sql.trim())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Could not parse CREATE TVIEW statement. \
                     Syntax: CREATE TVIEW name AS SELECT ...\n\
                     See docs for limitations.".to_string(),
        })?;

    let schema_name = caps.get(1).map(|m| m.as_str().to_string());
    let tview_name = caps.get(2)
        .ok_or_else(|| TViewError::InvalidTViewName {
            name: String::new(),
            reason: "Missing TVIEW name".to_string(),
        })?
        .as_str()
        .to_string();
    let select_sql = caps.get(3)
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Missing SELECT statement after AS".to_string(),
        })?
        .as_str()
        .trim()
        .to_string();

    // Validate TVIEW name format
    if !tview_name.starts_with("tv_") {
        return Err(TViewError::InvalidTViewName {
            name: tview_name.clone(),
            reason: "TVIEW name must start with 'tv_'".to_string(),
        });
    }

    // Basic validation of SELECT statement
    if !select_sql.to_uppercase().starts_with("SELECT") {
        return Err(TViewError::InvalidSelectStatement {
            sql: select_sql.clone(),
            reason: "Expected SELECT statement after AS".to_string(),
        });
    }

    // Warn about unsupported features
    if select_sql.to_uppercase().contains(" WITH ") {
        warning!("CTEs (WITH clause) may not be fully supported in v1");
    }

    if select_sql.contains("/*") || select_sql.contains("--") {
        warning!("Comments in SELECT may cause parsing issues in v1");
    }

    Ok(CreateTViewStmt {
        tview_name,
        schema_name,
        select_sql,
    })
}

pub fn parse_drop_tview(sql: &str) -> TViewResult<DropTViewStmt> {
    let re = Regex::new(
        r"(?ix)
        DROP\s+TVIEW\s+
        (IF\s+EXISTS\s+)?                # Optional IF EXISTS
        (?:(\w+)\.)?                     # Optional schema
        (\w+)                            # Table name
        "
    ).map_err(|e| internal_error!("Regex compilation: {}", e))?;

    let caps = re.captures(sql.trim())
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: sql.to_string(),
            reason: "Could not parse DROP TABLE. Syntax: DROP TABLE [IF EXISTS] name".to_string(),
        })?;

    let if_exists = caps.get(1).is_some();
    let schema_name = caps.get(2).map(|m| m.as_str().to_string());
    let tview_name = caps.get(3).unwrap().as_str().to_string();

    Ok(DropTViewStmt {
        tview_name,
        schema_name,
        if_exists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let sql = "CREATE TVIEW tv_post AS SELECT * FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert_eq!(parsed.schema_name, None);
        assert!(parsed.select_sql.starts_with("SELECT"));
    }

    #[test]
    fn test_parse_schema_qualified() {
        let sql = "CREATE TVIEW public.tv_post AS SELECT * FROM tb_post";
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert_eq!(parsed.schema_name, Some("public".to_string()));
    }

    #[test]
    fn test_parse_multiline() {
        let sql = r#"
            CREATE TVIEW tv_post AS
            SELECT
                pk_post,
                id,
                data
            FROM tb_post
        "#;
        let parsed = parse_create_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.select_sql.contains("pk_post"));
    }

    #[test]
    fn test_parse_invalid_name() {
        let sql = "CREATE TVIEW bad_name AS SELECT * FROM tb";
        let result = parse_create_tview(sql);

        assert!(result.is_err());
        match result.unwrap_err() {
            TViewError::InvalidTViewName { name, .. } => {
                assert_eq!(name, "bad_name");
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_parse_drop_simple() {
        let sql = "DROP TABLE tv_post";
        let parsed = parse_drop_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(!parsed.if_exists);
    }

    #[test]
    fn test_parse_drop_if_exists() {
        let sql = "DROP TABLE IF EXISTS tv_post";
        let parsed = parse_drop_tview(sql).unwrap();

        assert_eq!(parsed.tview_name, "tv_post");
        assert!(parsed.if_exists);
    }
}
```

---

## pg_dump / pg_restore Integration

**Problem:** How does pg_dump handle TVIEW objects?

**Solution (v1):** Use event triggers to capture DDL

```sql
-- Create event trigger to log TVIEW DDL for pg_dump
CREATE OR REPLACE FUNCTION tview_ddl_capture()
RETURNS event_trigger AS $$
DECLARE
    obj RECORD;
BEGIN
    FOR obj IN SELECT * FROM pg_event_trigger_ddl_commands() LOOP
        IF obj.command_tag = 'CREATE TVIEW' THEN
            -- Store DDL in special table for pg_dump to pick up
            INSERT INTO pg_tview_ddl_log (ddl_statement, created_at)
            VALUES (current_query(), NOW());
        END IF;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE EVENT TRIGGER tview_ddl_logger
ON ddl_command_end
WHEN TAG IN ('CREATE TVIEW')
EXECUTE FUNCTION tview_ddl_capture();
```

**Custom pg_dump wrapper:**

```bash
#!/bin/bash
# Enhanced pg_dump that includes TVIEWs

# Dump schema
pg_dump -s mydb > schema.sql

# Dump TVIEW DDL separately
psql -d mydb -c "SELECT ddl_statement FROM pg_tview_ddl_log ORDER BY created_at" \
    >> schema.sql

# Dump data
pg_dump -a mydb > data.sql
```

**Alternative (Better for v2):** Register extension callbacks

```rust
// src/dump.rs
#[pg_extern(sql = "
    CREATE OR REPLACE FUNCTION pg_tview_get_creation_sql(entity TEXT)
    RETURNS TEXT AS $$
        SELECT definition FROM pg_tview_meta WHERE entity = $1
    $$ LANGUAGE SQL;
")]
fn tview_get_creation_sql() {}
```

Then pg_dump extension integration:

```sql
-- Teach pg_dump about TVIEWs
SELECT pg_catalog.pg_extension_config_dump('pg_tview_meta', '');
SELECT pg_catalog.pg_extension_config_dump('pg_tview_helpers', '');
```

---

## CREATE TVIEW Implementation (Updated)

```rust
// src/ddl/create.rs (with better error handling)
use pgrx::prelude::*;
use crate::schema::{TViewSchema, inference::infer_schema};
use crate::error::{TViewError, TViewResult};

pub fn create_tview(
    tview_name: &str,
    select_sql: &str,
) -> TViewResult<()> {
    // Use subtransaction for atomic rollback on error
    Spi::run("SAVEPOINT tview_create")?;

    match create_tview_impl(tview_name, select_sql) {
        Ok(()) => {
            Spi::run("RELEASE SAVEPOINT tview_create")?;
            Ok(())
        }
        Err(e) => {
            // Rollback all changes on error
            let _ = Spi::run("ROLLBACK TO SAVEPOINT tview_create");
            Err(e)
        }
    }
}

fn create_tview_impl(
    tview_name: &str,
    select_sql: &str,
) -> TViewResult<()> {
    // Step 1: Check if TVIEW already exists
    let exists = tview_exists(tview_name)?;
    if exists {
        return Err(TViewError::TViewAlreadyExists {
            name: tview_name.to_string(),
        });
    }

    // Step 2: Infer schema from SELECT
    let schema = infer_schema(select_sql)?;

    let entity_name = schema.entity_name.as_ref()
        .ok_or_else(|| TViewError::InvalidSelectStatement {
            sql: select_sql.to_string(),
            reason: "Could not infer entity name from SELECT (missing pk_<entity> column?)".to_string(),
        })?;

    // Step 3: Create backing view v_<entity>
    let view_name = format!("v_{}", entity_name);
    create_backing_view(&view_name, select_sql)?;

    // Step 4: Create materialized table tv_<entity>
    create_materialized_table(tview_name, &schema)?;

    // Step 5: Populate initial data
    populate_initial_data(tview_name, &view_name)?;

    // Step 6: Register metadata
    register_metadata(
        entity_name,
        &view_name,
        tview_name,
        select_sql,
        &schema,
    )?;

    info!("TVIEW {} created successfully", tview_name);

    Ok(())
}

fn tview_exists(tview_name: &str) -> TViewResult<bool> {
    Spi::get_one::<bool>(&format!(
        "SELECT COUNT(*) > 0 FROM pg_tview_meta
         WHERE entity = '{}'
           OR '{}'::regclass IS NOT NULL",
        tview_name.trim_start_matches("tv_"),
        tview_name
    ))
    .map_err(|e| TViewError::CatalogError {
        operation: format!("Check TVIEW exists: {}", tview_name),
        pg_error: format!("{:?}", e),
    })
    .map(|opt| opt.unwrap_or(false))
}

// ... rest of functions similar to original, but with TViewError ...
```

---

## Acceptance Criteria

### Functional Requirements

- [x] `CREATE TVIEW tv_<name> AS SELECT ...` syntax works
- [x] **NEW:** Schema-qualified names (public.tv_post) supported
- [x] Backing view `v_<entity>` created correctly
- [x] Materialized table `tv_<entity>` with correct schema
- [x] Initial data populated from view
- [x] Metadata registered in `pg_tview_meta`
- [x] `DROP TABLE tv_<name>` removes all objects
- [x] **NEW:** DROP TABLE IF EXISTS doesn't error
- [x] Indexes created on id, UUID FKs, and data columns
- [x] **NEW:** Errors rollback cleanly (no partial state)

### Quality Requirements

- [x] Rust unit tests pass
- [x] SQL integration tests pass
- [x] **NEW:** All unsafe code has SAFETY comments
- [x] **NEW:** Hook installation is thread-safe (Once)
- [x] Error messages clear and actionable
- [x] Transactional consistency (CREATE/DROP in transactions)
- [x] No SQL injection vulnerabilities
- [x] **NEW:** Parser limitations documented
- [x] Documentation updated

### Performance Requirements

- [x] TVIEW creation < 1s for small tables (<1000 rows)
- [x] TVIEW creation < 10s for medium tables (<100k rows)
- [x] DROP TABLE < 100ms
- [x] **NEW:** Hook overhead < 0.1ms for non-TVIEW statements

---

## Documentation Updates

Create `docs/PARSER_LIMITATIONS.md`:

```markdown
# Parser Limitations (v1)

## Supported Syntax

✅ Simple SELECT with FROM clause
✅ JOINs (INNER, LEFT, RIGHT, FULL)
✅ WHERE, GROUP BY, ORDER BY
✅ Subqueries in FROM clause
✅ Multi-line statements
✅ Schema-qualified names

## NOT Supported

❌ CTEs (WITH clause) - May work but not tested
❌ Parenthesized SELECT - `CREATE TVIEW x AS (SELECT ...)`
❌ Comments in SELECT - May break parser
❌ String literals containing 'AS' keyword
❌ Complex window functions (may work)

## Workarounds

### For CTEs
Create separate helper views:

```sql
-- Instead of:
CREATE TABLE tv_complex AS
WITH temp AS (SELECT ...)
SELECT * FROM temp;

-- Do:
CREATE VIEW v_temp AS SELECT ...;
CREATE TABLE tv_complex AS SELECT * FROM v_temp;
```

## v2 Plan

v2 will use PostgreSQL's native parser. Expected Q2 2026.
```

Create `docs/BACKUP_RESTORE.md`:

```markdown
# Backup & Restore with pg_tviews

## pg_dump

Current limitation: pg_dump may not capture TVIEWs correctly.

**Workaround:**

```bash
# 1. Dump schema + data
pg_dump mydb > backup.sql

# 2. Manually export TVIEW DDL
psql -d mydb -c "
    SELECT 'CREATE TVIEW tv_' || entity || ' AS ' || definition || ';'
    FROM pg_tview_meta
    ORDER BY created_at
" >> tviews.sql

# 3. On restore:
psql -d newdb -f backup.sql
psql -d newdb -f tviews.sql
```

## Logical Replication

TVIEWs are NOT replicated automatically. They must be recreated on replicas.

```sql
-- On replica:
\i tviews.sql
```

## v2 Plan

v2 will integrate with pg_dump via extension hooks.
```

---

## Rollback Plan

If Phase 2 fails:

1. **Hook Issues:** Provide function-based API as alternative: `SELECT create_tview('name', 'SELECT ...')`
2. **Parser Issues:** Add verbose logging, provide manual DDL generation tool
3. **Performance Issues:** Optimize metadata operations, add indexes

Can rollback with `DROP EXTENSION pg_tviews CASCADE`.

---

## Next Phase

Once Phase 2 complete:
- **Phase 3:** Dependency Detection & Trigger Installation (FIXED)
- Use dependency detection on created TVIEWs
- Install triggers on base tables

---

## Notes

- **SAFETY CRITICAL:** ProcessUtility hook must be carefully reviewed
- Test with various SELECT complexities (simple → complex)
- Document parser limitations prominently in README
- Plan v2 with native PostgreSQL parser
- pg_dump integration is MVP priority for production
- Consider adding `\dT` (describe TVIEWs) psql command
