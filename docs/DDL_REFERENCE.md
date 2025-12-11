# pg_tviews DDL Reference

**Version**: 0.1.0-alpha

## Overview

This document describes the DDL (Data Definition Language) commands for creating and managing TVIEWs.

## CREATE TVIEW

### Syntax

```sql
CREATE TVIEW tv_<entity> AS
SELECT ...
```

**Important**: TVIEW names must follow the `tv_*` prefix convention.

### Naming Conventions

- **TVIEW name**: `tv_<entity>` (e.g., `tv_post`, `tv_user`)
- **Entity name**: Derived from TVIEW name by removing `tv_` prefix
- **Source tables**: `tb_<entity>` (e.g., `tb_post`, `tb_user`)
- **Backing view**: `v_<entity>` (automatically created)

### Required Columns

The SELECT statement must include:

1. **Primary Key**: Column named `pk_<entity>` of type BIGINT or UUID
   ```sql
   p.id as pk_post  -- For tv_post
   ```

2. **JSONB Data**: Column named `data` of type JSONB
   ```sql
   jsonb_build_object(
       'id', p.id,
       'title', p.title,
       -- ... other fields
   ) as data
   ```

### Complete Example

```sql
CREATE TABLE tv_post AS
SELECT
    p.id as pk_post,
    p.title,
    p.content,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'name', u.name,
            'email', u.email
        ),
        'comments', COALESCE(
            jsonb_agg(
                jsonb_build_object('id', c.id, 'text', c.text)
            ) FILTER (WHERE c.id IS NOT NULL),
            '[]'::jsonb
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
LEFT JOIN tb_comments c ON c.fk_post = p.id
GROUP BY p.id, p.title, p.content, u.id, u.name, u.email;
```

### What Happens

When you CREATE TVIEW:

1. **Backing View Created**: `v_post` is created with your SELECT
2. **Materialized Table Created**: `tv_post` stores the cached data
3. **Dependencies Detected**: Analyzes FROM/JOIN to find source tables
4. **Triggers Installed**: Automatically installs triggers on source tables
5. **Initial Refresh**: Populates `tv_post` with current data

### Supported SQL Features

✅ **Supported**:
- JOINs (INNER, LEFT, RIGHT, FULL)
- WHERE clauses
- GROUP BY / HAVING
- jsonb_build_object()
- jsonb_agg()
- COALESCE, FILTER
- Array aggregations (ARRAY_AGG, ARRAY(...))
- Subqueries in SELECT list
- CASE expressions

❌ **Not Supported**:
- UNION / INTERSECT / EXCEPT
- WITH (CTEs) at top level
- Window functions (may work, not optimized)
- DISTINCT ON
- Self-joins (may cause issues)
- Recursive queries

### Limitations

- Maximum 10 source tables per TVIEW (Phase 7 limit)
- Circular dependencies detected and rejected
- View definition must be parseable by inference engine
- Performance degrades with >5 levels of TVIEW cascades

## DROP TVIEW

### Syntax

```sql
DROP TVIEW tv_<entity>;
```

### What Happens

When you DROP TVIEW:

1. **Triggers Removed**: Uninstalls all triggers for this TVIEW
2. **Backing View Dropped**: `v_<entity>` is dropped
3. **Materialized Table Dropped**: `tv_<entity>` is dropped
4. **Metadata Cleaned**: Entry removed from `pg_tview_meta`
5. **Dependent TVIEWs**: Must be dropped first (no CASCADE support yet)

### Example

```sql
-- Simple drop
DROP TABLE tv_post;

-- Check before dropping
SELECT entity, table_oid, view_oid
FROM pg_tview_meta
WHERE entity = 'post';

-- If dependencies exist, drop them first
DROP TVIEW tv_dependent_view;
DROP TABLE tv_post;
```

### Cascade Behavior

⚠️ **No CASCADE support in beta**: If other TVIEWs depend on this one, DROP will fail.

**Workaround**: Drop dependent TVIEWs first, then drop this one.

```sql
-- Find dependencies
SELECT entity
FROM pg_tview_meta
WHERE ... -- TODO: Add dependency query

-- Drop in reverse dependency order
DROP TVIEW tv_level3;
DROP TVIEW tv_level2;
DROP TVIEW tv_level1;
```

## ALTER TVIEW

⚠️ **Not supported in beta**: Use DROP + CREATE to modify TVIEWs.

```sql
-- To modify a TVIEW:
DROP TABLE tv_post;
CREATE TABLE tv_post AS SELECT ... -- new definition
```

## Statement-Level Triggers

### Installation

```sql
-- Install statement-level triggers for better performance
SELECT pg_tviews_install_stmt_triggers();
```

**Benefits**:
- 100-500× faster for bulk operations
- Uses transition tables (OLD/NEW tables)
- One trigger fire per statement instead of per row

**When to Use**:
- Bulk INSERT/UPDATE/DELETE operations
- Data warehouse ETL processes
- Migration scripts

### Uninstallation

```sql
-- Revert to row-level triggers
SELECT pg_tviews_uninstall_stmt_triggers();
```

**When to Uninstall**:
- Small, frequent single-row operations
- Compatibility with older PostgreSQL versions
- Debugging trigger behavior

## Troubleshooting

### CREATE TVIEW Fails

**Error**: `InvalidSelectStatement`
```sql
ERROR:  Invalid SELECT statement: [details]
```
**Solution**: Check that SELECT follows requirements (pk_*, data column, supported SQL)

**Error**: `DependencyCycle`
```sql
ERROR:  Dependency cycle detected: post -> comment -> post
```
**Solution**: TVIEWs cannot have circular dependencies. Restructure dependencies.

### DROP TVIEW Fails

**Error**: `DependentObjectsExist`
```sql
ERROR:  Cannot drop tv_post: other TVIEWs depend on it
```
**Solution**: Drop dependent TVIEWs first.

## See Also

- [API Reference](API_REFERENCE.md)
- [Operations Guide](OPERATIONS.md)
- [Debugging Guide](DEBUGGING.md)