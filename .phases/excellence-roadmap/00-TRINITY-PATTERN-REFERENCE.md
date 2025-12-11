# Trinity Pattern Reference Guide
**READ THIS FIRST** - All roadmap tasks assume this pattern

---

## Overview: The Trinity Pattern

Every entity in pg_tviews consists of **three components**:

1. **`tb_{entity}`** - Base table (source of truth, singular name)
2. **`tv_{entity}`** - TVIEW table (cached materialized data, singular name)
3. **`v_{entity}`** - Backing view (refresh definition, singular name)

### Key Principles

- ✅ **Always singular**: `tb_post`, `tv_post`, `v_post` (NEVER `tb_posts`)
- ✅ **id is always UUID**: For external API/GraphQL
- ✅ **pk_{entity} is always INTEGER**: For internal DB operations
- ✅ **fk_{entity} is always INTEGER**: Foreign keys reference pk_* columns
- ✅ **Always qualify columns**: `tb_post.id` not just `id`
- ✅ **JSONB uses camelCase**: `'userId'` not `'user_id'`

---

## 1. Complete Base Table Pattern (tb_*)

```sql
-- ========================================
-- CANONICAL BASE TABLE PATTERN
-- ========================================
-- Copy this pattern for ALL base table examples

CREATE TABLE tb_post (
  -- Trinity Pattern: Always these 2 columns first
  pk_post SERIAL PRIMARY KEY,                      -- Integer PK (internal DB use)
  id UUID NOT NULL DEFAULT gen_random_uuid(),      -- UUID (external API/GraphQL)

  -- Foreign keys: Always fk_* and always INTEGER
  fk_user INTEGER NOT NULL,                        -- References tb_user(pk_user)
  fk_category INTEGER,                             -- Optional FK (can be NULL)

  -- Business columns (your domain data)
  title TEXT NOT NULL,
  content TEXT,
  published_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

  -- Constraints
  CONSTRAINT fk_post_user FOREIGN KEY (fk_user)
    REFERENCES tb_user(pk_user) ON DELETE CASCADE,
  CONSTRAINT fk_post_category FOREIGN KEY (fk_category)
    REFERENCES tb_category(pk_category) ON DELETE SET NULL
);

-- MANDATORY: UUID index for GraphQL queries
CREATE INDEX idx_tb_post_id ON tb_post(id);

-- MANDATORY: FK indexes for cascade performance
CREATE INDEX idx_tb_post_fk_user ON tb_post(fk_user);
CREATE INDEX idx_tb_post_fk_category ON tb_post(fk_category);

-- Comments for clarity
COMMENT ON TABLE tb_post IS 'Base table for posts (Trinity pattern)';
COMMENT ON COLUMN tb_post.pk_post IS 'Integer primary key (internal DB operations)';
COMMENT ON COLUMN tb_post.id IS 'UUID identifier (external API/GraphQL)';
COMMENT ON COLUMN tb_post.fk_user IS 'Foreign key to tb_user.pk_user (integer)';
```

---

## 2. Complete TVIEW Pattern (tv_*)

```sql
-- ========================================
-- CANONICAL TVIEW PATTERN
-- ========================================
-- Copy this pattern for ALL TVIEW examples

CREATE TABLE tv_post AS
SELECT
  -- 1. ALWAYS select pk_{entity} first (integer primary key)
  tb_post.pk_post,

  -- 2. ALWAYS select id second (UUID for GraphQL)
  tb_post.id,

  -- 3. ALWAYS select ALL fk_* columns (integers, for cascade tracking)
  tb_post.fk_user,
  tb_post.fk_category,

  -- 4. ALWAYS build JSONB data column last
  jsonb_build_object(
    -- Include UUID id for GraphQL queries
    'id', tb_post.id,

    -- Include business data (use camelCase for keys)
    'title', tb_post.title,
    'content', tb_post.content,
    'publishedAt', tb_post.published_at,  -- camelCase, not published_at

    -- Include foreign UUIDs for GraphQL relations (from JOINs)
    'userId', tb_user.id,           -- UUID from joined table
    'categoryId', tb_category.id,   -- UUID (can be NULL)

    -- Include denormalized parent data
    'userName', tb_user.name,
    'categoryName', COALESCE(tb_category.name, 'Uncategorized')  -- Handle NULL
  ) as data

FROM tb_post
-- ALWAYS use explicit JOINs to get related UUIDs and denormalized data
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- Result: tv_post has these columns:
-- - pk_post (SERIAL)    → for internal cascade operations
-- - id (UUID)           → for GraphQL queries by UUID
-- - fk_user (INTEGER)   → for cascade tracking
-- - fk_category (INTEGER) → for cascade tracking
-- - data (JSONB)        → for GraphQL field resolution
```

**MANDATORY Indexes for Every TVIEW:**
```sql
-- 1. UUID index (for GraphQL queries)
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- 2. Foreign key indexes (for cascade performance)
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_post_fk_category ON tv_post(fk_category);

-- 3. Optional: JSONB GIN index (if querying JSONB fields directly)
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);
```

---

## 3. Backing View Pattern (v_*)

```sql
-- ========================================
-- BACKING VIEW PATTERN
-- ========================================
-- pg_tviews creates this automatically

CREATE OR REPLACE VIEW v_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  tb_post.fk_category,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'content', tb_post.content,
    'publishedAt', tb_post.published_at,
    'userId', tb_user.id,
    'categoryId', tb_category.id,
    'userName', tb_user.name,
    'categoryName', COALESCE(tb_category.name, 'Uncategorized')
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- The v_* view is used by triggers to refresh tv_* data
-- When tb_post is updated, trigger executes:
--   UPDATE tv_post
--   SET data = v_post.data, fk_user = v_post.fk_user, fk_category = v_post.fk_category
--   WHERE tv_post.pk_post = NEW.pk_post;
```

---

## 4. Complete End-to-End Example

```sql
-- ========================================
-- FULL WORKFLOW: CREATE → INSERT → UPDATE → CASCADE
-- ========================================

-- Step 1: Create parent table (tb_user)
CREATE TABLE tb_user (
  pk_user SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE
);

CREATE INDEX idx_tb_user_id ON tb_user(id);

CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'name', tb_user.name,
    'email', tb_user.email
  ) as data
FROM tb_user;

CREATE INDEX idx_tv_user_id ON tv_user(id);

-- Step 2: Create child table (tb_post)
CREATE TABLE tb_post (
  pk_post SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_user INTEGER NOT NULL REFERENCES tb_user(pk_user),
  title TEXT NOT NULL,
  content TEXT
);

CREATE INDEX idx_tb_post_id ON tb_post(id);
CREATE INDEX idx_tb_post_fk_user ON tb_post(fk_user);

CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'content', tb_post.content,
    'userId', tb_user.id,        -- UUID for GraphQL
    'userName', tb_user.name     -- Denormalized
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);

-- Step 3: Insert data (use INTEGER pk_* for FKs)
INSERT INTO tb_user (name, email) VALUES
  ('Alice', 'alice@example.com'),
  ('Bob', 'bob@example.com');

INSERT INTO tb_post (fk_user, title, content) VALUES
  (1, 'First Post', 'Hello World'),   -- fk_user=1 is Alice's pk_user
  (2, 'Second Post', 'From Bob');     -- fk_user=2 is Bob's pk_user

-- Step 4: Query by integer PK (internal operations)
SELECT * FROM tv_post WHERE tv_post.pk_post = 1;

-- Step 5: Query by UUID (GraphQL pattern)
SELECT tv_post.data
FROM tv_post
WHERE tv_post.id = (SELECT tb_post.id FROM tb_post WHERE tb_post.pk_post = 1);

-- Step 6: Update child (triggers auto-update tv_post)
UPDATE tb_post
SET title = 'Updated Title'
WHERE tb_post.pk_post = 1;

-- Step 7: Verify child cascade
SELECT tv_post.data->>'title' FROM tv_post WHERE tv_post.pk_post = 1;
-- Returns: 'Updated Title'

-- Step 8: Update parent (cascades to tv_post!)
UPDATE tb_user
SET name = 'Alice Smith'
WHERE tb_user.pk_user = 1;

-- Step 9: Verify parent cascade to child TVIEW
SELECT tv_post.data->>'userName' FROM tv_post WHERE tv_post.fk_user = 1;
-- Returns: 'Alice Smith' (cascaded from tb_user update!)
```

---

## 5. Data Modification Patterns

### INSERT: Always into tb_*, never tv_*
```sql
-- ✅ CORRECT: Insert into base table
INSERT INTO tb_post (fk_user, title, content)
VALUES (
  1,  -- fk_user is INTEGER (pk_user from tb_user)
  'New Post',
  'Content here'
);
-- TVIEW automatically populated by trigger

-- ❌ WRONG: Don't insert into TVIEW
INSERT INTO tv_post (pk_post, id, data) VALUES (...);  -- ERROR
```

### UPDATE: Always tb_*, TVIEW cascades automatically
```sql
-- ✅ CORRECT: Update by integer PK
UPDATE tb_post
SET title = 'Updated'
WHERE tb_post.pk_post = 1;
-- tv_post automatically updated via trigger

-- ✅ CORRECT: Update by UUID (if you have it)
UPDATE tb_post
SET title = 'Updated'
WHERE tb_post.id = 'uuid-here';

-- ❌ WRONG: Don't update TVIEW directly
UPDATE tv_post SET data = '{"title": "New"}' WHERE pk_post = 1;  -- DON'T
```

### DELETE: Always tb_*, TVIEW cascades automatically
```sql
-- ✅ CORRECT: Delete by integer PK
DELETE FROM tb_post WHERE tb_post.pk_post = 1;
-- tv_post row automatically deleted via CASCADE

-- ✅ CORRECT: Delete by UUID
DELETE FROM tb_post WHERE tb_post.id = 'uuid-here';

-- ❌ WRONG: Don't delete from TVIEW
DELETE FROM tv_post WHERE pk_post = 1;  -- DON'T
```

### Finding PK from UUID
```sql
-- Get integer pk_post from UUID
SELECT tb_post.pk_post
FROM tb_post
WHERE tb_post.id = 'uuid-from-graphql';

-- Use in operations
DELETE FROM tb_post
WHERE tb_post.pk_post = (
  SELECT tb_post.pk_post FROM tb_post WHERE tb_post.id = 'uuid-here'
);
```

---

## 6. When to Use INTEGER vs UUID

### Use INTEGER (pk_*, fk_*):
- ✅ Internal database operations (JOINs, foreign keys)
- ✅ Cascade update tracking (triggers use pk_* to match rows)
- ✅ WHERE clauses for direct table access
- ✅ ALL foreign key columns (fk_user, fk_post, etc.)

**Example:**
```sql
UPDATE tv_post
SET data = v_post.data
WHERE tv_post.pk_post = NEW.pk_post;  -- INTEGER comparison
```

### Use UUID (id):
- ✅ External API responses (GraphQL, REST)
- ✅ Public-facing identifiers (URLs, client references)
- ✅ JSONB data content (for GraphQL resolvers)
- ✅ Cross-system references

**Example:**
```sql
SELECT tv_post.data
FROM tv_post
WHERE tv_post.id = $graphql_id;  -- UUID from client
```

### Rule of Thumb:
- **Database ↔ Database**: Use `pk_*/fk_*` (integers)
- **Database → Client**: Use `id` (UUID)
- **Client → Database**: Convert UUID to `pk_*` in resolver

---

## 7. JSONB Naming Conventions

### ✅ CORRECT: Use camelCase (GraphQL convention)
```sql
jsonb_build_object(
  'id', tb_post.id,                    -- lowercase for single word
  'userId', tb_user.id,                -- camelCase for compound
  'publishedAt', tb_post.published_at, -- camelCase
  'categoryId', tb_post.fk_category    -- camelCase
)
```

### ❌ WRONG: Don't use snake_case
```sql
jsonb_build_object(
  'user_id', tb_user.id,               -- NO
  'published_at', tb_post.published_at -- NO
)
```

### ❌ WRONG: Don't include pk_*/fk_* in JSONB
```sql
jsonb_build_object(
  'pk_post', tb_post.pk_post,  -- NO - internal PK, not for API
  'fk_user', tb_post.fk_user   -- NO - use 'userId' with UUID instead
)
```

**Rule:** JSONB is for external API (GraphQL) - use camelCase and UUIDs, not integers.

---

## 8. Handling NULL Foreign Keys

```sql
-- Scenario: Optional foreign key (fk_category can be NULL)

CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  tb_post.fk_category,  -- Can be NULL
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,

    -- Required FK (INNER JOIN): Always non-null
    'userId', tb_user.id,
    'userName', tb_user.name,

    -- Optional FK (LEFT JOIN): Handle NULL gracefully
    'categoryId', tb_category.id,  -- Will be JSON null if no category
    'categoryName', COALESCE(tb_category.name, 'Uncategorized')
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- Query posts without category:
SELECT * FROM tv_post WHERE tv_post.fk_category IS NULL;

-- JSON behavior with NULL:
SELECT tv_post.data->'categoryId' FROM tv_post WHERE tv_post.fk_category IS NULL;
-- Returns: null (JSON null, not SQL NULL)
```

---

## 9. Multi-Level Cascade Example

```sql
-- ========================================
-- 3-LEVEL CASCADE: User → Post → Comment
-- ========================================

-- Level 1: Users (no dependencies)
CREATE TABLE tb_user (
  pk_user SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  name TEXT NOT NULL
);

CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object('id', tb_user.id, 'name', tb_user.name) as data
FROM tb_user;

-- Level 2: Posts (depends on User)
CREATE TABLE tb_post (
  pk_post SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_user INTEGER NOT NULL REFERENCES tb_user(pk_user),
  title TEXT NOT NULL
);

CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'userId', tb_user.id,
    'userName', tb_user.name  -- Denormalized from parent
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

-- Level 3: Comments (depends on Post AND User transitively)
CREATE TABLE tb_comment (
  pk_comment SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_post INTEGER NOT NULL REFERENCES tb_post(pk_post),
  content TEXT NOT NULL
);

CREATE TABLE tv_comment AS
SELECT
  tb_comment.pk_comment,
  tb_comment.id,
  tb_comment.fk_post,
  jsonb_build_object(
    'id', tb_comment.id,
    'content', tb_comment.content,
    'postId', tb_post.id,
    'postTitle', tb_post.title,
    'authorId', tb_user.id,    -- Denormalized from GRANDPARENT
    'authorName', tb_user.name
  ) as data
FROM tb_comment
INNER JOIN tb_post ON tb_comment.fk_post = tb_post.pk_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

-- Test cascade:
-- 1. Update user name (grandparent)
UPDATE tb_user SET name = 'Alice Smith' WHERE pk_user = 1;

-- 2. Verify cascade to posts (1 level)
SELECT data->>'userName' FROM tv_post WHERE fk_user = 1;
-- Returns: 'Alice Smith'

-- 3. Verify cascade to comments (2 levels deep!)
SELECT data->>'authorName' FROM tv_comment
WHERE fk_post IN (SELECT pk_post FROM tb_post WHERE fk_user = 1);
-- Returns: 'Alice Smith' (cascaded through 2 levels!)
```

---

## 10. Common Errors and Solutions

```sql
-- ERROR: duplicate key value violates unique constraint "tv_post_pkey"
-- CAUSE: Trying to insert duplicate pk_post
-- SOLUTION: Don't insert into tv_* directly, insert into tb_*

-- ERROR: column "pk_post" does not exist
-- CAUSE: Missing column in SELECT or using wrong table alias
-- SOLUTION: Ensure SELECT includes tb_post.pk_post (qualified)

-- ERROR: foreign key violation on column "fk_user"
-- CAUSE: Trying to reference non-existent pk_user
-- SOLUTION: Ensure parent record exists in tb_user first

-- ERROR: column reference "id" is ambiguous
-- CAUSE: Multiple tables in JOIN have "id" column, not qualified
-- SOLUTION: Always use table.column syntax (tb_post.id, not just id)

-- WARNING: TVIEW not updating after base table change
-- CAUSE: Triggers not installed or transaction not committed
-- SOLUTION:
--   1. COMMIT; (ensure transaction completes)
--   2. SELECT * FROM pg_trigger WHERE tgrelid = 'tb_post'::regclass;
--   3. If missing, recreate TVIEW

-- WARNING: Slow cascade updates
-- CAUSE: Missing indexes on fk_* columns
-- SOLUTION: CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
```

---

## Quick Reference Checklist

When creating ANY example in the roadmap:

### Base Table (tb_*):
- [ ] `pk_{entity} SERIAL PRIMARY KEY` as first column
- [ ] `id UUID NOT NULL DEFAULT gen_random_uuid()` as second column
- [ ] All FKs named `fk_{entity}` and type INTEGER
- [ ] Index on `id` column
- [ ] Index on each `fk_*` column
- [ ] Singular name (tb_post, not tb_posts)

### TVIEW (tv_*):
- [ ] SELECT `pk_{entity}` first
- [ ] SELECT `id` second
- [ ] SELECT all `fk_*` columns
- [ ] JSONB uses camelCase keys
- [ ] All columns qualified (tb_post.id not id)
- [ ] INNER/LEFT JOIN to get parent UUIDs
- [ ] Index on `id` column
- [ ] Index on each `fk_*` column
- [ ] Singular name (tv_post, not tv_posts)

### JSONB Data:
- [ ] Include `id` (UUID) for GraphQL
- [ ] Use camelCase for all keys
- [ ] Include foreign UUIDs (`userId` not `fk_user`)
- [ ] Handle NULL with COALESCE where needed
- [ ] No pk_*/fk_* integers in JSON

### SQL Patterns:
- [ ] All columns qualified (tb_post.column)
- [ ] Use INTEGER for pk_*/fk_* comparisons
- [ ] Use UUID for external API queries
- [ ] Modify tb_* only, never tv_*
- [ ] Let triggers handle tv_* updates

---

**Remember:** When in doubt, refer to this guide. All examples in the roadmap must follow these patterns exactly.
