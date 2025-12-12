# Architect Guide

Design patterns and architectural decisions for building CQRS systems with pg_tviews and FraiseQL.

**Version**: 0.1.0-beta.1 • **Last Updated**: December 11, 2025

## Overview

This guide helps system architects design CQRS applications using pg_tviews as the read model infrastructure. You'll learn about architectural patterns, performance characteristics, and design trade-offs for building scalable GraphQL applications.

## CQRS with pg_tviews

### Architecture Overview

pg_tviews enables efficient CQRS implementation by providing automatic read model maintenance:

```
Command Side (Write)          Query Side (Read)
┌─────────────────┐          ┌─────────────────┐
│   FraiseQL      │          │   GraphQL       │
│   Mutations     │          │   Queries       │
│                 │          │   (Cascade)     │
└─────────┬───────┘          └─────────┬───────┘
          │                            │
          ▼                            ▼
┌─────────────────┐          ┌─────────────────┐
│   tb_* tables   │ ───────► │   tv_* tables   │
│ (normalized)    │   auto   │ (denormalized)  │
│ (write models)  │ refresh  │ (read models)   │
└─────────────────┘          └─────────────────┘
          ▲                            ▲
          │                            │
          └────────────── pg_tviews ───┘
                    (incremental refresh)
```

### Benefits for CQRS

1. **Automatic Consistency**: Read models always match write models
2. **Performance**: 5,000-12,000× faster than traditional materialized views
3. **Scalability**: Linear scaling with data size vs O(n) for full refresh
4. **Developer Experience**: No manual refresh logic or cache invalidation

## Design Patterns

### Trinity Identifier Pattern

Design entities following FraiseQL's trinity pattern for optimal performance:

```sql
-- Entity: Post
CREATE TABLE tb_post (
    pk_post INT GENERATED ALWAYS AS IDENTITY,    -- 1. Primary Key (integer)
    id UUID NOT NULL DEFAULT gen_random_uuid(),  -- 2. Public ID (UUID)
    identifier TEXT UNIQUE,           -- 3. SEO slug (optional)
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT REFERENCES tb_user(pk_user),  -- Cascade FK
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Read Model: Post
CREATE TABLE tv_post AS
SELECT
    p.pk_post as pk_post,  -- Required: lineage root
    p.id,                  -- GraphQL ID
    p.identifier,          -- SEO slug
    p.fk_user,             -- Cascade propagation
    u.id as user_id,       -- Filtering FK
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'createdAt', p.created_at,
        'author', jsonb_build_object(
            'id', u.id,
            'name', u.name
        )
    ) as data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

**Design Principles**:
- **pk_**: Integer primary keys for efficient joins and lineage
- **id**: UUID public identifiers for GraphQL and APIs
- **identifier**: Optional slugs for SEO-friendly URLs
- **fk_**: Integer foreign keys for cascade propagation
- **{parent}_id**: UUID FKs for efficient filtering

### Aggregate Design

Design aggregates that match your GraphQL schema boundaries:

```sql
-- Product Aggregate (E-commerce)
CREATE TABLE tv_product AS
SELECT
    p.pk_product,
    p.id,
    p.fk_category,
    p.fk_supplier,
    c.id as category_id,
    s.id as supplier_id,
    jsonb_build_object(
        'id', p.id,
        'name', p.name,
        'price', jsonb_build_object('current', p.price_current),
        'category', jsonb_build_object('id', c.id, 'name', c.name),
        'supplier', jsonb_build_object('id', s.id, 'name', s.name),
        'inventory', jsonb_build_object('quantity', i.quantity_available),
        'reviews', COALESCE(jsonb_agg(
            jsonb_build_object('id', r.id, 'rating', r.rating)
        ) FILTER (WHERE r.id IS NOT NULL), '[]'::jsonb),
        'avgRating', COALESCE(AVG(r.rating), 0)
    ) as data
FROM tb_product p
LEFT JOIN tb_category c ON p.fk_category = c.pk_category
LEFT JOIN tb_supplier s ON p.fk_supplier = s.pk_supplier
LEFT JOIN tb_inventory i ON p.pk_product = i.fk_product
LEFT JOIN tb_review r ON p.pk_product = r.fk_product
GROUP BY p.pk_product, p.id, p.fk_category, p.fk_supplier,
         c.id, c.name, s.id, s.name, i.quantity_available;
```

**Aggregate Guidelines**:
- Include all data needed for GraphQL resolvers
- Pre-compute relationships and aggregations
- Keep aggregates focused on specific use cases
- Consider cascade impact on aggregate size

### Cascade Architecture

Design cascade relationships for optimal performance:

```sql
-- Shallow cascades (recommended)
tb_user → tv_user (1:1)
tb_post → tv_post (many:1 with user)
tb_comment → tv_comment (many:1 with post+user)

-- Deep cascades (use carefully)
tb_category → tv_category
    ↓ cascade
tb_product → tv_product (category updates affect all products)
    ↓ cascade
tb_review → tv_review (product updates affect all reviews)
```

**Cascade Design Principles**:
- **Depth Limit**: Keep cascade chains < 3 levels
- **Fan-out Control**: Limit high-fan-out cascades (1:N where N is large)
- **Update Frequency**: Consider how often entities change
- **Read Patterns**: Design cascades to match query patterns

## Performance Architecture

### Read Model Optimization

Optimize for GraphQL query patterns:

```sql
-- Query Pattern: Get post with author
SELECT data FROM tv_post WHERE id = ?;
-- Optimized: Direct UUID lookup, pre-joined author data

-- Query Pattern: User's posts
SELECT data FROM tv_post WHERE user_id = ? ORDER BY data->>'createdAt' DESC;
-- Optimized: UUID FK index, JSONB ordering

-- Query Pattern: Posts by category
SELECT data FROM tv_post WHERE data->'category'->>'id' = ?;
-- Consider: Add category_id UUID FK for better performance
```

### Indexing Strategy

Design indexes for your GraphQL query patterns:

```sql
-- Primary lookup patterns
CREATE UNIQUE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_user_id ON tv_post(user_id);

-- JSONB field queries
CREATE INDEX idx_tv_post_created_at ON tv_post USING gin((data->'createdAt'));
CREATE INDEX idx_tv_post_title ON tv_post USING gin((data->'title'));

-- Composite patterns
CREATE INDEX idx_tv_post_user_created ON tv_post(user_id, (data->>'createdAt'));
CREATE INDEX idx_tv_post_category_created ON tv_post((data->'category'->>'id'), (data->>'createdAt'));

-- Full-text search
CREATE INDEX idx_tv_post_content_fts ON tv_post USING gin(to_tsvector('english', data->>'content'));
```

### Partitioning Strategy

Partition large TVIEWs for better performance:

```sql
-- Time-based partitioning
CREATE TABLE tv_post_y2024 PARTITION OF tv_post
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

CREATE TABLE tv_post_y2025 PARTITION OF tv_post
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

-- Include partitioning key in TVIEW
CREATE TABLE tv_post AS
SELECT
    pk_post,
    id,
    EXTRACT(YEAR FROM created_at) as partition_key,  -- For partitioning
    jsonb_build_object(...) as data
FROM tb_post;
```

## Scalability Patterns

### Read Scaling

Scale read workloads with read replicas:

```
Primary (Write)
├── tb_* tables (writes)
└── TVIEW triggers (refresh)

Read Replicas
├── tv_* tables (reads)
└── Automatic replication
```

**Read Scaling Benefits**:
- Horizontal scaling for read workloads
- TVIEWs automatically stay in sync
- No application changes required
- Standard PostgreSQL streaming replication

### Write Scaling

Handle high write throughput:

```sql
-- Statement-level triggers for bulk operations
SELECT pg_tviews_install_stmt_triggers();

-- Batch writes in transactions
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES (...);
INSERT INTO tb_post (title, fk_user) VALUES (...);
-- ... more inserts
COMMIT;  -- Single cascade operation
```

### Data Partitioning

Partition for large datasets:

```sql
-- Hash partitioning by user
CREATE TABLE tv_post_0 PARTITION OF tv_post
    FOR VALUES WITH (MODULUS 4, REMAINDER 0);
CREATE TABLE tv_post_1 PARTITION OF tv_post
    FOR VALUES WITH (MODULUS 4, REMAINDER 1);
-- etc.

-- Update TVIEW to include partitioning key
CREATE TABLE tv_post AS
SELECT
    pk_post,
    id,
    abs(hashtext(user_id::text)) % 4 as partition_key,
    jsonb_build_object(...) as data
FROM tb_post;
```

## Consistency Models

### Eventual Consistency

pg_tviews provides transactional consistency within the write transaction:

```sql
-- Transactional consistency
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES ('New Post', 1);
-- TVIEW automatically updated here
SELECT data FROM tv_post WHERE id = 'new-post-uuid';  -- Fresh data
COMMIT;
```

### Read-after-Write Consistency

For immediate consistency requirements:

```sql
-- Same transaction read
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES ('New Post', 1);
-- TVIEW updated, immediate read returns fresh data
COMMIT;
```

### Cross-Transaction Consistency

For eventual consistency scenarios:

```sql
-- Application handles eventual consistency
app.post('/posts', async (req, res) => {
  const postId = await createPost(req.body);  // Transaction 1
  // TVIEW updated in transaction 1

  // Immediate read gets fresh data (same connection)
  const post = await getPost(postId);  // Fresh data
  res.json(post);
});
```

## Error Handling Architecture

### Transaction Failure Handling

Design for transaction rollback scenarios:

```sql
-- Automatic rollback consistency
BEGIN;
INSERT INTO tb_post (title, fk_user) VALUES ('New Post', 1);
-- TVIEW updated
ROLLBACK;  -- Both tb_post insert and TVIEW update rolled back
```

### Cascade Failure Handling

Handle cascade failures gracefully:

```sql
-- Monitor cascade performance
SELECT pg_tviews_queue_stats();

-- Detect slow cascades
SELECT CASE
    WHEN (pg_tviews_queue_stats()->>'total_timing_ms')::float > 1000
    THEN 'Slow cascade detected'
    ELSE 'OK'
END;
```

### Circuit Breaker Pattern

Implement circuit breakers for cascade protection:

```sql
-- Check cascade depth
CREATE OR REPLACE FUNCTION check_cascade_depth()
RETURNS trigger AS $$
DECLARE
    cascade_count int;
BEGIN
    -- Count pending cascades
    SELECT (pg_tviews_queue_stats()->>'queue_size')::int INTO cascade_count;

    IF cascade_count > 1000 THEN
        RAISE EXCEPTION 'Cascade queue too large, rejecting update';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Install circuit breaker
CREATE TRIGGER cascade_circuit_breaker
    BEFORE INSERT ON tb_post
    FOR EACH ROW EXECUTE FUNCTION check_cascade_depth();
```

## Migration Strategies

### From Monolithic Applications

**Before**: Single database with complex queries
```sql
-- Complex joins on every query
SELECT p.*, u.name, c.name, COUNT(r.id) as review_count
FROM posts p
JOIN users u ON p.user_id = u.id
JOIN categories c ON p.category_id = c.id
LEFT JOIN reviews r ON p.id = r.post_id
GROUP BY p.id, u.name, c.name;
```

**After**: Pre-computed read models
```sql
-- Simple lookup
SELECT data FROM tv_post WHERE id = ?;
-- Data includes all relationships and aggregations
```

### From Manual Cache Management

**Before**: Application-level caching
```javascript
// Manual cache invalidation
app.post('/posts', async (req, res) => {
  await db.insert('posts', req.body);
  await cache.invalidate('posts:*');  // Manual!
  await cache.invalidate('user:123:posts');  // Manual!
  res.json(await getPostWithCache(id));
});
```

**After**: Automatic cache consistency
```javascript
// No cache management needed
app.post('/posts', async (req, res) => {
  const postId = await fraiseql.create('Post', req.body);
  // TVIEW automatically updated
  res.json(await db.query('SELECT data FROM tv_post WHERE id = $1', [postId]));
});
```

### From Event Sourcing

**Before**: Event sourcing with manual projection
```javascript
// Manual event projection
eventStore.on('PostCreated', (event) => {
  db.insert('posts', event.data);
  cache.update('post:' + event.data.id, event.data);
});

eventStore.on('PostUpdated', (event) => {
  db.update('posts', event.data);
  cache.invalidate('post:' + event.data.id);
});
```

**After**: Automatic projection with pg_tviews
```javascript
// Events update tb_* tables
// pg_tviews automatically maintains tv_* projections
// No manual projection code needed
```

## Performance Trade-offs

### Automatic vs Manual Refresh

| Aspect | Automatic (pg_tviews) | Manual Refresh |
|--------|----------------------|----------------|
| **Consistency** | Transactional | Eventual |
| **Performance** | 5,000-12,000× faster | 95% of automatic |
| **Developer Effort** | Zero | High |
| **Operational Complexity** | Low | High |
| **Flexibility** | Fixed patterns | Full control |

### Statement vs Row Triggers

| Aspect | Statement Triggers | Row Triggers |
|--------|-------------------|--------------|
| **Bulk Performance** | 100-500× faster | Baseline |
| **Single Operations** | Same performance | Same performance |
| **Memory Usage** | Higher | Lower |
| **Compatibility** | PostgreSQL 13+ | All versions |
| **Use Case** | High-throughput | General purpose |

### JSONB vs Normalized Storage

| Aspect | JSONB (TVIEW) | Normalized |
|--------|---------------|------------|
| **Query Flexibility** | High | Low |
| **Storage Efficiency** | Lower | Higher |
| **Index Options** | GIN, GiST | B-tree, etc. |
| **Update Performance** | Surgical | Full row |
| **Schema Evolution** | Easy | Complex |

## Monitoring and Observability

### Key Metrics to Monitor

```sql
-- Performance metrics
SELECT pg_tviews_queue_stats();

-- Cache efficiency
SELECT
    (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float as graph_hit_rate,
    (pg_tviews_queue_stats()->>'table_cache_hit_rate')::float as table_hit_rate;

-- Cascade depth monitoring
SELECT pg_tviews_debug_queue();

-- TVIEW health
SELECT * FROM pg_tviews_health_check();
```

### Alerting Strategy

```sql
-- Performance alerts
CREATE OR REPLACE FUNCTION tview_performance_alerts()
RETURNS TABLE(alert_level text, message text) AS $$
BEGIN
    -- Slow refresh alert
    IF (pg_tviews_queue_stats()->>'total_timing_ms')::float > 5000 THEN
        RETURN QUERY SELECT 'WARNING'::text, 'TVIEW refresh > 5 seconds'::text;
    END IF;

    -- Large queue alert
    IF (pg_tviews_queue_stats()->>'queue_size')::int > 1000 THEN
        RETURN QUERY SELECT 'CRITICAL'::text, 'TVIEW queue > 1000'::text;
    END IF;

    -- Low cache hit rate
    IF (pg_tviews_queue_stats()->>'graph_cache_hit_rate')::float < 0.8 THEN
        RETURN QUERY SELECT 'WARNING'::text, 'Graph cache hit rate < 80%'::text;
    END IF;
END;
$$ LANGUAGE plpgsql;
```

## Best Practices

### Architecture Principles

1. **CQRS First**: Design with command-query separation from the start
2. **Read Model Optimization**: Optimize TVIEWs for specific query patterns
3. **Cascade Planning**: Design cascade relationships carefully
4. **Monitoring First**: Implement observability from day one

### Design Guidelines

1. **Trinity Pattern**: Always use id/pk_/fk_ consistently
2. **Aggregate Boundaries**: Match TVIEWs to GraphQL schema boundaries
3. **Cascade Depth**: Keep dependency chains shallow (< 3 levels)
4. **Index Strategically**: Index for actual query patterns, not assumptions

### Performance Guidelines

1. **Statement Triggers**: Use for high-throughput write scenarios
2. **Partitioning**: Consider for tables > 100M rows
3. **Read Replicas**: Scale reads with standard PostgreSQL replication
4. **Monitoring**: Track performance metrics continuously

### Operational Guidelines

1. **Health Checks**: Implement comprehensive monitoring
2. **Backup Strategy**: Include TVIEWs in regular backups
3. **Disaster Recovery**: Test recovery procedures regularly
4. **Version Compatibility**: Plan upgrades carefully

## Case Studies

### E-commerce Platform

**Challenge**: Product catalog with complex relationships and frequent updates
**Solution**: pg_tviews with category/supplier/product/review cascades
**Result**: 8,000× performance improvement, real-time inventory updates

### Social Media Platform

**Challenge**: User timelines with nested relationships and high read load
**Solution**: TVIEWs with user/post/comment cascades, read replica scaling
**Result**: Sub-millisecond queries, 95% cache hit rates

### Analytics Dashboard

**Challenge**: Pre-aggregated reporting data that must stay fresh
**Solution**: TVIEWs with automatic refresh on source data changes
**Result**: Always-consistent reports, zero manual refresh burden

## See Also

- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md) - Framework patterns
- [Performance Benchmarks](../benchmarks/overview.md) - Detailed performance data
- [Performance Tuning](../operations/performance-tuning.md) - Optimization strategies
- [Troubleshooting Guide](../operations/troubleshooting.md) - Common issues
