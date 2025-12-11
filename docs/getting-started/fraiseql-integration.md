# FraiseQL Integration Guide

Learn how pg_tviews fits into the FraiseQL ecosystem and powers GraphQL Cascade with automatic incremental refresh.

## FraiseQL CQRS Overview

FraiseQL implements Command Query Responsibility Segregation (CQRS) using three layers:

```
tb_* tables  →  v_* views  →  tv_* tables  →  GraphQL Cascade
(normalized)    (declarative)  (materialized)    (real-time)
```

- **tb_* tables**: Normalized write models (commands)
- **v_* views**: Declarative read model definitions
- **tv_* tables**: Incrementally refreshed materialized views
- **GraphQL Cascade**: Always-fresh nested data for queries

pg_tviews automates the `v_*` → `tv_*` transformation with surgical updates.

## Trinity Identifier Pattern

pg_tviews follows FraiseQL's trinity identifier pattern for optimal GraphQL performance:

### Core Identifiers

- **`id` (UUID)**: Public GraphQL identifier
- **`pk_entity` (integer)**: Primary key for efficient joins and lineage
- **`fk_*` (integer)`**: Foreign keys for cascade propagation

### Optional Identifiers

- **`identifier` (text)**: SEO-friendly unique slugs
- **`{parent}_id` (UUID)`**: Parent UUID FKs for filtering

### Example Schema

```sql
-- User entity
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,    -- lineage root
    id UUID NOT NULL DEFAULT gen_random_uuid(),  -- GraphQL ID
    identifier TEXT UNIQUE,           -- SEO slug (optional)
    name TEXT NOT NULL,
    email TEXT UNIQUE
);

-- Post entity
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,    -- lineage root
    id UUID NOT NULL DEFAULT gen_random_uuid(),  -- GraphQL ID
    identifier TEXT UNIQUE,           -- SEO slug (optional)
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT REFERENCES tb_user(pk_user),  -- cascade FK
    user_id UUID REFERENCES tb_user(id)         -- filtering FK (optional)
);
```

## TVIEW Creation Patterns

### Basic TVIEW

```sql
CREATE TVIEW tv_posts AS
SELECT
    p.pk_post as pk_post,  -- Required: lineage root
    p.id,                  -- GraphQL ID
    p.fk_user,             -- Cascade propagation
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'content', p.content
    ) as data              -- Required: JSONB read model
FROM tb_post p;
```

### Full Trinity TVIEW

```sql
CREATE TVIEW tv_posts AS
SELECT
    p.pk_post as pk_post,  -- lineage root
    p.id,                  -- GraphQL ID
    p.identifier,          -- SEO slug
    p.fk_user,             -- cascade FK
    u.id as user_id,       -- filtering FK
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

## GraphQL Cascade Integration

### Automatic Updates

pg_tviews automatically refreshes TVIEWs during transactions:

```sql
-- FraiseQL mutation creates/updates tb_* tables
INSERT INTO tb_post (id, title, content, fk_user)
VALUES ('uuid-here', 'New Post', 'Content', 1);

-- pg_tviews automatically updates tv_posts within the same transaction
-- GraphQL Cascade immediately sees fresh data
COMMIT;
```

### Query Patterns

```sql
-- UUID-based single post query
SELECT data FROM tv_posts WHERE id = 'uuid-here';

-- SEO-friendly slug query
SELECT data FROM tv_posts WHERE data->>'identifier' = 'my-post-slug';

-- Author filtering using UUID FK
SELECT data FROM tv_posts WHERE user_id = 'author-uuid';
```

## Cascade Propagation

### Multi-Level Dependencies

pg_tviews automatically handles cascading updates:

```sql
-- When a user changes their name:
UPDATE tb_user SET name = 'New Name' WHERE pk_user = 1;

-- pg_tviews cascades to update all their posts:
-- tv_posts: Updates author.name in all related posts
-- Automatic dependency resolution ensures correct order
```

### Dependency Graph

```
tb_user ──┬─cascade──▶ tv_user
          │
          └─cascade──▶ tv_posts (via fk_user)
                      │
                      └─cascade──▶ tv_comments (via fk_post)
```

## Performance Optimization

### Statement-Level Triggers

Enable for 100-500× better bulk performance:

```sql
SELECT pg_tviews_install_stmt_triggers();
```

### Bulk Operations

pg_tviews optimizes multiple updates:

```sql
-- Single transaction with multiple updates
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES ('Post 1', 1);
INSERT INTO tb_post (title, fk_user) VALUES ('Post 2', 1);
INSERT INTO tb_post (title, fk_user) VALUES ('Post 3', 1);
COMMIT;

-- pg_tviews: 2 queries total (1 SELECT, 1 UPDATE) instead of 6
```

### JSONB Optimization

Use `jsonb_ivm` extension for 2× performance boost:

```sql
-- Surgical JSONB updates instead of full replacement
-- Especially beneficial for large nested objects
```

## Migration from Manual Refresh

### Before: Manual Maintenance

```sql
-- Manual refresh (error-prone, slow)
REFRESH MATERIALIZED VIEW mv_posts;

-- Or custom triggers (complex, buggy)
CREATE OR REPLACE FUNCTION refresh_posts()...
```

### After: Automatic with pg_tviews

```sql
-- One-time setup
CREATE TVIEW tv_posts AS SELECT...;

-- Automatic forever
-- Just use your database normally!
```

## Best Practices

### Schema Design

1. **Always use pk_ prefix** for integer primary keys
2. **Always use fk_ prefix** for integer foreign keys
3. **Include id (UUID) columns** for GraphQL exposure
4. **Add identifier columns** for SEO-friendly URLs
5. **Include parent UUID FKs** for efficient filtering

### TVIEW Design

1. **Include all cascade FKs** in SELECT list
2. **Use descriptive JSONB structure** matching GraphQL schema
3. **Include relevant parent UUIDs** for filtering
4. **Keep TVIEWs focused** on specific use cases

### Performance

1. **Enable statement triggers** for bulk operations
2. **Consider jsonb_ivm** for large JSONB objects
3. **Monitor cascade depth** to avoid performance issues
4. **Use appropriate indexing** on TVIEW tables

## Advanced Patterns

### Computed Fields

```sql
CREATE TVIEW tv_posts AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'wordCount', array_length(string_to_array(p.content, ' '), 1),
        'author', jsonb_build_object('id', u.id, 'name', u.name)
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

### Array Relationships

```sql
CREATE TVIEW tv_posts AS
SELECT
    p.pk_post,
    p.id,
    p.fk_user,
    jsonb_build_object(
        'id', p.id,
        'title', p.title,
        'tags', (
            SELECT jsonb_agg(jsonb_build_object('id', t.id, 'name', t.name))
            FROM tb_tag t
            JOIN tb_post_tag pt ON t.pk_tag = pt.fk_tag
            WHERE pt.fk_post = p.pk_post
        )
    ) as data
FROM tb_post p;
```

## Monitoring Integration

### Health Checks

```sql
SELECT * FROM pg_tviews_health_check();
```

### Performance Metrics

```sql
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '24 hours';
```

### Queue Monitoring

```sql
SELECT * FROM pg_tviews_queue_realtime;
```

## Troubleshooting

### Common Issues

**TVIEW not updating:**
```sql
-- Check triggers are installed
SELECT * FROM pg_trigger WHERE tgname LIKE 'tview%';

-- Check for errors
SELECT * FROM pg_tviews_health_check();
```

**Performance degradation:**
```sql
-- Check cascade depth
SELECT * FROM pg_tviews_cache_stats;

-- Monitor queue size
SELECT count(*) FROM pg_tviews_queue_realtime;
```

**JSONB too large:**
```sql
-- Consider breaking into multiple TVIEWs
-- Or use jsonb_ivm for surgical updates
```

## Next Steps

- **[Developer Guide](../user-guides/developers.md)** - Application integration patterns
- **[Architect Guide](../user-guides/architects.md)** - CQRS design decisions
- **[API Reference](../reference/api.md)** - Complete function reference

## Related Resources

- **FraiseQL Framework**: [github.com/fraiseql/fraiseql](https://github.com/fraiseql/fraiseql)
- **GraphQL Cascade**: Learn about FraiseQL's real-time query capabilities
- **CQRS Patterns**: Best practices for command-query separation