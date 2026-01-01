# A+ Documentation Quality Plan - Part 3 (Final)

*Continuation of APLUS_DOCUMENTATION_PLAN_PART2.md*

---

## Phase D: Learning & Onboarding (continued)

### D3: Example Applications (4 hours)

**Objective**: Provide complete, runnable example applications.

**Examples to Create**:

1. **Blog System** (Simple)
2. **E-commerce Product Catalog** (Medium)
3. **Social Media Feed** (Advanced)

**Example Structure** (Blog System):

```markdown
# Example: Blog System with pg_tviews

Complete working example of a blog built with pg_tviews.

## Features

- User management
- Blog posts with rich content
- Comments and likes
- Category organization
- Real-time updates

## Repository Structure

```
blog-example/
├── schema/
│   ├── 01_create_tables.sql
│   ├── 02_create_tviews.sql
│   └── 03_sample_data.sql
├── app/
│   ├── server.js (Node.js API)
│   ├── queries.js (Database queries)
│   └── package.json
├── README.md
└── docker-compose.yml
```

## Quick Start

```bash
# 1. Clone repository
git clone https://github.com/your-org/pg_tviews-blog-example.git
cd pg_tviews-blog-example

# 2. Start PostgreSQL with pg_tviews
docker-compose up -d

# 3. Run schema and sample data
psql -h localhost -U postgres -d blog -f schema/01_create_tables.sql
psql -h localhost -U postgres -d blog -f schema/02_create_tviews.sql
psql -h localhost -U postgres -d blog -f schema/03_sample_data.sql

# 4. Start API server
cd app
npm install
npm start

# 5. Test API
curl http://localhost:3000/api/posts
```

## Schema Design

### Tables (Trinity Pattern)

```sql
-- Users
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE NOT NULL,
    bio TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Posts
CREATE TABLE tb_post (
    pk_post BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    fk_category BIGINT REFERENCES tb_category(pk_category),
    published BOOLEAN DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Comments
CREATE TABLE tb_comment (
    pk_comment BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    text TEXT NOT NULL,
    fk_post BIGINT NOT NULL REFERENCES tb_post(pk_post) ON DELETE CASCADE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Likes
CREATE TABLE tb_like (
    pk_like BIGSERIAL PRIMARY KEY,
    fk_post BIGINT NOT NULL REFERENCES tb_post(pk_post) ON DELETE CASCADE,
    fk_user BIGINT NOT NULL REFERENCES tb_user(pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(fk_post, fk_user)
);

-- Categories
CREATE TABLE tb_category (
    pk_category BIGSERIAL PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL UNIQUE,
    description TEXT
);
```

### TVIEWs (Read Models)

```sql
-- User profile with post count
CREATE TABLE tv_user AS
SELECT
    u.pk_user,
    u.id,
    u.identifier,
    jsonb_build_object(
        'id', u.id,
        'identifier', u.identifier,
        'name', u.name,
        'bio', u.bio,
        'postCount', COUNT(DISTINCT p.pk_post),
        'createdAt', u.created_at
    ) AS data
FROM tb_user u
LEFT JOIN tb_post p ON p.fk_user = u.pk_user
GROUP BY u.pk_user, u.id, u.identifier, u.name, u.bio, u.created_at;

-- Posts with author, comments, likes
CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.identifier,
    p.fk_user,
    p.fk_category,
    u.id AS user_id,
    c.id AS category_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'published', p.published,
        'author', jsonb_build_object(
            'id', u.id,
            'identifier', u.identifier,
            'name', u.name
        ),
        'category', jsonb_build_object(
            'id', c.id,
            'identifier', c.identifier,
            'name', c.name
        ),
        'commentCount', (
            SELECT COUNT(*)
            FROM tb_comment
            WHERE fk_post = p.pk_post
        ),
        'likeCount', (
            SELECT COUNT(*)
            FROM tb_like
            WHERE fk_post = p.pk_post
        ),
        'comments', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', cm.id,
                    'text', cm.text,
                    'author', jsonb_build_object(
                        'id', cu.id,
                        'name', cu.name
                    ),
                    'createdAt', cm.created_at
                ) ORDER BY cm.created_at DESC
            )
            FROM tb_comment cm
            JOIN tb_user cu ON cm.fk_user = cu.pk_user
            WHERE cm.fk_post = p.pk_post
        ), '[]'::jsonb),
        'createdAt', p.created_at,
        'updatedAt', p.updated_at
    ) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user
LEFT JOIN tb_category c ON p.fk_category = c.pk_category;

-- Category with post list
CREATE TABLE tv_category AS
SELECT
    c.pk_category,
    c.id,
    c.identifier,
    jsonb_build_object(
        'id', c.id,
        'identifier', c.identifier,
        'name', c.name,
        'description', c.description,
        'postCount', COUNT(p.pk_post),
        'posts', COALESCE(jsonb_agg(
            jsonb_build_object(
                'id', p.id,
                'title', p.title,
                'author', jsonb_build_object('id', u.id, 'name', u.name)
            ) ORDER BY p.created_at DESC
        ) FILTER (WHERE p.pk_post IS NOT NULL), '[]'::jsonb)
    ) AS data
FROM tb_category c
LEFT JOIN tb_post p ON p.fk_category = c.pk_category AND p.published = TRUE
LEFT JOIN tb_user u ON p.fk_user = u.pk_user
GROUP BY c.pk_category, c.id, c.identifier, c.name, c.description;
```

## API Implementation (Node.js)

```javascript
// app/server.js
const express = require('express');
const { Pool } = require('pg');

const pool = new Pool({
    host: 'localhost',
    database: 'blog',
    user: 'postgres',
    password: 'password',
    port: 5432
});

const app = express();
app.use(express.json());

// Get all posts
app.get('/api/posts', async (req, res) => {
    try {
        const result = await pool.query(`
            SELECT data FROM tv_post
            WHERE data->>'published' = 'true'
            ORDER BY data->>'createdAt' DESC
            LIMIT 20
        `);
        res.json(result.rows.map(row => row.data));
    } catch (err) {
        res.status(500).json({ error: err.message });
    }
});

// Get single post
app.get('/api/posts/:id', async (req, res) => {
    try {
        const result = await pool.query(`
            SELECT data FROM tv_post WHERE id = $1
        `, [req.params.id]);

        if (result.rows.length === 0) {
            return res.status(404).json({ error: 'Post not found' });
        }

        res.json(result.rows[0].data);
    } catch (err) {
        res.status(500).json({ error: err.message });
    }
});

// Create post
app.post('/api/posts', async (req, res) => {
    const { title, content, userId, categoryId, slug } = req.body;

    try {
        const result = await pool.query(`
            INSERT INTO tb_post (identifier, title, content, fk_user, fk_category, published)
            VALUES ($1, $2, $3, $4, $5, true)
            RETURNING id
        `, [slug, title, content, userId, categoryId]);

        // Immediately query TVIEW for fresh data
        const postResult = await pool.query(`
            SELECT data FROM tv_post WHERE id = $1
        `, [result.rows[0].id]);

        res.status(201).json(postResult.rows[0].data);
    } catch (err) {
        res.status(400).json({ error: err.message });
    }
});

// Add comment
app.post('/api/posts/:id/comments', async (req, res) => {
    const { text, userId } = req.body;
    const postId = req.params.id;

    try {
        // First get pk_post from id
        const postResult = await pool.query(`
            SELECT pk_post FROM tv_post WHERE id = $1
        `, [postId]);

        if (postResult.rows.length === 0) {
            return res.status(404).json({ error: 'Post not found' });
        }

        const pkPost = postResult.rows[0].pk_post;

        // Insert comment
        await pool.query(`
            INSERT INTO tb_comment (text, fk_post, fk_user)
            VALUES ($1, $2, $3)
        `, [text, pkPost, userId]);

        // Return updated post (TVIEW automatically refreshed)
        const updated = await pool.query(`
            SELECT data FROM tv_post WHERE id = $1
        `, [postId]);

        res.json(updated.rows[0].data);
    } catch (err) {
        res.status(400).json({ error: err.message });
    }
});

// Like post
app.post('/api/posts/:id/like', async (req, res) => {
    const { userId } = req.body;
    const postId = req.params.id;

    try {
        const postResult = await pool.query(`
            SELECT pk_post FROM tv_post WHERE id = $1
        `, [postId]);

        if (postResult.rows.length === 0) {
            return res.status(404).json({ error: 'Post not found' });
        }

        const pkPost = postResult.rows[0].pk_post;

        await pool.query(`
            INSERT INTO tb_like (fk_post, fk_user)
            VALUES ($1, $2)
            ON CONFLICT (fk_post, fk_user) DO NOTHING
        `, [pkPost, userId]);

        const updated = await pool.query(`
            SELECT data FROM tv_post WHERE id = $1
        `, [postId]);

        res.json(updated.rows[0].data);
    } catch (err) {
        res.status(400).json({ error: err.message });
    }
});

app.listen(3000, () => {
    console.log('Blog API running on http://localhost:3000');
});
```

## Docker Setup

```yaml
# docker-compose.yml
version: '3.8'

services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_DB: blog
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: password
    ports:
      - "5432:5432"
    volumes:
      - ./pg_tviews.so:/usr/lib/postgresql/17/lib/pg_tviews.so
      - ./pg_tviews.control:/usr/share/postgresql/17/extension/pg_tviews.control
      - ./pg_tviews--0.1.0.sql:/usr/share/postgresql/17/extension/pg_tviews--0.1.0.sql
```

## Testing the Example

```bash
# Create a post
curl -X POST http://localhost:3000/api/posts \
  -H "Content-Type: application/json" \
  -d '{
    "title": "My First Post",
    "content": "Hello world!",
    "userId": 1,
    "categoryId": 1,
    "slug": "my-first-post"
  }'

# Get all posts
curl http://localhost:3000/api/posts

# Add comment (notice post automatically includes it!)
curl -X POST http://localhost:3000/api/posts/UUID-HERE/comments \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Great post!",
    "userId": 2
  }'

# Like post
curl -X POST http://localhost:3000/api/posts/UUID-HERE/like \
  -H "Content-Type: application/json" \
  -d '{"userId": 2}'
```

## Key Takeaways

1. **Automatic Refresh**: Comments and likes automatically update post JSONB
2. **No Cache Invalidation**: TVIEWs always fresh, no manual caching needed
3. **Simple Queries**: Application just queries tv_* tables for complete data
4. **Transactional**: All updates atomic within database transaction

## Complete Code

Full source code: https://github.com/your-org/pg_tviews-blog-example
```

**Deliverables**:
- 3 complete example applications
- Runnable code repositories
- Docker Compose setups
- README with instructions
- Live demos (optional)

**Acceptance Criteria**:
- [ ] All 3 examples complete
- [ ] Code runs without errors
- [ ] README instructions work
- [ ] Docker setup provided
- [ ] Published to GitHub

---

### D4: FAQ & Common Patterns (4 hours)

**Objective**: Document frequently asked questions and reusable patterns.

**Content**:

```markdown
# FAQ & Common Patterns

## Frequently Asked Questions

### General

**Q: What is pg_tviews?**

A: pg_tviews is a PostgreSQL extension that provides automatic incremental refresh for materialized views. Instead of rebuilding entire views on every change (like `REFRESH MATERIALIZED VIEW`), pg_tviews updates only the affected rows surgically.

**Q: How is this different from regular materialized views?**

A: Traditional materialized views:
- Require manual `REFRESH MATERIALIZED VIEW` commands
- Rebuild entire table every time
- Can be stale between refreshes
- Take seconds/minutes for large datasets

pg_tviews:
- Updates automatically on data changes
- Updates only affected rows
- Always fresh within transactions
- Takes milliseconds

**Q: Do I need to use FraiseQL?**

A: No! While pg_tviews was designed for FraiseQL, it works standalone. You just need to follow the trinity pattern (pk_/id/fk_ columns).

**Q: What's the trinity pattern?**

A: A schema design pattern using three types of identifiers:
- `pk_entity` (BIGINT): Internal primary key
- `id` (UUID): Public API identifier
- `fk_parent` (BIGINT): Foreign keys for cascades

**Q: Is this production-ready?**

A: v0.1.0-beta.1 is feature-complete and suitable for evaluation. We recommend testing thoroughly in staging before production use. v1.0.0 (stable) is planned for Q1 2026.

**Q: What PostgreSQL versions are supported?**

A: PostgreSQL 15, 16, and 17 are fully supported and tested.

---

### Installation & Setup

**Q: How do I install pg_tviews?**

A: See the [Installation Guide](docs/getting-started/installation.md). Quick version:
```bash
cargo install cargo-pgrx
cargo pgrx init
git clone https://github.com/your-org/pg_tviews.git
cd pg_tviews
cargo pgrx install --release
psql -c "CREATE EXTENSION pg_tviews;"
```

**Q: Do I need to install jsonb_delta?**

A: It's optional but recommended. Without it, pg_tviews uses native PostgreSQL JSONB operations (slightly slower). With it, you get 1.5-3× performance improvement.

**Q: Can I use pg_tviews with existing tables?**

A: Yes! Either:
1. Add trinity columns to existing tables (ALTER TABLE)
2. Create views implementing trinity pattern on top of existing tables

**Q: Does this work with connection poolers?**

A: Yes! pg_tviews is compatible with PgBouncer and pgpool-II. Just ensure `DISCARD ALL` is in your reset query.

---

### Usage

**Q: How do I create a TVIEW?**

A:
```sql
CREATE TABLE tv_posts AS
SELECT
    pk_post,  -- Required: integer PK
    id,       -- Required: UUID
    fk_user,  -- Required: FK for cascades
    jsonb_build_object(...) AS data  -- Required: JSONB
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

**Q: Do I need to manually refresh TVIEWs?**

A: No! That's the whole point. TVIEWs refresh automatically when source data changes.

**Q: What SQL features are supported in TVIEW SELECT?**

A: Supported:
- JOINs (all types)
- WHERE clauses
- Subqueries
- Aggregates (jsonb_agg, array_agg, COUNT, etc.)
- Window functions
- CTEs (WITH clauses)

Not supported:
- UNION/INTERSECT/EXCEPT
- Recursive CTEs
- DISTINCT ON

**Q: Can I query a TVIEW like a regular table?**

A: Yes! TVIEWs are regular PostgreSQL tables. Use standard SQL:
```sql
SELECT * FROM tv_posts WHERE data->>'title' ILIKE '%postgres%';
```

**Q: How do I filter by nested JSONB fields?**

A:
```sql
-- By top-level field
SELECT * FROM tv_posts WHERE data->>'title' = 'My Title';

-- By nested field
SELECT * FROM tv_posts WHERE data->'author'->>'name' = 'Alice';

-- By UUID FK
SELECT * FROM tv_posts WHERE user_id = 'uuid-here';
```

**Q: Can I add indexes to TVIEWs?**

A: Yes! Recommended:
```sql
CREATE INDEX idx_tv_posts_id ON tv_posts(id);
CREATE INDEX idx_tv_posts_user_id ON tv_posts(user_id);
CREATE INDEX idx_tv_posts_title ON tv_posts USING gin((data->>'title'));
```

---

### Performance

**Q: How fast is pg_tviews?**

A: Benchmarks show 2,000-12,000× improvement over traditional materialized views for incremental updates. See [Performance Results](docs/benchmarks/results.md).

**Q: What's the performance impact on writes?**

A: Minimal. Single-row updates add ~0.5-2ms for TVIEW refresh. Use statement-level triggers for bulk operations to reduce overhead.

**Q: When should I use statement-level triggers?**

A: Enable for:
- Bulk inserts/updates (>10 rows)
- ETL pipelines
- Data migrations

```sql
SELECT pg_tviews_install_stmt_triggers();
```

**Q: How do I optimize query performance?**

A:
1. Add indexes on frequently queried fields
2. Use LIMIT for large result sets
3. Filter on indexed columns (id, user_id)
4. Use prepared statements

**Q: What's the maximum supported data size?**

A: Tested up to 1M rows per table. Larger datasets should work but haven't been extensively tested. Performance scales linearly.

---

### Troubleshooting

**Q: TVIEW not updating after INSERT/UPDATE?**

A: Check:
```sql
-- Are triggers installed?
SELECT * FROM pg_trigger WHERE tgname LIKE 'tview%';

-- Is extension healthy?
SELECT * FROM pg_tviews_health_check();

-- Test manual refresh
SELECT pg_tviews_cascade('tb_post'::regclass::oid, 1);
```

**Q: Getting "MetadataNotFound" error?**

A: TVIEW wasn't created or metadata corrupted. Solution:
```sql
-- Check metadata
SELECT * FROM pg_tview_meta;

-- Recreate TVIEW
DROP TABLE tv_posts;  -- If exists
CREATE TABLE tv_posts AS SELECT ...;
```

**Q: Queries are slow on TVIEWs?**

A: Add indexes:
```sql
-- Check query plan
EXPLAIN ANALYZE SELECT * FROM tv_posts WHERE data->>'title' = 'Test';

-- If Seq Scan, add index:
CREATE INDEX idx_tv_posts_title ON tv_posts USING gin((data->>'title'));
```

**Q: High memory usage during bulk operations?**

A: Enable statement-level triggers:
```sql
SELECT pg_tviews_install_stmt_triggers();
```

---

### Migration

**Q: How do I migrate from materialized views?**

A: See [Migration Guide](docs/operations/migration.md). TL;DR:
1. Add trinity columns to tables
2. Convert MV definition to TVIEW syntax
3. Test performance
4. Cut over

**Q: Can I run pg_tviews alongside existing MVs?**

A: Yes! You can migrate incrementally.

**Q: What's the rollback plan if migration fails?**

A:
```sql
DROP EXTENSION pg_tviews CASCADE;
-- Your source tables are unchanged
-- Recreate traditional MVs
CREATE MATERIALIZED VIEW ...;
```

---

## Common Patterns

### Pattern 1: Nested Objects

```sql
CREATE TABLE tv_post AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(
        'id', id,
        'title', title,
        'author', jsonb_build_object(  -- Nested object
            'id', u.id,
            'name', u.name,
            'email', u.email
        )
    ) AS data
FROM tb_post p
JOIN tb_user u ON p.fk_user = u.pk_user;
```

### Pattern 2: Arrays of Objects

```sql
CREATE TABLE tv_post AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(
        'id', id,
        'title', title,
        'comments', COALESCE((  -- Array of comments
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', c.id,
                    'text', c.text,
                    'author', cu.name
                ) ORDER BY c.created_at DESC
            )
            FROM tb_comment c
            JOIN tb_user cu ON c.fk_user = cu.pk_user
            WHERE c.fk_post = p.pk_post
        ), '[]'::jsonb)
    ) AS data
FROM tb_post p;
```

### Pattern 3: Aggregations

```sql
CREATE TABLE tv_category AS
SELECT
    pk_category, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'postCount', COUNT(p.pk_post),  -- Aggregate
        'avgLikes', AVG(
            (SELECT COUNT(*) FROM tb_like WHERE fk_post = p.pk_post)
        )
    ) AS data
FROM tb_category c
LEFT JOIN tb_post p ON p.fk_category = c.pk_category
GROUP BY c.pk_category, c.id, c.name;
```

### Pattern 4: Computed Fields

```sql
CREATE TABLE tv_post AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(
        'id', id,
        'title', title,
        'content', content,
        'wordCount', array_length(  -- Computed
            string_to_array(content, ' '), 1
        ),
        'readingTime', CEIL(  -- Computed
            array_length(string_to_array(content, ' '), 1) / 200.0
        )
    ) AS data
FROM tb_post;
```

### Pattern 5: Conditional Fields

```sql
CREATE TABLE tv_user AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', CASE  -- Conditional
            WHEN is_admin THEN email
            ELSE NULL
        END,
        'role', CASE
            WHEN is_admin THEN 'admin'
            ELSE 'user'
        END
    ) AS data
FROM tb_user;
```

### Pattern 6: Multiple Levels of Nesting

```sql
CREATE TABLE tv_user AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'posts', COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', p.id,
                    'title', p.title,
                    'comments', COALESCE((  -- Nested array
                        SELECT jsonb_agg(
                            jsonb_build_object(
                                'id', c.id,
                                'text', c.text
                            )
                        )
                        FROM tb_comment c
                        WHERE c.fk_post = p.pk_post
                    ), '[]'::jsonb)
                )
            )
            FROM tb_post p
            WHERE p.fk_user = u.pk_user
        ), '[]'::jsonb)
    ) AS data
FROM tb_user u;
```

### Pattern 7: NULL Handling

```sql
CREATE TABLE tv_post AS
SELECT
    pk_post, id, fk_user, fk_category,
    jsonb_build_object(
        'id', id,
        'title', title,
        'category', CASE  -- Handle NULL FK
            WHEN c.id IS NOT NULL THEN
                jsonb_build_object('id', c.id, 'name', c.name)
            ELSE NULL
        END
    ) AS data
FROM tb_post p
LEFT JOIN tb_category c ON p.fk_category = c.pk_category;
```

### Pattern 8: Filtering in TVIEW Definition

```sql
-- Only published posts
CREATE TABLE tv_published_post AS
SELECT
    pk_post, id, fk_user,
    jsonb_build_object(...) AS data
FROM tb_post p
WHERE p.published = TRUE AND p.deleted_at IS NULL;
```

### Pattern 9: Separate Admin/Public Views

```sql
-- Public view (limited fields)
CREATE TABLE tv_user_public AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'avatar', avatar_url
    ) AS data
FROM tb_user;

-- Admin view (includes sensitive data)
CREATE TABLE tv_user_admin AS
SELECT
    pk_user, id,
    jsonb_build_object(
        'id', id,
        'name', name,
        'email', email,
        'phone', phone,
        'createdAt', created_at
    ) AS data
FROM tb_user;
```

### Pattern 10: Pagination Support

```sql
-- Application-level pagination
SELECT data FROM tv_posts
ORDER BY data->>'createdAt' DESC
LIMIT 20 OFFSET 0;

-- Cursor-based pagination (recommended)
SELECT data FROM tv_posts
WHERE data->>'createdAt' < '2025-01-01T00:00:00Z'
ORDER BY data->>'createdAt' DESC
LIMIT 20;
```

---

## Anti-Patterns (What NOT to Do)

### ❌ Anti-Pattern 1: Querying Other TVIEWs in TVIEW Definition

```sql
-- DON'T DO THIS (creates dependency cycle risk)
CREATE TABLE tv_post AS
SELECT
    pk_post, id,
    jsonb_build_object(
        'author', (SELECT data FROM tv_user WHERE pk_user = p.fk_user)
    ) AS data
FROM tb_post p;
```

**Instead**: Query base tables (tb_*)

### ❌ Anti-Pattern 2: Not Following Trinity Pattern

```sql
-- DON'T DO THIS (missing required columns)
CREATE TABLE tv_post AS
SELECT
    id,  -- Missing pk_post!
    title,
    jsonb_build_object('title', title) AS data
FROM tb_post;
```

**Instead**: Always include pk_*, id, fk_*, and data

### ❌ Anti-Pattern 3: Over-Nesting

```sql
-- DON'T DO THIS (too deep, performance issues)
CREATE TABLE tv_company AS
SELECT
    pk_company, id,
    jsonb_build_object(
        'divisions', (
            SELECT jsonb_agg(jsonb_build_object(
                'departments', (
                    SELECT jsonb_agg(jsonb_build_object(
                        'teams', (...)
                    ))
                )
            ))
        )
    ) AS data
FROM tb_company;
```

**Instead**: Create separate TVIEWs for each level

### ❌ Anti-Pattern 4: Not Using Indexes

```sql
-- DON'T DO THIS (slow queries without indexes)
SELECT * FROM tv_post WHERE data->>'title' = 'Test';
-- Seq Scan on tv_post (slow!)
```

**Instead**: Add appropriate indexes

### ❌ Anti-Pattern 5: Ignoring Statement-Level Triggers

```sql
-- DON'T DO THIS for bulk operations
INSERT INTO tb_post SELECT * FROM staging;  -- 10,000 rows
-- Row-level triggers fire 10,000 times!
```

**Instead**: Enable statement-level triggers first

---

## Need More Help?

- [Troubleshooting Guide](docs/operations/troubleshooting.md)
- [Error Reference](docs/reference/errors.md)
- [GitHub Discussions](https://github.com/your-org/pg_tviews/discussions)
```

**Deliverables**:
- Comprehensive FAQ (30+ questions)
- Common patterns library (10+ patterns)
- Anti-patterns guide
- Quick reference cards

**Acceptance Criteria**:
- [ ] 30+ FAQ entries
- [ ] 10+ reusable patterns
- [ ] 5+ anti-patterns documented
- [ ] All code examples tested
- [ ] Links to detailed guides

---

## Phase E: Maintenance & Quality Assurance (8-12 hours)

### E1: Documentation Testing Framework (4 hours)

**Objective**: Ensure documentation stays accurate and in sync with code.

**Tasks**:

1. **Create Docs Test Suite** (2 hours):

```bash
#!/bin/bash
# test/docs/test_examples.sh

# Test all SQL examples in documentation

set -e

echo "Testing documentation examples..."

# 1. Create test database
createdb pg_tviews_docs_test

# 2. Install extension
psql pg_tviews_docs_test -c "CREATE EXTENSION pg_tviews;"

# 3. Extract and test SQL examples from markdown
find docs -name "*.md" | while read -r file; do
    echo "Testing examples in $file..."

    # Extract SQL code blocks
    awk '/```sql/,/```/' "$file" | \
        grep -v '```' | \
        psql pg_tviews_docs_test -v ON_ERROR_STOP=1

    echo "✓ $file examples pass"
done

# 4. Cleanup
dropdb pg_tviews_docs_test

echo "All documentation examples tested successfully!"
```

2. **Add to CI/CD** (1 hour):

```yaml
# .github/workflows/docs.yml
name: Documentation Tests

on:
  push:
    paths:
      - 'docs/**'
      - '.github/workflows/docs.yml'
  pull_request:
    paths:
      - 'docs/**'

jobs:
  test-docs:
    runs-on: ubuntu-latest

    services:
      postgres:
        image: postgres:17
        env:
          POSTGRES_PASSWORD: postgres
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
        ports:
          - 5432:5432

    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install pgrx
        run: cargo install --locked cargo-pgrx

      - name: Install pg_tviews
        run: cargo pgrx install --release

      - name: Test documentation examples
        run: bash test/docs/test_examples.sh

      - name: Check for broken links
        uses: lycheeverse/lychee-action@v1
        with:
          args: --verbose --no-progress 'docs/**/*.md'

      - name: Spell check
        uses: rojopolis/spellcheck-github-actions@0.28.0
```

3. **Create Documentation Style Linter** (1 hour):

```python
#!/usr/bin/env python3
# test/docs/lint_docs.py

"""
Documentation linter to enforce style guide.
"""

import re
import sys
from pathlib import Path

ERRORS = []

def check_file(filepath):
    """Check a single markdown file for style violations."""
    with open(filepath, 'r') as f:
        content = f.read()
        lines = content.split('\n')

    # Check 1: No TODO comments
    if 'TODO' in content:
        ERRORS.append(f"{filepath}: Contains TODO comments")

    # Check 2: Code blocks have language specified
    if re.search(r'```\n', content):
        ERRORS.append(f"{filepath}: Code block without language specification")

    # Check 3: Headings use proper hierarchy
    prev_level = 0
    for i, line in enumerate(lines):
        if line.startswith('#'):
            level = len(re.match(r'^#+', line).group())
            if level > prev_level + 1:
                ERRORS.append(f"{filepath}:{i+1}: Skipped heading level (went from h{prev_level} to h{level})")
            prev_level = level

    # Check 4: Links are not broken (internal only)
    for match in re.finditer(r'\[([^\]]+)\]\(([^)]+)\)', content):
        link = match.group(2)
        if not link.startswith('http') and not Path(filepath.parent / link).exists():
            ERRORS.append(f"{filepath}: Broken internal link: {link}")

    # Check 5: All examples have expected output
    code_blocks = re.findall(r'```sql(.*?)```', content, re.DOTALL)
    for block in code_blocks:
        if 'SELECT' in block and 'Expected' not in content[content.find(block):content.find(block)+500]:
            ERRORS.append(f"{filepath}: SELECT query without expected output")

def main():
    docs_dir = Path('docs')

    for md_file in docs_dir.rglob('*.md'):
        check_file(md_file)

    if ERRORS:
        print("Documentation style errors found:\n")
        for error in ERRORS:
            print(f"  ❌ {error}")
        sys.exit(1)
    else:
        print("✅ All documentation style checks passed!")
        sys.exit(0)

if __name__ == '__main__':
    main()
```

**Deliverables**:
- Documentation test suite
- CI/CD integration
- Style linter
- Broken link checker

**Acceptance Criteria**:
- [ ] All code examples tested automatically
- [ ] CI fails on doc errors
- [ ] Style guide enforced by linter
- [ ] Links validated

---

### E2: Documentation Update Process (2 hours)

**Objective**: Establish process to keep docs in sync with code changes.

**Process Documentation**:

```markdown
# Documentation Update Process

## When to Update Documentation

Update documentation when:

1. **Adding a new feature**
   - Update API reference
   - Add usage examples
   - Update changelog
   - Add to appropriate user guide

2. **Changing existing behavior**
   - Update all affected docs
   - Add migration note if breaking
   - Update examples
   - Add to changelog

3. **Fixing a bug**
   - Update error reference if new error
   - Update troubleshooting guide
   - Add to changelog

4. **Deprecating a feature**
   - Add deprecation notice
   - Update migration guide
   - Set removal timeline
   - Update all examples to use new approach

## Pull Request Checklist

Before merging a PR that affects user-facing behavior:

- [ ] Documentation updated (if applicable)
- [ ] Examples tested and work
- [ ] Changelog entry added
- [ ] Migration notes added (if breaking)
- [ ] Screenshots updated (if UI change)
- [ ] Version number incremented (if release)

## Review Process

1. **Code author** updates docs alongside code
2. **Code reviewer** verifies docs match implementation
3. **Tech writer** (if available) reviews for clarity
4. **CI** validates examples and checks links

## Documentation Locations

| Change Type | Update These Docs |
|-------------|------------------|
| New function | API Reference, User Guide, Changelog |
| New DDL command | DDL Reference, User Guide, Changelog |
| Performance change | Performance Tuning Guide, Benchmark Results |
| Bug fix | Error Reference, Troubleshooting, Changelog |
| Breaking change | Migration Guide, Changelog, All affected guides |
| Security fix | Security Reference, Changelog |

## Version Changelog Template

```markdown
## [X.Y.Z] - YYYY-MM-DD

### Added
- New feature description

### Changed
- Changed behavior description

### Deprecated
- Deprecated feature with removal timeline

### Removed
- Removed feature (with migration path)

### Fixed
- Bug fix description

### Security
- Security fix description
```

## Deprecation Policy

When deprecating a feature:

1. **Add warning** to documentation
   ```markdown
   ⚠️ **Deprecated**: This feature is deprecated and will be removed in v2.0.0.
   Use [new approach](link) instead.
   ```

2. **Add runtime warning** (if possible)
   ```sql
   WARNING: Function pg_tviews_old() is deprecated.
   Use pg_tviews_new() instead. Support will be removed in v2.0.0.
   ```

3. **Provide migration path** in docs

4. **Maintain for at least 2 minor versions** before removal

## Documentation Review Schedule

- **Monthly**: Review for accuracy (spot-check examples)
- **Per release**: Full documentation audit
- **Quarterly**: User feedback review and improvements

## Tools

- **Markdown linter**: `test/docs/lint_docs.py`
- **Example tester**: `test/docs/test_examples.sh`
- **Link checker**: GitHub Actions (lychee)
- **Spell checker**: GitHub Actions (rojopolis/spellcheck)

## Contact

Documentation questions: docs@your-domain.com
```

**Deliverables**:
- Update process documentation
- PR checklist template
- Deprecation policy
- Review schedule

**Acceptance Criteria**:
- [ ] Process documented clearly
- [ ] Checklist integrated into PR template
- [ ] Deprecation policy established
- [ ] Schedule defined

---

### E3: User Feedback Integration (3 hours)

**Objective**: Create channels for users to improve documentation.

**Tasks**:

1. **Add Feedback Widgets** (1 hour):

```html
<!-- Add to bottom of each doc page -->
<div class="doc-feedback">
  <h3>Was this page helpful?</h3>
  <button onclick="submitFeedback('yes')">👍 Yes</button>
  <button onclick="submitFeedback('no')">👎 No</button>

  <div id="feedback-detail" style="display:none;">
    <textarea id="feedback-text" placeholder="What could be improved?"></textarea>
    <button onclick="submitDetailedFeedback()">Submit</button>
  </div>
</div>

<script>
function submitFeedback(helpful) {
  if (helpful === 'no') {
    document.getElementById('feedback-detail').style.display = 'block';
  } else {
    // Track positive feedback
    analytics.track('doc_helpful', { page: window.location.pathname });
    alert('Thank you for your feedback!');
  }
}

function submitDetailedFeedback() {
  const feedback = document.getElementById('feedback-text').value;
  analytics.track('doc_feedback', {
    page: window.location.pathname,
    feedback: feedback
  });
  alert('Thank you! We\'ll use your feedback to improve this page.');
  document.getElementById('feedback-detail').style.display = 'none';
}
</script>
```

2. **Create Feedback Issue Template** (30 min):

```yaml
# .github/ISSUE_TEMPLATE/documentation.yml
name: Documentation Improvement
description: Suggest improvements to documentation
title: "[Docs]: "
labels: ["documentation"]
body:
  - type: dropdown
    id: doc-type
    attributes:
      label: Documentation Type
      options:
        - API Reference
        - User Guide
        - Tutorial
        - Error Reference
        - Operations Guide
        - Other
    validations:
      required: true

  - type: input
    id: doc-location
    attributes:
      label: Documentation Location
      description: Which page/file needs improvement?
      placeholder: "docs/getting-started/quickstart.md"
    validations:
      required: true

  - type: textarea
    id: issue
    attributes:
      label: What's unclear or missing?
      description: Describe what's confusing, incorrect, or missing
    validations:
      required: true

  - type: textarea
    id: suggestion
    attributes:
      label: Suggested improvement
      description: How could we make this better?

  - type: dropdown
    id: severity
    attributes:
      label: Impact
      options:
        - Blocker (can't use feature without this)
        - High (significantly affects usability)
        - Medium (would improve clarity)
        - Low (nice to have)
    validations:
      required: true
```

3. **Create Documentation Survey** (30 min):

```markdown
# pg_tviews Documentation Survey

Help us improve our documentation!

## How did you find our documentation?

- [ ] Very easy to find what I needed
- [ ] Somewhat easy
- [ ] Somewhat difficult
- [ ] Very difficult

## Rate the following aspects (1-5, 5 = excellent):

- Completeness: ___
- Accuracy: ___
- Clarity: ___
- Examples: ___
- Organization: ___

## Which sections were most helpful?

- [ ] Quick Start
- [ ] API Reference
- [ ] Tutorials
- [ ] Troubleshooting
- [ ] Performance Tuning
- [ ] Error Reference
- [ ] Other: ___________

## What's missing or could be improved?

_________________________________

## Would you recommend pg_tviews to others?

- [ ] Definitely
- [ ] Probably
- [ ] Maybe
- [ ] Probably not
- [ ] Definitely not

## Any other feedback?

_________________________________

Thank you! Submit responses to: docs-survey@your-domain.com
```

4. **Track Documentation Metrics** (1 hour):

```python
# analytics/doc_metrics.py

"""
Track documentation metrics.
"""

import json
from datetime import datetime, timedelta

class DocMetrics:
    def __init__(self):
        self.metrics = {
            'page_views': {},
            'helpful_votes': {},
            'time_on_page': {},
            'bounce_rate': {},
            'search_queries': [],
            'feedback': []
        }

    def track_page_view(self, page):
        """Track page view."""
        if page not in self.metrics['page_views']:
            self.metrics['page_views'][page] = 0
        self.metrics['page_views'][page] += 1

    def track_helpful_vote(self, page, helpful):
        """Track helpful/not helpful vote."""
        if page not in self.metrics['helpful_votes']:
            self.metrics['helpful_votes'][page] = {'yes': 0, 'no': 0}
        self.metrics['helpful_votes'][page]['yes' if helpful else 'no'] += 1

    def track_search(self, query, found_result):
        """Track documentation search."""
        self.metrics['search_queries'].append({
            'query': query,
            'found': found_result,
            'timestamp': datetime.now().isoformat()
        })

    def get_top_pages(self, n=10):
        """Get most viewed pages."""
        return sorted(
            self.metrics['page_views'].items(),
            key=lambda x: x[1],
            reverse=True
        )[:n]

    def get_problem_pages(self):
        """Get pages with low helpful votes."""
        problem_pages = []
        for page, votes in self.metrics['helpful_votes'].items():
            total = votes['yes'] + votes['no']
            if total > 10:  # Minimum sample size
                helpful_rate = votes['yes'] / total
                if helpful_rate < 0.6:  # Less than 60% helpful
                    problem_pages.append((page, helpful_rate))
        return sorted(problem_pages, key=lambda x: x[1])

    def get_missing_content(self):
        """Analyze search queries that didn't find results."""
        missing = {}
        for search in self.metrics['search_queries']:
            if not search['found']:
                query = search['query']
                missing[query] = missing.get(query, 0) + 1
        return sorted(missing.items(), key=lambda x: x[1], reverse=True)[:10]

    def generate_report(self):
        """Generate monthly documentation report."""
        return {
            'top_pages': self.get_top_pages(),
            'problem_pages': self.get_problem_pages(),
            'missing_content': self.get_missing_content(),
            'total_page_views': sum(self.metrics['page_views'].values()),
            'average_helpful_rate': self._calc_avg_helpful_rate()
        }

    def _calc_avg_helpful_rate(self):
        """Calculate average helpful rate across all pages."""
        total_yes = sum(v['yes'] for v in self.metrics['helpful_votes'].values())
        total_votes = sum(
            v['yes'] + v['no'] for v in self.metrics['helpful_votes'].values()
        )
        return total_yes / total_votes if total_votes > 0 else 0

# Usage
metrics = DocMetrics()

# In your docs site:
# metrics.track_page_view('/docs/getting-started/quickstart')
# metrics.track_helpful_vote('/docs/getting-started/quickstart', True)

# Monthly report:
# report = metrics.generate_report()
# print(json.dumps(report, indent=2))
```

**Deliverables**:
- Feedback widgets on doc pages
- GitHub issue templates
- User survey
- Metrics tracking system

**Acceptance Criteria**:
- [ ] Feedback mechanism deployed
- [ ] Issue templates created
- [ ] Survey distributed
- [ ] Metrics being tracked
- [ ] Monthly reports generated

---

### E4: Documentation Quality Scorecard (3 hours)

**Objective**: Create measurable quality standards and track progress.

**Scorecard**:

```markdown
# Documentation Quality Scorecard

## Scoring Criteria

Each documentation page is scored 0-100 based on:

### Completeness (40 points)
- [ ] 10 pts: All features documented
- [ ] 10 pts: All parameters/options explained
- [ ] 10 pts: Examples provided
- [ ] 10 pts: Edge cases covered

### Accuracy (30 points)
- [ ] 15 pts: Examples tested and work
- [ ] 10 pts: Code matches current version
- [ ] 5 pts: No broken links

### Clarity (20 points)
- [ ] 10 pts: Clear, concise writing
- [ ] 5 pts: Proper formatting
- [ ] 5 pts: Good organization

### Usability (10 points)
- [ ] 5 pts: Easy to navigate
- [ ] 3 pts: Search-friendly
- [ ] 2 pts: Mobile-friendly

## Quality Levels

- **A+ (90-100)**: Exemplary documentation
- **A (80-89)**: Excellent documentation
- **B (70-79)**: Good documentation, minor improvements needed
- **C (60-69)**: Acceptable, needs significant improvement
- **D (50-59)**: Poor documentation, major gaps
- **F (<50)**: Inadequate, requires rewrite

## Current Scores (as of 2025-12-11)

| Document | Score | Grade | Notes |
|----------|-------|-------|-------|
| README.md | 85 | A | Good overview, add more examples |
| API Reference | 90 | A+ | Comprehensive, well-tested |
| Quick Start | 80 | A | Clear steps, add troubleshooting |
| Migration Guide | 75 | B | Needs more edge cases |
| Error Reference | 70 | B | Missing some error types |
| DDL Reference | 85 | A | Good syntax docs, add limitations |
| Performance Tuning | 60 | C | Incomplete, needs expansion |
| Disaster Recovery | 0 | F | Not yet written |

## Target Goals

### Q1 2026 (v1.0.0 Release)
- All critical docs: A or better (80+)
- Average score: 85+
- No docs below C (70)

### Actions Required

**High Priority** (score <70):
1. ❌ Write Disaster Recovery Guide
2. ⚠️ Expand Performance Tuning Guide
3. ⚠️ Complete Error Reference

**Medium Priority** (score 70-79):
1. Enhance Migration Guide with more examples
2. Add troubleshooting section to Quick Start

**Continuous Improvement** (score 80+):
1. Gather user feedback
2. Add more real-world examples
3. Keep examples up-to-date

## Measurement Process

### Quarterly Reviews
1. Score all documentation pages
2. Identify lowest-scoring pages
3. Create improvement tasks
4. Track progress

### User Feedback
- Helpful votes <60%: Automatic review
- Repeated feedback themes: Prioritize improvements
- High bounce rates: Investigate and improve

### Automated Checks
- CI tests all examples (weekly)
- Link validation (daily)
- Spell check (on commit)
- Style lint (on commit)

## Success Metrics

Documentation is A+ quality when:

- ✅ Average score >90
- ✅ All critical docs >80
- ✅ No docs <70
- ✅ User helpful rate >85%
- ✅ Support questions <10/month due to docs
- ✅ 100% of examples tested and work
- ✅ Zero broken links
- ✅ Updated within 1 week of code changes
```

**Deliverables**:
- Quality scorecard template
- Scoring methodology
- Current baseline scores
- Improvement targets

**Acceptance Criteria**:
- [ ] All docs scored
- [ ] Improvement targets set
- [ ] Action items identified
- [ ] Review process established
- [ ] Success metrics defined

---

## Execution Summary

### Total Effort Estimate

| Phase | Sub-Phases | Hours | Difficulty |
|-------|-----------|-------|-----------|
| **A: Foundation** | 5 tasks | 16-24 | Medium |
| **B: Reference** | 6 tasks | 32-48 | Medium |
| **C: Operations** | 4 tasks | 24-32 | Hard |
| **D: Learning** | 4 tasks | 16-24 | Easy |
| **E: Maintenance** | 4 tasks | 8-12 | Easy |
| **Total** | **23 tasks** | **96-140 hours** | Mixed |

### Timeline Estimates

**Full-Time (40 hrs/week)**:
- Best case: 2.5 weeks
- Realistic: 3-4 weeks
- With reviews: 4-5 weeks

**Part-Time (20 hrs/week)**:
- Best case: 5 weeks
- Realistic: 6-8 weeks
- With reviews: 8-10 weeks

**Team of 2** (40 hrs/week each):
- Parallel work: 2-3 weeks
- With reviews: 3-4 weeks

### Recommended Approach

**Sprint 1-2** (Weeks 1-2): Foundation & Fix Inconsistencies
- Phase A complete
- Fix all critical issues
- Establish standards

**Sprint 3-4** (Weeks 2-4): Complete Reference Docs
- Phase B complete
- All APIs, DDL, SQL, errors documented
- Configuration reference

**Sprint 5-6** (Weeks 4-6): Operational Excellence
- Phase C complete
- Migration guide
- Disaster recovery
- Production checklist
- Performance tuning

**Sprint 7-8** (Weeks 6-8): Learning & Quality
- Phase D & E complete
- Tutorials and examples
- Documentation testing
- Quality scorecard
- User feedback systems

### Success Criteria

Documentation achieves A+ quality when:

✅ **Completeness**:
- 100% of public APIs documented
- All error types explained
- All operational procedures covered
- Migration paths documented

✅ **Accuracy**:
- 100% of examples tested in CI
- Code matches documentation
- Zero broken links
- Updated within 1 week of code changes

✅ **Usability**:
- User helpful rate >85%
- Support questions <10/month
- 90%+ can complete tasks without help
- Clear learning paths for all personas

✅ **Maintainability**:
- Automated testing
- Style enforcement
- Update process established
- Quality metrics tracked

✅ **Impact**:
- Users choose pg_tviews because of docs
- Onboarding time <2 hours
- Migration success rate >95%
- Community contributions increase

---

## Next Steps

1. **Review this plan** with stakeholders
2. **Prioritize phases** based on release timeline
3. **Assign ownership** for each phase
4. **Set milestones** and deadlines
5. **Begin Phase A** (Foundation)

Good luck achieving A+ documentation quality! 🎉
