# Trinity Pattern Clarifications for EXCELLENCE_ROADMAP.md
# Missing Examples and Implicit Assumptions

## Issues Found

### 1. **MISSING: Complete Canonical Base Table Example**

The roadmap shows fragments but never a complete "reference implementation" for tb_* tables.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- CANONICAL BASE TABLE PATTERN (tb_*)
-- ========================================
-- This is the EXACT pattern for ALL base tables in examples

CREATE TABLE tb_post (
  -- Trinity Pattern: Always these 2 columns first
  pk_post SERIAL PRIMARY KEY,        -- Integer PK for internal use
  id UUID NOT NULL DEFAULT gen_random_uuid(),  -- UUID for external API (GraphQL)

  -- Foreign keys: Always fk_* and always INTEGER (never UUID)
  fk_user INTEGER NOT NULL,          -- References tb_user(pk_user)
  fk_category INTEGER,               -- Optional FK (can be NULL)

  -- Business columns
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

-- Always create UUID index for GraphQL queries
CREATE INDEX idx_tb_post_id ON tb_post(id);

-- Always create FK indexes for cascade performance
CREATE INDEX idx_tb_post_fk_user ON tb_post(fk_user);
CREATE INDEX idx_tb_post_fk_category ON tb_post(fk_category);

COMMENT ON TABLE tb_post IS 'Base table for posts (Trinity pattern)';
COMMENT ON COLUMN tb_post.pk_post IS 'Integer primary key (for internal DB operations)';
COMMENT ON COLUMN tb_post.id IS 'UUID identifier (for external API/GraphQL)';
COMMENT ON COLUMN tb_post.fk_user IS 'Foreign key to tb_user.pk_user (integer)';
```

**WHERE TO ADD:** Beginning of Task 1.1 as "Reference Pattern"

---

### 2. **MISSING: Complete Canonical TVIEW Example**

The roadmap never shows the COMPLETE SELECT with all rules applied.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- CANONICAL TVIEW PATTERN (tv_*)
-- ========================================
-- This is the EXACT pattern for ALL TVIEW examples

CREATE TABLE tv_post AS
SELECT
  -- 1. ALWAYS select pk_{entity} first (integer primary key)
  tb_post.pk_post,

  -- 2. ALWAYS select id second (UUID for GraphQL)
  tb_post.id,

  -- 3. ALWAYS select fk_* columns for cascade tracking (integers)
  tb_post.fk_user,
  tb_post.fk_category,

  -- 4. ALWAYS build JSONB data column last
  jsonb_build_object(
    -- Include UUID id for GraphQL queries
    'id', tb_post.id,

    -- Include business data
    'title', tb_post.title,
    'content', tb_post.content,
    'publishedAt', tb_post.published_at,  -- camelCase for JSON

    -- Include foreign UUIDs for GraphQL relations (from JOINs)
    'userId', tb_user.id,           -- UUID from joined table
    'categoryId', tb_category.id,   -- UUID from joined table (can be NULL)

    -- Include denormalized parent data
    'userName', tb_user.name,
    'categoryName', tb_category.name
  ) as data

FROM tb_post
-- Always use INNER/LEFT JOIN to get related UUIDs
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- Result columns in tv_post:
-- - pk_post (SERIAL) - for internal cascade operations
-- - id (UUID) - for GraphQL queries by UUID
-- - fk_user (INTEGER) - for cascade tracking
-- - fk_category (INTEGER) - for cascade tracking
-- - data (JSONB) - for GraphQL field resolution
```

**WHERE TO ADD:** Task 1.2 "Standardize TVIEW Creation Examples" - replace simplified examples

---

### 3. **MISSING: v_* Backing View Pattern**

The roadmap mentions "backing views" but NEVER shows what they look like!

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- CANONICAL BACKING VIEW PATTERN (v_*)
-- ========================================
-- pg_tviews creates this automatically, but here's what it looks like:

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
    'categoryName', tb_category.name
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- The v_* view is used by triggers to refresh tv_* data
-- When tb_post is updated, trigger does:
--   UPDATE tv_post SET data = v_post.data WHERE pk_post = NEW.pk_post;
```

**WHERE TO ADD:** Task 1.2 as new subsection "Understanding the Three Components"

---

### 4. **MISSING: Complete End-to-End Example**

No example shows the FULL workflow from CREATE TABLE to SELECT.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- COMPLETE END-TO-END EXAMPLE
-- ========================================

-- Step 1: Create base tables
CREATE TABLE tb_user (
  pk_user SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  name TEXT NOT NULL,
  email TEXT NOT NULL UNIQUE
);

CREATE TABLE tb_post (
  pk_post SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_user INTEGER NOT NULL REFERENCES tb_user(pk_user),
  title TEXT NOT NULL,
  content TEXT
);

-- Step 2: Insert sample data (using pk_* for FKs)
INSERT INTO tb_user (name, email) VALUES
  ('Alice', 'alice@example.com'),
  ('Bob', 'bob@example.com');

INSERT INTO tb_post (fk_user, title, content) VALUES
  (1, 'First Post', 'Hello World'),  -- fk_user=1 is Alice's pk_user
  (2, 'Second Post', 'From Bob');    -- fk_user=2 is Bob's pk_user

-- Step 3: Create TVIEW
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
    'userName', tb_user.name
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

-- Step 4: Query by integer PK (internal operations)
SELECT * FROM tv_post WHERE tv_post.pk_post = 1;

-- Step 5: Query by UUID (GraphQL pattern)
SELECT tv_post.data
FROM tv_post
WHERE tv_post.id = '<uuid-from-step-4>';

-- Step 6: Update base table (triggers auto-update TVIEW)
UPDATE tb_post
SET title = 'Updated Title'
WHERE tb_post.pk_post = 1;

-- Step 7: Verify cascade worked
SELECT tv_post.data->>'title' FROM tv_post WHERE tv_post.pk_post = 1;
-- Should return 'Updated Title'

-- Step 8: Update parent (cascade to child TVIEW)
UPDATE tb_user
SET name = 'Alice Smith'
WHERE tb_user.pk_user = 1;

-- Step 9: Verify parent cascade
SELECT tv_post.data->>'userName' FROM tv_post WHERE tv_post.fk_user = 1;
-- Should return 'Alice Smith' (cascaded from tb_user update)
```

**WHERE TO ADD:** New section at beginning: "Task 0.0: Reference Implementation"

---

### 5. **IMPLICIT: JSONB Key Naming Convention**

Examples inconsistently use camelCase vs snake_case.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- JSONB KEY NAMING RULES
-- ========================================

-- ✅ CORRECT: Use camelCase for JSONB keys (GraphQL convention)
jsonb_build_object(
  'id', tb_post.id,              -- lowercase for single word
  'userId', tb_user.id,          -- camelCase for compound
  'publishedAt', tb_post.published_at,
  'categoryId', tb_post.fk_category
)

-- ❌ WRONG: Don't use snake_case in JSONB
jsonb_build_object(
  'user_id', tb_user.id,         -- NO
  'published_at', tb_post.published_at  -- NO
)

-- ❌ WRONG: Don't include pk_* or fk_* in JSONB (use UUIDs instead)
jsonb_build_object(
  'pk_post', tb_post.pk_post,    -- NO - internal PK
  'fk_user', tb_post.fk_user     -- NO - use 'userId' with UUID
)

-- RULE: JSONB is for external API (GraphQL) - use camelCase and UUIDs
-- RULE: Table columns use snake_case and integers for pk_*/fk_*
```

**WHERE TO ADD:** Task 1.2 as "JSONB Naming Conventions"

---

### 6. **MISSING: How to Handle NULL Foreign Keys**

Examples don't show NULL FK handling explicitly.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- HANDLING NULL FOREIGN KEYS
-- ========================================

-- Scenario: Post with optional category (fk_category can be NULL)

CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  tb_post.fk_category,  -- Can be NULL
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,

    -- For required FK (INNER JOIN): Always non-null
    'userId', tb_user.id,
    'userName', tb_user.name,

    -- For optional FK (LEFT JOIN): Use COALESCE or handle NULL
    'categoryId', tb_category.id,  -- NULL if no category
    'categoryName', COALESCE(tb_category.name, 'Uncategorized')
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
LEFT JOIN tb_category ON tb_post.fk_category = tb_category.pk_category;

-- Querying posts without category:
SELECT * FROM tv_post WHERE tv_post.fk_category IS NULL;

-- JSON behavior with NULL:
SELECT tv_post.data->'categoryId' FROM tv_post;
-- Returns: null (JSON null, not SQL NULL)
```

**WHERE TO ADD:** Task 1.2 or new "Common Patterns" section

---

### 7. **MISSING: Multi-Level Cascade Example**

No complete 3-level cascade example showing FK propagation.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- MULTI-LEVEL CASCADE EXAMPLE
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

-- Level 3: Comments (depends on Post, which depends on User)
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
    'authorId', tb_user.id,    -- Denormalized from grandparent
    'authorName', tb_user.name
  ) as data
FROM tb_comment
INNER JOIN tb_post ON tb_comment.fk_post = tb_post.pk_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;

-- Test cascade:
-- 1. Update user name
UPDATE tb_user SET name = 'Alice Smith' WHERE pk_user = 1;

-- 2. Verify cascade to posts
SELECT data->>'userName' FROM tv_post WHERE fk_user = 1;
-- Should show 'Alice Smith'

-- 3. Verify cascade to comments (2 levels deep!)
SELECT data->>'authorName' FROM tv_comment
WHERE fk_post IN (SELECT pk_post FROM tb_post WHERE fk_user = 1);
-- Should show 'Alice Smith' (cascaded through 2 levels)
```

**WHERE TO ADD:** New Task 1.2.1 "Multi-Level Cascade Patterns"

---

### 8. **MISSING: INSERT/UPDATE/DELETE Patterns**

Examples only show CREATE and SELECT, never modification patterns.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- DATA MODIFICATION PATTERNS
-- ========================================

-- INSERT Pattern: Always insert into tb_*, never tv_*
-- (TVIEWs auto-populate via triggers)

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

-- UPDATE Pattern: Update tb_*, TVIEW cascades automatically

-- ✅ CORRECT: Update by integer PK
UPDATE tb_post
SET title = 'Updated'
WHERE tb_post.pk_post = 1;
-- tv_post automatically updated via trigger

-- ✅ CORRECT: Update by UUID (if you have it)
UPDATE tb_post
SET title = 'Updated'
WHERE tb_post.id = 'uuid-here';
-- tv_post automatically updated

-- ❌ WRONG: Don't update TVIEW directly
UPDATE tv_post SET data = '{"title": "New"}' WHERE pk_post = 1;  -- DON'T

-- DELETE Pattern: Delete from tb_*, TVIEW cascades automatically

-- ✅ CORRECT: Delete by integer PK
DELETE FROM tb_post WHERE tb_post.pk_post = 1;
-- tv_post row automatically deleted via CASCADE

-- ✅ CORRECT: Delete by UUID
DELETE FROM tb_post WHERE tb_post.id = 'uuid-here';

-- ❌ WRONG: Don't delete from TVIEW
DELETE FROM tv_post WHERE pk_post = 1;  -- DON'T

-- FINDING PK FROM UUID Pattern:

-- Get integer pk_post from UUID for operations
SELECT tb_post.pk_post, tb_post.id
FROM tb_post
WHERE tb_post.id = 'uuid-from-graphql';

-- Use in subquery for operations
DELETE FROM tb_post
WHERE tb_post.pk_post = (
  SELECT tb_post.pk_post FROM tb_post WHERE tb_post.id = 'uuid-here'
);
```

**WHERE TO ADD:** New section "Task 1.2.2: Data Modification Patterns"

---

### 9. **MISSING: Metadata Table Structure**

The roadmap references `pg_tview_meta` but never shows its structure!

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- METADATA TABLE STRUCTURE
-- ========================================

-- pg_tview_meta stores TVIEW definitions
CREATE TABLE pg_tview_meta (
  entity TEXT PRIMARY KEY,              -- 'post', 'user', etc. (singular)
  definition TEXT NOT NULL,             -- Original SELECT query
  dependencies OID[] NOT NULL,          -- Array of table OIDs this TVIEW depends on
  dependency_paths JSONB,               -- Cascade path information
  created_at TIMESTAMPTZ DEFAULT NOW(),
  updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Example row for tv_post:
INSERT INTO pg_tview_meta (entity, definition, dependencies) VALUES (
  'post',
  'SELECT tb_post.pk_post, tb_post.id, jsonb_build_object(...) FROM tb_post JOIN tb_user ...',
  ARRAY['tb_post'::regclass::oid, 'tb_user'::regclass::oid]
);

-- Querying metadata:
SELECT
  pg_tview_meta.entity,
  array_length(pg_tview_meta.dependencies, 1) as dependency_count
FROM pg_tview_meta
ORDER BY pg_tview_meta.entity;

-- Finding what TVIEWs depend on a table:
SELECT pg_tview_meta.entity
FROM pg_tview_meta
WHERE 'tb_user'::regclass::oid = ANY(pg_tview_meta.dependencies);
```

**WHERE TO ADD:** Phase 3, Task 3.1 "Complete Monitoring Infrastructure"

---

### 10. **MISSING: Error Messages and Troubleshooting**

No examples of WHAT errors mean or HOW to interpret them.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- COMMON ERRORS AND SOLUTIONS
-- ========================================

-- ERROR: duplicate key value violates unique constraint "tv_post_pkey"
-- CAUSE: Trying to insert duplicate pk_post
-- SOLUTION: Don't insert into tv_* directly, insert into tb_*

-- ERROR: column "pk_post" does not exist
-- CAUSE: Missing column in SELECT or using wrong table alias
-- SOLUTION: Ensure SELECT includes tb_post.pk_post

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

**WHERE TO ADD:** New section in Phase 1: "Task 1.3: Common Errors Reference"

---

### 11. **IMPLICIT: When to Use INTEGER vs UUID**

Never explicitly stated when to use pk_* vs id.

**NEEDS TO BE ADDED:**
```markdown
## When to Use INTEGER (pk_/fk_) vs UUID (id)

### Use INTEGER (pk_*, fk_*):
- Internal database operations (JOINs, foreign keys, indexes)
- Cascade update tracking (triggers use pk_* to match rows)
- WHERE clauses for direct table access
- All foreign key columns (fk_user, fk_post, etc.)

Example:
UPDATE tv_post SET data = v_post.data WHERE tv_post.pk_post = NEW.pk_post;

### Use UUID (id):
- External API responses (GraphQL, REST)
- Public-facing identifiers (URLs, client references)
- JSONB data content (for GraphQL resolvers)
- Cross-system references (if syncing between databases)

Example:
SELECT tv_post.data FROM tv_post WHERE tv_post.id = $graphql_id;

### Rule of Thumb:
- Database ↔ Database: Use pk_*/fk_* (integers)
- Database → Client (GraphQL/API): Use id (UUID)
- Client → Database (GraphQL query): Convert UUID to pk_* in resolver
```

**WHERE TO ADD:** Beginning of document as "Trinity Pattern Principles"

---

### 12. **MISSING: Index Creation Checklist**

Examples mention indexes but no systematic checklist.

**NEEDS TO BE ADDED:**
```sql
-- ========================================
-- MANDATORY INDEX CHECKLIST FOR TVIEWS
-- ========================================

-- For EVERY TVIEW, create these indexes:

-- 1. UUID index (for GraphQL queries by id)
CREATE INDEX idx_tv_{entity}_id ON tv_{entity}(id);

-- 2. Foreign key indexes (for cascade performance)
CREATE INDEX idx_tv_{entity}_fk_{parent} ON tv_{entity}(fk_{parent});
-- Repeat for EACH fk_* column

-- 3. Primary key index (automatic via PRIMARY KEY constraint)
-- Already exists as tv_{entity}_pkey

-- Example for tv_post (with fk_user and fk_category):
CREATE INDEX idx_tv_post_id ON tv_post(id);
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_post_fk_category ON tv_post(fk_category);

-- 4. Optional: JSONB GIN index (for JSONB queries)
-- Only needed if querying JSONB fields directly
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);

-- 5. Optional: Composite indexes (based on query patterns)
-- Only if you frequently query by fk_* + another field
CREATE INDEX idx_tv_post_user_created
  ON tv_post(fk_user, (data->>'createdAt'));
```

**WHERE TO ADD:** Phase 4, Task 4.1 at the very beginning

---

## Summary: What to Add to EXCELLENCE_ROADMAP.md

### High Priority (Blockers for implementation):
1. ✅ **Task 0.0: Complete Reference Pattern** - Add canonical tb_*/tv_*/v_* examples
2. ✅ **Task 1.2.0: End-to-End Example** - Full workflow from CREATE to SELECT
3. ✅ **Task 1.2.1: JSONB Naming Rules** - Explicit camelCase convention
4. ✅ **Task 1.2.2: Data Modification Patterns** - INSERT/UPDATE/DELETE rules
5. ✅ **Task 1.3: Common Errors Reference** - Error messages and solutions

### Medium Priority (Avoid confusion):
6. ✅ **Task 1.2.3: NULL Foreign Key Handling** - LEFT JOIN patterns
7. ✅ **Task 1.2.4: Multi-Level Cascade** - 3+ level dependency example
8. ✅ **Task 3.1.1: Metadata Table Structure** - pg_tview_meta schema
9. ✅ **Principles Section: INTEGER vs UUID** - When to use what

### Low Priority (Nice to have):
10. ✅ **Task 4.1.0: Mandatory Index Checklist** - Systematic index creation
11. ✅ **Task 1.2.5: Backing View (v_*) Pattern** - Show what v_* looks like

---

## Recommendation

Add a **new section at the very beginning** of EXCELLENCE_ROADMAP.md:

```markdown
## Trinity Pattern Reference (MUST READ FIRST)

Before implementing ANY task in this roadmap, read this section to understand
the complete trinity pattern. All examples below assume this pattern.

### Complete Reference Implementation
[Include all 12 examples above]
```

This will make the roadmap **self-contained** and **unambiguous** for any implementer.
