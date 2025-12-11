# Developer Guide

Complete guide for integrating pg_tviews into FraiseQL applications and GraphQL APIs.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

This guide helps backend developers integrate pg_tviews into FraiseQL applications. You'll learn how to create TVIEWs, handle GraphQL Cascade queries, and optimize performance for API workloads.

## FraiseQL Integration Patterns

### CQRS Architecture with pg_tviews

pg_tviews powers FraiseQL's CQRS pattern by maintaining automatically refreshed read models:

```
GraphQL Mutation (Command)    GraphQL Query (Read)
        ↓                              ↓
    tb_* tables  ── pg_tviews ──→  tv_* tables
   (write models)   (automatic)    (read models)
        ↑                              ↑
   FraiseQL Commands            GraphQL Cascade
```

### Basic Integration Workflow

1. **Design Schema**: Define tb_* tables following trinity pattern
2. **Create TVIEWs**: Define tv_* views for GraphQL queries
3. **GraphQL Integration**: Use TVIEWs in GraphQL resolvers
4. **Monitor Performance**: Track refresh metrics and optimize

## Schema Design

### Trinity Pattern Implementation

Follow FraiseQL's trinity identifier pattern for optimal GraphQL performance:

```sql
-- User entity
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,    -- Lineage root
    id UUID NOT NULL DEFAULT gen_random_uuid(),  -- GraphQL ID
    identifier TEXT UNIQUE,           -- SEO slug (optional)
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Post entity
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,    -- Lineage root
    id UUID NOT NULL DEFAULT gen_random_uuid(),  -- GraphQL ID
    identifier TEXT UNIQUE,           -- SEO slug (optional)
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),  -- Cascade FK
    user_id UUID,                     -- Filtering FK (computed)
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Maintain user_id for filtering
CREATE OR REPLACE FUNCTION maintain_user_id()
RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        SELECT id INTO NEW.user_id
        FROM tb_user WHERE pk_user = NEW.fk_user;
        RETURN NEW;
    END IF;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trig_maintain_user_id
    BEFORE INSERT OR UPDATE ON tb_post
    FOR EACH ROW EXECUTE FUNCTION maintain_user_id();
```

### TVIEW Design for GraphQL

Design TVIEWs to match your GraphQL schema exactly:

```sql
CREATE TVIEW tv_post AS
SELECT
    p.pk_post as pk_post,  -- Required: lineage root
    p.id,                  -- GraphQL ID
    p.identifier,          -- SEO-friendly slug
    p.fk_user,             -- Cascade FK
    u.id as user_id,       -- Filtering FK
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'createdAt', p.created_at,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name,
            'email', u.email
        ),
        'comments', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', c.id,
                    'text', c.text,
                    'author', jsonb_build_object('id', cu.id, 'name', cu.name)
                ) ORDER BY c.created_at
            )
            FROM tb_comment c
            JOIN tb_user cu ON c.fk_user = cu.pk_user
            WHERE c.fk_post = p.pk_post
        ), '[]'::jsonb)
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

## GraphQL Integration

### GraphQL Cascade Queries

Use TVIEWs directly in GraphQL resolvers for instant fresh data:

```javascript
// GraphQL Resolvers (Node.js/TypeScript example)
const resolvers = {
  Query: {
    post: async (_, { id }) => {
      const result = await db.query(`
        SELECT data FROM tv_post WHERE id = $1
      `, [id]);
      return result.rows[0]?.data;
    },

    posts: async (_, { authorId, limit = 10 }) => {
      const result = await db.query(`
        SELECT data FROM tv_post
        WHERE user_id = $1
        ORDER BY data->>'createdAt' DESC
        LIMIT $2
      `, [authorId, limit]);
      return result.rows.map(row => row.data);
    }
  },

  Mutation: {
    createPost: async (_, { input }) => {
      // FraiseQL handles the tb_* write
      const postId = await fraiseql.create('Post', input);

      // TVIEW automatically updated - return fresh data immediately
      const result = await db.query(`
        SELECT data FROM tv_post WHERE id = $1
      `, [postId]);
      return result.rows[0]?.data;
    },

    updatePost: async (_, { id, input }) => {
      // FraiseQL handles the tb_* update
      await fraiseql.update('Post', id, input);

      // TVIEW automatically updated - return fresh data immediately
      const result = await db.query(`
        SELECT data FROM tv_post WHERE id = $1
      `, [id]);
      return result.rows[0]?.data;
    }
  }
};
```

### UUID-Based Filtering

Leverage UUID FKs for efficient GraphQL filtering:

```sql
-- Efficient UUID filtering (no JOINs needed)
SELECT data FROM tv_post WHERE user_id = 'uuid-here';
SELECT data FROM tv_post WHERE id = 'post-uuid';

-- SEO-friendly slug queries
SELECT data FROM tv_post WHERE data->>'identifier' = 'my-blog-post';
```

### Connection Pattern

Implement GraphQL connections with efficient pagination:

```sql
-- Forward pagination
SELECT data FROM tv_post
WHERE user_id = $1
  AND data->>'createdAt' > $2  -- cursor
ORDER BY data->>'createdAt' ASC
LIMIT $3;

-- Backward pagination
SELECT data FROM tv_post
WHERE user_id = $1
  AND data->>'createdAt' < $2  -- cursor
ORDER BY data->>'createdAt' DESC
LIMIT $3;
```

## Performance Optimization

### Statement-Level Triggers

Enable for 100-500× better bulk operation performance:

```sql
-- Enable for bulk operations
SELECT pg_tviews_install_stmt_triggers();

-- Your application code remains unchanged
-- Bulk inserts/updates automatically optimized
```

### JSONB Indexing

Create indexes for common GraphQL query patterns:

```sql
-- Index for UUID lookups
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);

-- Index for JSONB field queries
CREATE INDEX idx_tv_post_title ON tv_post USING gin((data->'title'));
CREATE INDEX idx_tv_post_created_at ON tv_post USING gin((data->'createdAt'));

-- Index for nested author queries
CREATE INDEX idx_tv_post_author_name ON tv_post USING gin((data->'author'->'name'));

-- Composite indexes for common filters
CREATE INDEX idx_tv_post_user_created ON tv_post(user_id, (data->>'createdAt'));
```

### Query Optimization

Structure queries for optimal performance:

```sql
-- ✅ Efficient: Direct UUID lookup
SELECT data FROM tv_post WHERE id = $1;

-- ✅ Efficient: Indexed filtering
SELECT data FROM tv_post WHERE user_id = $1 AND data->>'createdAt' > $2;

-- ❌ Inefficient: Non-indexed JSONB queries
SELECT data FROM tv_post WHERE data->'author'->>'name' ILIKE '%john%';

-- ✅ Better: Pre-compute searchable fields or use separate indexes
```

## Error Handling

### Transactional Consistency

pg_tviews maintains ACID compliance with your application transactions:

```javascript
// Successful transaction
await db.transaction(async (tx) => {
  // Update tb_* tables via FraiseQL
  await tx.query('UPDATE tb_post SET title = $1 WHERE id = $2', [title, id]);

  // TVIEW automatically updated within same transaction
  // GraphQL query sees fresh data immediately
});

// Failed transaction
try {
  await db.transaction(async (tx) => {
    await tx.query('UPDATE tb_post SET title = $1 WHERE id = $2', [title, id]);
    throw new Error('Something went wrong');
  });
} catch (error) {
  // Both tb_* changes and TVIEW updates rolled back
  // Data remains consistent
}
```

### Monitoring Refresh Performance

Track TVIEW refresh performance in your application:

```javascript
// Monitor refresh queue in development
const queueStats = await db.query('SELECT pg_tviews_queue_stats()');
console.log('Refresh queue:', queueStats.rows[0].pg_tviews_queue_stats);

// Check for refresh errors
const health = await db.query('SELECT * FROM pg_tviews_health_check()');
if (!health.rows[0].healthy) {
  console.error('TVIEW refresh issues detected');
}
```

## Migration Strategies

### From Manual Refresh

**Before**: Manual materialized view maintenance
```sql
-- Manual refresh (slow, error-prone)
REFRESH MATERIALIZED VIEW mv_posts;

-- Application code handles consistency
app.post('/posts', async (req, res) => {
  await db.query('INSERT INTO posts ...');
  await db.query('REFRESH MATERIALIZED VIEW mv_posts'); // Manual!
  res.json(await getPostData());
});
```

**After**: Automatic with pg_tviews
```sql
-- One-time setup
CREATE TABLE tv_post AS SELECT ...;

-- Application code unchanged, automatic refresh
app.post('/posts', async (req, res) => {
  await db.query('INSERT INTO tb_post ...'); // Via FraiseQL
  // TVIEW automatically updated!
  res.json(await db.query('SELECT data FROM tv_post WHERE id = $1', [id]));
});
```

### From Application-Level Caching

**Before**: Application-managed cache
```javascript
// Complex caching logic
const post = await db.query('SELECT * FROM posts WHERE id = $1');
const author = await db.query('SELECT * FROM users WHERE id = $1');
const comments = await db.query('SELECT * FROM comments WHERE post_id = $1');

// Manual JSON composition
const result = {
  id: post.id,
  title: post.title,
  author: { id: author.id, name: author.name },
  comments: comments.map(c => ({ id: c.id, text: c.text }))
};
```

**After**: Pre-computed with pg_tviews
```javascript
// Single query, always fresh
const result = await db.query('SELECT data FROM tv_post WHERE id = $1');
// Data automatically includes nested relationships
```

## Testing Strategies

### Unit Testing TVIEWs

Test TVIEW definitions and refresh behavior:

```sql
-- Test TVIEW creation
CREATE TVIEW tv_test AS
SELECT p.pk_post, p.id, jsonb_build_object('id', p.id, 'title', p.title) as data
FROM tb_post p WHERE p.pk_post < 100; -- Limited test data

-- Verify structure
SELECT pk_post, id, jsonb_object_keys(data) as fields FROM tv_test;

-- Test refresh behavior
INSERT INTO tb_post (id, title) VALUES ('test-uuid', 'Test Post');
SELECT COUNT(*) FROM tv_test WHERE id = 'test-uuid'; -- Should be 1

-- Cleanup
DROP TVIEW tv_test;
```

### Integration Testing

Test end-to-end GraphQL workflows:

```javascript
// Test GraphQL mutation + immediate query
describe('Post Creation', () => {
  test('creates post and returns fresh data', async () => {
    const mutation = `
      mutation CreatePost($input: CreatePostInput!) {
        createPost(input: $input) {
          id
          title
          author { name }
        }
      }
    `;

    const result = await graphql(mutation, {
      input: { title: 'Test Post', authorId: 'user-uuid' }
    });

    expect(result.data.createPost).toBeDefined();

    // Immediate query should return fresh data
    const query = `
      query GetPost($id: ID!) {
        post(id: $id) {
          id
          title
          author { name }
        }
      }
    `;

    const freshResult = await graphql(query, {
      id: result.data.createPost.id
    });

    expect(freshResult.data.post.title).toBe('Test Post');
  });
});
```

### Performance Testing

Test TVIEW performance under load:

```javascript
// Load testing TVIEW refreshes
async function loadTest() {
  const promises = [];
  for (let i = 0; i < 100; i++) {
    promises.push(
      db.query('INSERT INTO tb_post (id, title, fk_user) VALUES ($1, $2, $3)', [
        `post-${i}`, `Post ${i}`, 1
      ])
    );
  }

  const start = Date.now();
  await Promise.all(promises);
  const duration = Date.now() - start;

  console.log(`100 inserts took ${duration}ms`);
  console.log(`Average: ${duration/100}ms per insert`);

  // Verify all TVIEWs updated
  const count = await db.query('SELECT COUNT(*) FROM tv_post');
  expect(count.rows[0].count).toBeGreaterThanOrEqual(100);
}
```

## Troubleshooting

### TVIEW Not Updating

**Check triggers are installed:**
```sql
SELECT tgname FROM pg_trigger WHERE tgname LIKE 'tview%';
-- Should see triggers for your TVIEWs
```

**Check for errors:**
```sql
SELECT * FROM pg_tviews_health_check();
-- Look for any error messages
```

**Verify TVIEW definition:**
```sql
SELECT * FROM pg_tview_meta WHERE entity = 'post';
-- Check if TVIEW is properly registered
```

### Slow Queries

**Check indexes:**
```sql
SELECT * FROM pg_indexes WHERE tablename = 'tv_post';
-- Ensure proper indexes on id, user_id, and JSONB fields
```

**Analyze query performance:**
```sql
EXPLAIN ANALYZE SELECT data FROM tv_post WHERE id = 'uuid-here';
-- Look for sequential scans or slow operations
```

**Check cascade depth:**
```sql
SELECT pg_tviews_queue_stats();
-- High cascade depths may indicate performance issues
```

### Memory Issues

**Monitor queue size:**
```sql
SELECT pg_tviews_debug_queue();
-- Large queues may indicate refresh backlog
```

**Check for memory leaks:**
```sql
SELECT * FROM pg_tviews_performance_summary
WHERE hour > now() - interval '1 hour';
-- Look for increasing memory usage patterns
```

## Best Practices

### Schema Design

1. **Follow Trinity Pattern**: Always use id/pk_/fk_ consistently
2. **Include All Relationships**: Pre-compute JOINs in TVIEWs for fast queries
3. **Use Appropriate Data Types**: UUID for IDs, integer for FKs and PKs
4. **Plan Cascade Depth**: Keep dependency chains shallow (<3 levels)

### TVIEW Design

1. **Match GraphQL Schema**: Design TVIEW JSONB to match your GraphQL types exactly
2. **Include Filtering Fields**: Add UUID FKs for common query patterns
3. **Pre-compute Aggregations**: Include counts, averages in JSONB for fast access
4. **Use Efficient JOINs**: Prefer INNER JOINs, avoid complex subqueries

### Application Integration

1. **Trust Automatic Updates**: Don't manually refresh TVIEWs
2. **Use UUIDs for Filtering**: Leverage user_id, category_id etc. for fast queries
3. **Monitor Performance**: Track refresh metrics in production
4. **Test Thoroughly**: Verify TVIEW updates work in your specific schema

### Performance

1. **Enable Statement Triggers**: For bulk operations and high-throughput scenarios
2. **Index Strategically**: Create indexes for your actual query patterns
3. **Monitor Queue Stats**: Watch for cascade performance issues
4. **Profile Regularly**: Use EXPLAIN ANALYZE to optimize slow queries

## See Also

- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md) - Framework patterns
- [API Reference](../reference/api.md) - Complete function reference
- [Performance Tuning](../operations/performance-tuning.md) - Optimization strategies
- [Troubleshooting Guide](../operations/troubleshooting.md) - Common issues and solutions