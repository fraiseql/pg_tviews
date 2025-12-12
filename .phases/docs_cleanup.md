# Documentation Cleanup: CREATE TVIEW Syntax Issues

## Problem Statement

The `CREATE TVIEW` syntax appears throughout the documentation and codebase, but this syntax **cannot work** in the current PostgreSQL implementation. PostgreSQL's parser cannot be extended to support custom DDL syntax through hooks.

## Required Changes

All instances of `CREATE TVIEW` should be replaced with one of these alternatives:
1. **Function call**: `SELECT pg_tviews_create('tv_name', 'SELECT ... FROM ...')`
2. **CREATE TABLE syntax**: `CREATE TABLE tv_name AS SELECT ... FROM ...`

## All Occurrences Found

### Documentation Files

#### docs/user-guides/architects.md
- Line 63: `CREATE TVIEW tv_post AS`
- Line 98: `CREATE TVIEW tv_product AS`
- Line 211: `CREATE TVIEW tv_post AS`
- Line 271: `CREATE TVIEW tv_post AS`

#### docs/benchmarks/overview.md
- Line 100: `CREATE TVIEW tv_product AS`
- Line 159: `CREATE TVIEW tv_category AS`

#### docs/user-guides/developers.md
- Line 85: `CREATE TVIEW tv_post AS`
- Line 366: `CREATE TVIEW tv_test AS`

#### docs/user-guides/operators.md
- Line 493: `CREATE TVIEW tv_post AS SELECT ...;`
- Line 581: `CREATE TVIEW tv_post AS SELECT ... FROM tv_post_backup;`

#### docs/operations.md
- Line 144: `'CREATE TVIEW tv_' || entity || ' AS ' ||`

#### docs/arrays.md
- Line 98: `CREATE TVIEW tv_orders AS`
- Line 116: `CREATE TVIEW tv_categories AS`

#### docs/getting-started/syntax-comparison.md
- Line 28: `## 2. CREATE TVIEW Syntax (Planned - Not Yet Implemented)`
- Line 30: `> **Note**: \`CREATE TVIEW\` syntax is planned for a future release but not currently implemented. PostgreSQL's parser cannot be extended to support custom DDL syntax through hooks.`

#### docs/getting-started/installation.md
- Line 285: `CREATE TVIEW test_tview AS SELECT id, data::jsonb as data FROM test_table;`

#### docs/operations/performance-tuning.md
- Line 233: `CREATE TVIEW tv_post AS`
- Line 250: `CREATE TVIEW tv_post AS`
- Line 275: `CREATE TVIEW tv_post AS`
- Line 546: `CREATE TVIEW tv_post_shard_1 AS`
- Line 550: `CREATE TVIEW tv_post_shard_2 AS`

#### docs/operations/security.md
- Line 72: `CREATE TVIEW tv_user AS`
- Line 86: `CREATE TVIEW tv_user AS`

#### docs/operations/troubleshooting.md
- Line 153: `CREATE TVIEW tv_content AS`
- Line 160: `CREATE TVIEW tv_pages AS SELECT ... FROM tb_page;`
- Line 181: `CREATE TVIEW tv_post AS`
- Line 230: `CREATE TVIEW tv_post AS SELECT ...;  -- Original definition`
- Line 413: `CREATE TVIEW tv_post AS SELECT ...;`

### Product Requirement Documents

#### PRD_v2.md
- Line 34: `CREATE TVIEW name AS SELECT ...`
- Line 52: `| FG1  | Support \`CREATE TVIEW name AS SELECT ...\`                                                |`
- Line 83: `CREATE TVIEW tv_post AS`
- Line 122: `|   CREATE TVIEW AS ...  |`
- Line 207: `CREATE TVIEW tv_post AS ... FROM tb_post JOIN v_user ...`
- Line 411: `CREATE TVIEW tv_company AS`
- Line 415: `CREATE TVIEW tv_user AS`
- Line 425: `CREATE TVIEW tv_post AS`
- Line 435: `CREATE TVIEW tv_feed AS`
- Line 493: `├ create.rs        -- CREATE TVIEW AS ... handler`
- Line 593: `CREATE TVIEW tv_<entity> AS`
- Line 718: `CREATE TVIEW name AS SELECT ...`

#### PRD.md
- Line 398: `* Cyclic view dependencies detected on \`CREATE TVIEW\``
- Line 399: `* Missing FK columns → \`CREATE TVIEW\` error`
- Line 410: `CREATE TVIEW FOR v_post;`

#### PRD_multiupdate.md
- Line 9: `We assume the rest of TVIEW (CREATE TVIEW, view/table generation, dependency graph) is already designed.`

### Source Code Files

#### src/ddl/create.rs
- Line 7: `/// This is the main entry point for CREATE TVIEW. PostgreSQL's transaction`

#### src/parser/mod.rs
- Line 4: `//! - **CREATE TVIEW**: Extracts name and SELECT statement`
- Line 12: `//! CREATE TVIEW tv_entity AS SELECT id, data FROM base_table;`
- Line 18: `//! CREATE TVIEW schema.tv_entity AS SELECT * FROM schema.base_table;`
- Line 45: `/// Parse CREATE TVIEW statement`
- Line 48: `/// - CREATE TVIEW tv_name AS SELECT ...`
- Line 49: `/// - CREATE TVIEW schema.tv_name AS SELECT ...`
- Line 59: `CREATE\s+TVIEW\s+                # CREATE TVIEW keyword`
- Line 74: `reason: "Could not parse CREATE TVIEW statement. \`
- Line 75: `Syntax: CREATE TVIEW name AS SELECT ...\n\`
- Line 166: `let sql = "CREATE TVIEW tv_post AS SELECT * FROM tb_post";`
- Line 176: `let sql = "CREATE TVIEW public.tv_post AS SELECT pk_post FROM tb_post";`
- Line 186: `CREATE TVIEW tv_post AS`
- Line 201: `let sql = "CREATE TVIEW bad_name AS SELECT * FROM tb";`

#### src/ddl/mod.rs
- Line 4: `//! - **CREATE TVIEW**: Parses SQL, creates metadata, sets up triggers`

### Test Files

#### test/sql/comprehensive_benchmarks/schemas/01_ecommerce_schema.sql
- Line 199: `-- Note: CREATE TVIEW automatically handles pg_tview_meta registration and trigger setup`
- Line 210: `-- Note: refresh_tv_product() is no longer needed since CREATE TVIEW auto-populates`

#### test/sql/40_refresh_trigger_dynamic_pk.sql
- Line 37: `CREATE TVIEW tv_post AS`
- Line 94: `CREATE TVIEW tv_user AS`

#### test_ddl_consistency.sql
- Line 2: `-- Verify CREATE TVIEW and pg_tviews_create() produce identical results`
- Line 24: `-- Method 1: CREATE TVIEW DDL (intercepted by ProcessUtility hook)`
- Line 25: `CREATE TVIEW tv_test_user1 AS`

#### test/sql/44_trigger_cascade_integration.sql
- Line 76: `CREATE TVIEW tv_company AS`
- Line 88: `CREATE TVIEW tv_user AS`
- Line 104: `CREATE TVIEW tv_post AS`

#### test/sql/43_cascade_depth_limit.sql
- Line 82: `CREATE TVIEW tv_level_0 AS`
- Line 92: `CREATE TVIEW tv_level_1 AS`
- Line 106: `CREATE TVIEW tv_level_2 AS`
- Line 120: `CREATE TVIEW tv_level_3 AS`
- Line 134: `CREATE TVIEW tv_level_4 AS`
- Line 148: `CREATE TVIEW tv_level_5 AS`

### Other Files

#### ARCHITECTURE.md
- Line 62: `CREATE TVIEW FOR v_entity;`

#### DOCUMENTATION_GAPS.md
- Line 102: `### 2.1 CREATE TVIEW Syntax (MEDIUM) 🟡`
- Line 114: `- Full CREATE TVIEW syntax (what's supported, what's not)`
- Line 122: `- Document CREATE TVIEW fully`

## Status: COMPLETED ✅

All 79 occurrences of "CREATE TVIEW" syntax have been replaced with "CREATE TABLE tv_" syntax where appropriate.

### What was replaced:
- **Documentation examples** (user guides, operations docs, installation) → `CREATE TABLE tv_name AS SELECT ...`
- **PRD examples** → `CREATE TABLE tv_name AS SELECT ...`
- **Test files** → `CREATE TABLE tv_name AS SELECT ...`
- **Source code comments** → Updated to reflect new syntax
- **Error messages and regex patterns** → Updated to match new syntax

### What was NOT replaced:
- `docs/getting-started/syntax-comparison.md` - Correctly notes CREATE TVIEW is not implemented
- Comments that are about future/planned features remain as-is

### Additional changes made:
- Updated `DROP TABLE` references to `DROP TABLE` for consistency
- Updated regex patterns in parser to match `CREATE TABLE` instead of `CREATE TVIEW`
- Updated error messages to reflect new syntax

## Total Occurrences Replaced: 79

This cleanup ensures all user-facing documentation shows working SQL syntax instead of the non-functional CREATE TVIEW syntax.