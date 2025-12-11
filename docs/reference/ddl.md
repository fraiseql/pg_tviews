# DDL Reference

Complete reference for CREATE TVIEW and DROP TVIEW syntax with FraiseQL patterns.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

pg_tviews extends PostgreSQL DDL with the `CREATE TVIEW` and `DROP TVIEW` commands for managing transactional materialized views. TVIEWs follow FraiseQL's trinity identifier pattern and CQRS architecture.

## CREATE TVIEW

### Syntax

```sql
CREATE TVIEW tv_<entity> AS
SELECT
    <pk_column> as pk_<entity>,  -- Required: lineage root
    <uuid_column> as id,         -- Optional: GraphQL ID
    <other_columns>,             -- Optional: cascade FKs, filtering FKs
    <jsonb_data> as data         -- Required: JSONB read model
FROM tb_<entity> t
[LEFT JOIN tb_<related> r ON ...]
[WHERE ...]
[GROUP BY ...];
```

### FraiseQL Naming Conventions

Following FraiseQL patterns:

- **TVIEW name**: `tv_<entity>` (e.g., `tv_post`, `tv_user`)
- **Source tables**: `tb_<entity>` (e.g., `tb_post`, `tb_user`)
- **Backing view**: `v_<entity>` (automatically created)
- **Entity name**: Derived from TVIEW name by removing `tv_` prefix

### Required Columns

#### Primary Key Column (`pk_<entity>`)

Every TVIEW must have exactly one primary key column named `pk_<entity>`:

```sql
-- Correct: Follows trinity pattern
SELECT p.pk_post as pk_post, ... FROM tb_post p

-- Incorrect: Wrong name
SELECT p.id as pk_post, ... FROM tb_post p  -- ERROR: not lineage root

-- Incorrect: Wrong type
SELECT p.id::bigint as pk_post, ... FROM tb_post p  -- ERROR: not original PK
```

**Requirements**:
- Must be named `pk_<entity>` where `<entity>` matches TVIEW name
- Must be the actual primary key from source table (no casting)
- Used for lineage tracking and cascade propagation

#### JSONB Data Column (`data`)

Every TVIEW must have exactly one JSONB column named `data`:

```sql
-- Correct: JSONB read model
jsonb_build_object(
    'id', p.id,
    'title', p.title,
    'author', jsonb_build_object('id', u.id, 'name', u.name)
) as data

-- Incorrect: Wrong type
jsonb_build_object(...)::text as data  -- ERROR: not JSONB

-- Incorrect: Wrong name
jsonb_build_object(...) as json_data  -- ERROR: not named 'data'
```

**Best Practices**:
- Include all GraphQL-required fields
- Use nested objects for relationships
- Include UUIDs for GraphQL filtering
- Add computed fields as needed

### Optional Columns

#### Trinity Identifiers

Following FraiseQL's trinity pattern:

```sql
SELECT
    p.pk_post as pk_post,        -- Required: lineage root
    p.id as id,                  -- Optional: GraphQL ID (UUID)
    p.identifier as identifier,  -- Optional: SEO slug (text)
    p.fk_user as fk_user,        -- Optional: cascade FK (integer)
    u.id as user_id,             -- Optional: filtering FK (UUID)
    jsonb_build_object(...) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

#### Cascade Foreign Keys

Include all foreign keys used for cascade propagation:

```sql
-- Include FKs for automatic cascade updates
SELECT
    p.pk_post,
    p.fk_user,        -- Enables user → post cascades
    p.fk_category,    -- Enables category → post cascades
    jsonb_build_object(...) as data
FROM tb_post p;
```

#### Filtering Foreign Keys

Include UUID FKs for efficient GraphQL filtering:

```sql
-- Include UUID FKs for WHERE clauses
SELECT
    p.pk_post,
    u.id as user_id,        -- Filter posts by user UUID
    c.id as category_id,    -- Filter posts by category UUID
    jsonb_build_object(...) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
JOIN tb_category c ON p.fk_category = c.pk_category;
```

### Complete Examples

#### Simple TVIEW

```sql
CREATE TVIEW tv_user AS
SELECT
    u.pk_user as pk_user,
    u.id,
    u.identifier,
    u.name,
    jsonb_build_object(
        'id', u.id,
        'identifier', u.identifier,
        'name', u.name,
        'email', u.email,
        'createdAt', u.created_at
    ) as data
FROM tb_user u;
```

#### Complex TVIEW with Relationships

```sql
CREATE TVIEW tv_post AS
SELECT
    p.pk_post as pk_post,
    p.id,
    p.identifier,
    p.fk_user,
    u.id as user_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'createdAt', p.created_at,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name
        ),
        'comments', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'id', c.id,
                    'text', c.text,
                    'author', jsonb_build_object('id', cu.id, 'name', cu.name)
                )
            ) FILTER (WHERE c.id IS NOT NULL),
            '[]'::jsonb
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
LEFT JOIN tb_comment c ON c.fk_post = p.pk_post
LEFT JOIN tb_user cu ON c.fk_user = cu.pk_user
GROUP BY p.pk_post, p.id, p.identifier, p.title, p.content,
         p.created_at, p.fk_user, u.id, u.identifier, u.name;
```

### What Happens During CREATE TVIEW

1. **SQL Analysis**: Parses SELECT statement to identify dependencies
2. **Schema Inference**: Determines column types and relationships
3. **Backing View Creation**: Creates `v_<entity>` with your SELECT
4. **Materialized Table Creation**: Creates `tv_<entity>` table
5. **Trigger Installation**: Sets up triggers on all source tables
6. **Initial Population**: Fills TVIEW with current data
7. **Metadata Registration**: Records TVIEW in system catalogs

### Supported SQL Features

#### ✅ Supported

- **JOINs**: INNER, LEFT, RIGHT, FULL OUTER
- **Aggregations**: GROUP BY, HAVING, jsonb_agg(), array_agg()
- **Expressions**: CASE, COALESCE, NULLIF, FILTER
- **Subqueries**: In SELECT list (scalar subqueries)
- **Functions**: jsonb_build_object(), jsonb_array_elements(), etc.
- **Operators**: Standard PostgreSQL operators

#### ❌ Not Supported

- **Set Operations**: UNION, INTERSECT, EXCEPT
- **CTEs**: WITH clauses (Common Table Expressions)
- **Window Functions**: ROW_NUMBER(), RANK(), etc.
- **Recursive Queries**: Recursive CTEs
- **Self-Joins**: May cause dependency cycles
- **DISTINCT ON**: Use GROUP BY instead

### Limitations

- **Maximum Source Tables**: 10 tables per TVIEW (configurable)
- **Dependency Depth**: Performance degrades with >5 cascade levels
- **Circular Dependencies**: Automatically detected and rejected
- **Column Name Conflicts**: Must resolve ambiguous column names

## DROP TVIEW

### Syntax

```sql
DROP TVIEW [IF EXISTS] tv_<entity>;
```

### Examples

```sql
-- Drop a TVIEW
DROP TVIEW tv_post;

-- Safe drop (no error if doesn't exist)
DROP TVIEW IF EXISTS tv_missing;
```

### What Happens During DROP TVIEW

1. **Trigger Removal**: Uninstalls all triggers for this TVIEW
2. **Backing View Drop**: Removes `v_<entity>` view
3. **Materialized Table Drop**: Removes `tv_<entity>` table
4. **Metadata Cleanup**: Removes entry from system catalogs
5. **Dependency Check**: Fails if other TVIEWs depend on this one

### Cascade Behavior

**No CASCADE support in beta**: If other TVIEWs depend on this TVIEW, DROP will fail.

**Workaround**: Drop dependent TVIEWs first:

```sql
-- Find dependent TVIEWs (manual inspection for now)
-- Look for TVIEWs that reference this entity in their SELECT

-- Drop in reverse dependency order
DROP TVIEW tv_post_comments;  -- Depends on tv_post
DROP TVIEW tv_post;           -- Can now be dropped
```

## ALTER TVIEW

**Not supported in beta**. To modify a TVIEW:

```sql
-- Drop and recreate
DROP TVIEW tv_post;
CREATE TVIEW tv_post AS
SELECT ... -- new definition
FROM ...;
```

## Statement-Level Triggers

### Installation

```sql
-- Enable for 100-500× better bulk performance
SELECT pg_tviews_install_stmt_triggers();
```

**Benefits**:
- Processes entire statement at once using transition tables
- Dramatically faster for bulk operations
- Reduces trigger overhead from N× to 1× per statement

### Uninstallation

```sql
-- Revert to row-level triggers
SELECT pg_tviews_uninstall_stmt_triggers();
```

**When to use row-level triggers**:
- Single-row operations
- Debugging trigger behavior
- Compatibility requirements

## Troubleshooting

### CREATE TVIEW Errors

**"TVIEW name must follow tv_* convention"**
```sql
-- Fix: Use correct naming
CREATE TVIEW tv_post AS ...  -- ✅ Correct
CREATE TVIEW post_view AS ... -- ❌ Wrong
```

**"Missing required column: pk_post"**
```sql
-- Fix: Include primary key column
SELECT p.pk_post as pk_post, ...  -- ✅ Correct
SELECT p.id as pk_post, ...       -- ❌ Wrong column
```

**"Missing required column: data"**
```sql
-- Fix: Include JSONB data column
jsonb_build_object(...) as data  -- ✅ Correct
jsonb_build_object(...) as json  -- ❌ Wrong name
```

**"Dependency cycle detected"**
```sql
-- Fix: Restructure to avoid circular dependencies
-- TVIEW A references TVIEW B which references TVIEW A
```

**"Unsupported SQL feature: UNION"**
```sql
-- Fix: Rewrite without UNION
SELECT ... FROM table1
UNION                    -- ❌ Not supported
SELECT ... FROM table2

-- Alternative: Use separate TVIEWs or application logic
```

### DROP TVIEW Errors

**"Cannot drop tv_post: other TVIEWs depend on it"**
```sql
-- Fix: Drop dependent TVIEWs first
DROP TVIEW tv_post_comments;  -- Remove dependency
DROP TVIEW tv_post;           -- Now works
```

### Performance Issues

**Slow initial creation**:
- Complex SELECT with many JOINs
- Large tables (consider WHERE clauses for initial subset)

**Slow refreshes**:
- Deep cascade chains (>3 levels)
- Large JSONB objects (consider jsonb_ivm extension)

## Best Practices

### Schema Design

1. **Follow Trinity Pattern**: Use id/pk_/fk_ consistently
2. **Include All FKs**: Both integer (cascade) and UUID (filtering)
3. **Use Meaningful Identifiers**: SEO-friendly slugs where appropriate
4. **Plan Cascade Depth**: Keep dependency chains shallow (<3 levels)

### TVIEW Design

1. **One Entity Per TVIEW**: Focus each TVIEW on a single primary entity
2. **Include GraphQL Fields**: All fields needed for API responses
3. **Use Efficient JOINs**: Prefer INNER JOINs where possible
4. **Test with Real Data**: Verify performance with production-scale data

### Maintenance

1. **Monitor Dependencies**: Track which TVIEWs depend on others
2. **Plan Drop Order**: Know dependency chains for maintenance
3. **Test Changes**: Use staging environment for DDL changes
4. **Backup First**: Always backup before major DDL operations

## See Also

- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
- [API Reference](api.md)
- [Troubleshooting Guide](../operations/troubleshooting.md)