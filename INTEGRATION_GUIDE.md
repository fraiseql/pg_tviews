# Integration Guide: Automatic TVIEW Conversion

This guide shows how to integrate automatic TVIEW conversion into your schema build workflow.

## Quick Start

### 1. Define your schema with `tb_*` tables

Create your base tables using the `tb_<entity>` naming convention:

```sql
-- schema/01_base_tables.sql

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    email TEXT NOT NULL,
    name TEXT NOT NULL
);

CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    title TEXT NOT NULL,
    content TEXT NOT NULL
);

CREATE TABLE tb_comment (
    pk_comment BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    fk_post BIGINT NOT NULL REFERENCES tb_post(pk_post),
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    text TEXT NOT NULL
);
```

### 2. Create corresponding `tv_*` materialized view tables

These will become the JSONB materialized views:

```sql
-- schema/02_materialized_views.sql

CREATE TABLE tv_user (
    pk_user BIGINT PRIMARY KEY,
    data JSONB NOT NULL,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE tv_post (
    pk_post BIGINT PRIMARY KEY,
    data JSONB NOT NULL,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE tv_comment (
    pk_comment BIGINT PRIMARY KEY,
    data JSONB NOT NULL,
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);
```

### 3. Build and convert

```bash
#!/bin/bash

# 1. Create extension
psql -d mydb -c "CREATE EXTENSION IF NOT EXISTS pg_tviews;"

# 2. Apply base schema
psql -d mydb -f schema/01_base_tables.sql
psql -d mydb -f schema/02_materialized_views.sql

# 3. DRY RUN: Preview conversion
echo "Preview TVIEW conversion:"
psql -d mydb -f sql/auto_convert_tviews.sql
psql -d mydb -c "
    SELECT entity, base_table, backing_view 
    FROM pg_tviews_auto_convert_plan()
    ORDER BY entity;
"

# 4. Execute conversion
echo "Converting tv_* tables to TVIEWs:"
psql -d mydb -c "
    SELECT entity, status, message 
    FROM pg_tviews_auto_convert()
    ORDER BY entity;
"

# 5. Verify
echo "Registered TVIEWs:"
psql -d mydb -c "
    SELECT entity_name, tview_oid 
    FROM pg_tview_meta 
    ORDER BY entity_name;
"

# 6. Seed data (now works with TVIEWs active)
psql -d mydb -f schema/03_seed_data.sql
```

## Alternative: Use the Helper Script

```bash
# Show what would be converted (dry run)
./sql/convert_tviews.sh -d mydb -s public --plan

# Execute conversion
./sql/convert_tviews.sh -d mydb -s public
```

## Define your TVIEW Metadata

After conversion, you can define TVIEW-specific metadata (foreign keys, dependencies) by updating `pg_tview_meta`:

```sql
-- schema/03_tview_metadata.sql

-- Add foreign key relationships (if needed)
UPDATE pg_tview_meta
SET fk_columns = '{fk_user}'::TEXT[]
WHERE entity_name = 'post';

UPDATE pg_tview_meta
SET fk_columns = '{fk_user,fk_post}'::TEXT[]
WHERE entity_name = 'comment';
```

## Define your Backing Views

The auto-conversion creates basic backing views. You can customize them afterward:

```sql
-- schema/04_backing_views.sql

-- Override with custom backing view (if needed)
DROP VIEW IF EXISTS v_post;
CREATE VIEW v_post AS
SELECT
    pk_post,
    id,
    fk_user,
    title,
    content
FROM tb_post;

-- Re-register with custom view
SELECT pg_tviews_create('post', $$
    SELECT
        pk_post,
        jsonb_build_object(
            'id', id,
            'title', title,
            'content', content,
            'fk_user', fk_user
        ) AS data
    FROM tb_post
$$);
```

## Verify Everything Works

```bash
# 1. Check TVIEWs are registered
psql -d mydb -c "SELECT * FROM pg_tview_meta;"

# 2. Check backing views exist
psql -d mydb -c "
    SELECT * FROM information_schema.views 
    WHERE table_name ~ '^v_' 
    ORDER BY table_name;
"

# 3. Test automatic refresh
psql -d mydb -c "
    INSERT INTO tb_user (email, name) VALUES ('alice@example.com', 'Alice');
    SELECT * FROM tv_user;  -- Should be automatically populated
"

# 4. Check queue operations
psql -d mydb -c "SELECT * FROM pg_tview_queue LIMIT 5;"
```

## Building Schemas with Multiple Database Systems

If you're building for multiple schemas or databases, create a build script:

```bash
#!/bin/bash
# build.sh

for db in development staging; do
    echo "Building $db..."
    
    # Create extension
    psql -d "$db" -c "CREATE EXTENSION IF NOT EXISTS pg_tviews;"
    
    # Build schema
    psql -d "$db" -f schema.sql
    
    # Convert automatically
    ./sql/convert_tviews.sh -d "$db"
    
    echo "$db complete."
done
```

## Troubleshooting

### No TVIEWs registered
Check that:
1. Extension is installed: `SELECT * FROM pg_extension WHERE extname = 'pg_tviews';`
2. Base tables exist with `tb_*` prefix
3. Primary keys follow naming convention `pk_<entity>`

```sql
-- Debug: see what would be converted
SELECT * FROM pg_tviews_auto_convert_plan();

-- Debug: see conversion errors
SELECT * FROM pg_tviews_auto_convert();
```

### Backing views not created
Check permissions and syntax:

```sql
-- Manual creation for debugging
CREATE VIEW v_post AS SELECT pk_post, * FROM tb_post;

-- Test pg_tviews_create manually
SELECT pg_tviews_create('post', 'SELECT pk_post, * FROM tb_post');
```

### Foreign key errors during conversion
Ensure base tables exist before conversion:

```sql
-- This must work first
SELECT * FROM pg_tviews_auto_convert_plan();

-- If entities are missing, check:
SELECT tablename FROM pg_tables WHERE tablename LIKE 'tb_%' ORDER BY tablename;
```

## Next Steps

After conversion:

1. **Monitor refresh operations**: Check `pg_tview_queue` and `pg_tview_meta_audit`
2. **Define foreign keys**: Update `fk_columns` in `pg_tview_meta` if needed
3. **Customize backing views**: Create custom `v_*` views with complex logic
4. **Set up alerting**: Monitor for stale TVIEWs or refresh lag

See the [pg_tviews documentation](https://github.com/yourusername/pg_tviews) for more.
