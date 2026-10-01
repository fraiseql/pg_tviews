# DDL Reference

Complete reference for TVIEW creation and management with FraiseQL patterns.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

pg_tviews provides transactional materialized views through DDL and SQL functions. TVIEWs follow FraiseQL's trinity identifier pattern and CQRS architecture.

## Creating TVIEWs

### DDL Method: CREATE TABLE tv_* AS SELECT

**Syntax**:
```sql
CREATE TABLE tv_<entity> AS
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

**Note**: The ProcessUtility hook automatically intercepts `CREATE TABLE tv_* AS SELECT` statements and converts them to TVIEW creation. This provides DDL-like syntax for TVIEW creation.

### Function Method: pg_tviews_create()

**Syntax**:
```sql
SELECT pg_tviews_create('tv_<entity>', '
SELECT
    <pk_column> as pk_<entity>,  -- Required: lineage root
    <uuid_column> as id,         -- Optional: GraphQL ID
    <other_columns>,             -- Optional: cascade FKs, filtering FKs
    <jsonb_data> as data         -- Required: JSONB read model
FROM tb_<entity> t
[LEFT JOIN tb_<related> r ON ...]
[WHERE ...]
[GROUP BY ...]
');
```

**Note**: This is the programmatic approach that can be used in scripts and applications.

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
SELECT tb_post.id as pk_post, ... FROM tb_post  -- ERROR: not lineage root

-- Incorrect: Wrong type
SELECT tb_post.id::bigint as pk_post, ... FROM tb_post  -- ERROR: not original PK
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
CREATE TABLE tv_user AS
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
        'created_at', u.created_at
    ) as data
FROM tb_user u;
```

#### Complex TVIEW with Relationships

```sql
CREATE TABLE tv_post AS
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
        'created_at', p.created_at,
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

### What Happens During TVIEW Creation

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
- **Subqueries in the SELECT list** (`(SELECT …)`, `ARRAY(SELECT …)`, `EXISTS`):
  allowed, but a base table read *only* inside one is not cascaded: see
  [Tables no cascade reaches](#tables-no-cascade-reaches)
- **Functions**: jsonb_build_object(), jsonb_array_elements(), etc.
- **Operators**: Standard PostgreSQL operators
- **UNION / UNION ALL**: incremental refresh cascades to every branch's base
  table; branches must key on disjoint `pk_<entity>` values (otherwise
  `pg_tviews.union_duplicate_policy` governs the duplicate)
- **CTEs (`WITH`)**: cascade paths resolve through a CTE whose body reads one or
  several joined base tables, reads earlier CTEs, or is a UNION / UNION ALL. The
  columns the CTE joins on must pass base columns through unchanged (a computed
  join column cannot be traced back to a base row)
- **DISTINCT ON**: deduplicated read models; the DISTINCT ON key may be aliased in
  the SELECT list (e.g. `DISTINCT ON (c.id_contract) c.id_contract AS pk_contract`)

#### ❌ Not Supported

- **Set Operations**: INTERSECT, EXCEPT (only UNION / UNION ALL is tracked)
- **Recursive Queries**: `WITH RECURSIVE` (rejected at create time)
- **CTEs with subqueries in FROM, or INTERSECT / EXCEPT bodies**: the tview is
  created, but base tables reachable only through such a CTE do not cascade
- **Window Functions**: ROW_NUMBER(), RANK(), etc.
- **Self-Joins**: May cause dependency cycles
- **DISTINCT ON + cascade join**: a DISTINCT ON tview cannot also depend on joined
  tables that would require PK-based cascade paths (rejected at create time)

### How a write finds the TVIEW rows to refresh

When a TVIEW is created, pg_tviews reads PostgreSQL's query tree of its backing view
(views, CTEs, subqueries and `UNION` branches included) and records, per base table,
how a changed row maps to TVIEW keys (`tviews.registry.cascade_kinds`): the key is a
column of the row (`local`), a generated query over the changed rows finds it
(`mapped`, for chains of joins and non-equality conditions), a TVIEW it embeds
refreshes it (`propagated`), or nothing selective links them (`all_keys`). A `mapped`
query that would scan a large table sequentially is reported at create time with
the index that avoids it:

```
NOTICE:  writes to public.tb_sku map to tv_order keys with a sequential scan of tb_line
         (about 20000 rows); an index on tb_line (fk_sku) would make them cheaper
```

### Tables no cascade reaches

Triggers go on every base table the backing view reads. A write refreshes the TVIEW
when pg_tviews can map the changed row to TVIEW keys: through the TVIEW's own
`tb_<entity>`, a join it traces (also through CTEs and UNION branches), or a TVIEW it
embeds through `fk_<entity>`. A table read any other way is reported when the TVIEW is
created, and listed in `tviews.registry.uncascaded_tables`. Two common shapes:

- a table read only inside a subquery of the SELECT list
  (`ARRAY(SELECT l.sku FROM tb_line l WHERE l.fk_order = o.pk_order)`);
- a table read only through a plain view (not a TVIEW's `v_<entity>`), for example
  one with an aggregate (`LEFT JOIN v_order_lines v ON v.fk_order = o.pk_order`).

What happens is fixed per TVIEW by `pg_tviews.uncascaded_policy` at create time:

| Policy | At create time | On a write to such a table |
|---|---|---|
| `warn` (default) | `WARNING:  writes to public.tb_line will not refresh public.tv_order (read in a subquery, or through a join pg_tviews cannot map)` | nothing: the rows stay stale until a mapped table changes |
| `error` | `ERROR` with the same text; nothing is created | — |
| `full_refresh` | `NOTICE` | the whole TVIEW is brought up to date at flush, once per transaction or statement; unchanged rows are not rewritten |

`full_refresh` recomputes every row of the TVIEW: on a 100 000-row TVIEW that is about
a second per flush that wrote to such a table. Use it for small TVIEWs, or rewrite the
definition so that the table is joined on a column pg_tviews can trace.

```sql
SET pg_tviews.uncascaded_policy = 'full_refresh';
SELECT pg_tviews_create('tv_order', $$ … $$);
RESET pg_tviews.uncascaded_policy;          -- the TVIEW keeps full_refresh
```

### Limitations

- **Maximum Source Tables**: 10 tables per TVIEW (configurable)
- **Dependency Depth**: Performance degrades with >5 cascade levels
- **Circular Dependencies**: Automatically detected and rejected
- **Column Name Conflicts**: Must resolve ambiguous column names

## DROP TABLE tv_*

### Syntax

```sql
DROP TABLE [IF EXISTS] tv_<entity> [CASCADE];
```

### Examples

```sql
-- Drop a TVIEW
DROP TABLE tv_post;

-- Safe drop (no error if doesn't exist)
DROP TABLE IF EXISTS tv_missing;

-- Drop with CASCADE (drops dependent objects)
DROP TABLE tv_post CASCADE;
```

### What Happens During DROP TABLE tv_*

1. **Trigger Removal**: Uninstalls all triggers for this TVIEW
2. **Backing View Drop**: Removes `v_<entity>` view
3. **Materialized Table Drop**: Removes `tv_<entity>` table
4. **Metadata Cleanup**: Removes entry from system catalogs
5. **Dependency Check**: Fails if other TVIEWs depend on this one

### Cascade Behavior

**CASCADE behavior**: PostgreSQL's standard CASCADE option is supported.

**Drop dependent TVIEWs first** (without CASCADE):

```sql
-- Find dependent TVIEWs (manual inspection for now)
-- Look for TVIEWs that reference this entity in their SELECT

-- Drop in reverse dependency order
DROP TABLE tv_post_comments;  -- Depends on tv_post
DROP TABLE tv_post;           -- Can now be dropped
```

**Or use CASCADE** (drops all dependents automatically):

```sql
DROP TABLE tv_post CASCADE;  -- Drops tv_post and all dependent TVIEWs
```

## ALTER TVIEW

Change a TVIEW's definition or storage with `pg_tviews_create_or_replace()`, which makes
the smallest change (`altered`, `replaced` in place, or `rebuilt`):

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_post', $$ SELECT ... -- new definition $$);
```

## Triggers

Creating a TVIEW installs, on each table its definition reads, a row-level trigger
(`tviews.pg_tview_trigger_handler`) that queues the affected keys, and a
statement-level trigger (`tviews.pg_tview_flush_trigger`) that refreshes them once per
statement. Nothing needs installing by hand. `tviews.pg_tviews_health_check()`
reports missing or orphaned triggers; `SELECT * FROM tviews.pg_tviews_reregister_all()`
re-installs any that are missing.

## Troubleshooting

### TVIEW Creation Errors

**"TVIEW name must follow tv_* convention"**
```sql
-- Fix: Use correct naming
SELECT pg_tviews_create('tv_post', '...');  -- ✅ Correct
SELECT pg_tviews_create('post_view', '...'); -- ❌ Wrong
```

**"Missing required column: pk_post"**
```sql
-- Fix: Include primary key column in SELECT
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

**"INTERSECT/EXCEPT set operations are not supported for cascade paths"**
```sql
-- UNION / UNION ALL are supported and cascade to every branch.
-- INTERSECT and EXCEPT are not (their set-difference semantics are not tracked).
SELECT ... FROM table1
INTERSECT                -- ❌ Not supported
SELECT ... FROM table2

-- Alternative: Use separate TVIEWs or application logic
```

### DROP TABLE tv_* Errors

**"Cannot drop tv_post: other TVIEWs depend on it"**
```sql
-- Fix: Drop dependent TVIEWs first
DROP TABLE tv_post_comments;  -- Remove dependency
DROP TABLE tv_post;           -- Now works

-- Or use CASCADE
DROP TABLE tv_post CASCADE;   -- Drops all dependents
```

### Performance Issues

**Slow initial creation**:
- Complex SELECT with many JOINs
- Large tables (consider WHERE clauses for initial subset)

**Slow refreshes**:
- Deep cascade chains (>3 levels)
- Large JSONB objects (consider jsonb_delta extension)

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