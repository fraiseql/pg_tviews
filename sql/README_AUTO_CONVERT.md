# Automatic TVIEW Conversion

This directory contains SQL functions to automatically detect and convert `tv_*` tables to TVIEWs.

## Problem

When creating a large schema with many tables, manually registering each one as a TVIEW is tedious and error-prone. The standard pg_tviews workflow requires:

1. Create base table `tb_entity`
2. Create backing view `v_entity` with appropriate SELECT
3. Call `pg_tviews_create('entity', 'SELECT ...')`

## Solution

The `auto_convert_tviews.sql` module provides two functions:

### 1. `pg_tviews_auto_convert_plan(schema_name text)`

**Dry-run mode**: Shows what would be created without making changes.

```sql
SELECT * FROM pg_tviews_auto_convert_plan('public');
```

Returns:
- `entity`: Entity name (e.g., "post")
- `base_table`: Qualified base table name (e.g., "public.tb_post")
- `backing_view`: Qualified view name to be created (e.g., "public.v_post")
- `select_sql`: The SELECT statement for the backing view

### 2. `pg_tviews_auto_convert(schema_name text)`

**Execute mode**: Actually creates backing views and registers TVIEWs.

```sql
SELECT * FROM pg_tviews_auto_convert('public');
```

Returns:
- `entity`: Entity name
- `status`: 'SUCCESS', 'SKIP', or 'ERROR'
- `message`: Detailed status message

## Usage Pattern

### Step 1: Create base tables with `tb_*` naming

Your schema should have tables like:
- `tb_post` - base table
- `tb_user` - base table
- `tb_comment` - base table

These become materialized view tables: `tv_post`, `tv_user`, `tv_comment`

### Step 2: Review the conversion plan

Before applying changes, preview what will happen:

```sql
-- Load the helper functions
\i sql/auto_convert_tviews.sql

-- Preview the conversion
SELECT entity, base_table, backing_view FROM pg_tviews_auto_convert_plan();
```

### Step 3: Apply automatic conversion

```sql
-- Execute the conversion
SELECT entity, status, message FROM pg_tviews_auto_convert();
```

### Step 4: Verify TVIEWs are registered

```sql
SELECT entity_name, tview_oid FROM pg_tview_meta ORDER BY entity_name;
```

## Integration into Schema Build

Add this to your build process after creating base tables:

```bash
# schema.sql contains your base table definitions
psql -d mydb -f schema.sql

# Auto-convert base tables to TVIEWs
psql -d mydb -f sql/auto_convert_tviews.sql
psql -d mydb -c "SELECT entity, status FROM pg_tviews_auto_convert();"

# Optionally verify
psql -d mydb -c "SELECT COUNT(*) as tview_count FROM pg_tview_meta;"
```

## How It Works

For each `tv_*` table found:

1. **Extract entity name**: `tv_post` → `post`
2. **Locate base table**: Look for `tb_post`
3. **List columns**: From `tb_post`, get all column names
4. **Create backing view**: 
   ```sql
   CREATE VIEW v_post AS
   SELECT pk_post, <all columns> FROM tb_post
   ```
5. **Register TVIEW**:
   ```sql
   SELECT pg_tviews_create('post', 'SELECT pk_post, ... FROM v_post')
   ```

## Notes

- **Primary Key**: Assumes `pk_<entity>` column exists in base table
- **Schema-aware**: Works with any schema, defaults to 'public'
- **Safe**: Skips missing base tables, provides detailed error messages
- **Idempotent**: Can be safely re-run (drops and recreates views)

## Example

Given this schema:

```sql
CREATE TABLE tb_post (
    pk_post INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    title TEXT NOT NULL,
    content TEXT
);

CREATE TABLE tv_post (
    pk_post INTEGER PRIMARY KEY,
    id UUID,
    title TEXT,
    content TEXT
);
```

Running:

```sql
SELECT * FROM pg_tviews_auto_convert();
```

Will:
1. Create `v_post` view selecting from `tb_post`
2. Register TVIEW 'post' with pg_tviews
3. Return status: `entity='post', status='SUCCESS', message='TVIEW registered: ...'`

After this, `tv_post` becomes a materialized JSONB view managed by pg_tviews, with:
- Automatic refresh on `tb_post` changes
- Change tracking and queue management
- All pg_tviews features active
