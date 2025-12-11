# Phase 1: Documentation Excellence

**Goal**: 85/100 → 95/100
**Effort**: 20-30 hours
**Priority**: High

> **⚠️ IMPORTANT**: All SQL examples in this phase MUST follow the trinity pattern.
> **See**: [00-TRINITY-PATTERN-REFERENCE.md](./00-TRINITY-PATTERN-REFERENCE.md) for complete patterns.
>
> **Quick Reminder**:
> - ✅ Singular names: `tb_post`, `tv_post`, `v_post` (NOT `tb_posts`)
> - ✅ Qualified columns: `tb_post.id` (NOT just `id`)
> - ✅ pk_*/fk_* are INTEGER, id is UUID
> - ✅ JSONB uses camelCase: `'userId'` (NOT `'user_id'`)

---

## Objectives

1. Fix all SQL examples with unqualified column references
2. Create comprehensive migration guides
3. Standardize example formatting
4. Add security documentation
5. Improve API reference completeness

---

## Task Breakdown

### Task 1.1: Fix Unqualified Column References (P1)
**Effort**: 3-4 hours
**Files**: 34 instances across documentation

**Search Pattern**:
```bash
grep -r "SELECT.*as pk_" docs/ README.md .phases/ \
  | grep -v "tb_\." \
  | grep "SELECT.*id as pk_"
```

**Fix Strategy** (Trinity Pattern):
```sql
# BEFORE (incorrect)
SELECT id as pk_post,
       jsonb_build_object('id', id, 'title', title) as data
FROM tb_post;

# AFTER (correct - trinity pattern)
SELECT tb_post.pk_post,
       tb_post.id,
       jsonb_build_object(
         'id', tb_post.id,
         'title', tb_post.title
       ) as data
FROM tb_post;

-- Trinity Pattern Rules:
-- - pk_post is SERIAL (integer) for internal DB operations
-- - id is UUID for external API/GraphQL
-- - Always qualify all columns with table name (tb_post.column)
-- - JSONB keys use camelCase
```

**Files to Update**:
- `docs/operations/troubleshooting.md` (6 instances)
- `docs/operations/debugging.md` (2 instances)
- `docs/reference/ddl.md` (7 instances)
- `docs/error-reference.md` (9 instances)
- `.phases/event-triggers-implementation-plan.md` (4 instances)
- `.phases/fix-process-utility-hook-*.md` (6 instances)

**Verification**:
```bash
# After fixes, this should return 0 unqualified references
grep -r "jsonb_build_object.*'id', id[^_a-z]" docs/ README.md
```

**Acceptance Criteria**:
- [ ] All SELECT examples use qualified column names
- [ ] All jsonb_build_object calls use table.column syntax
- [ ] Add note in docs/style-guide.md about column qualification
- [ ] No grep matches for unqualified patterns

---

### Task 1.2: Standardize TVIEW Creation Examples
**Effort**: 2-3 hours

**Problem**: Three different syntaxes shown without explanation

**Solution**: Create unified examples guide showing all three methods

**New File**: `docs/getting-started/syntax-comparison.md`

**Content** (All examples must follow trinity pattern):

```markdown
# TVIEW Creation Syntax Guide

> **Trinity Pattern Reference**: See complete patterns in [00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

pg_tviews supports three equivalent ways to create TVIEWs:

## 1. DDL Syntax (Recommended for Interactive Use)
```sql
CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,          -- INTEGER primary key
  tb_post.id,               -- UUID for GraphQL
  tb_post.fk_user,          -- INTEGER foreign key
  jsonb_build_object(
    'id', tb_post.id,       -- UUID in camelCase
    'title', tb_post.title,
    'userId', tb_user.id    -- Related UUID (from JOIN)
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
```
**Pros**: Natural SQL syntax, familiar to DBAs
**Cons**: Less explicit about TVIEW creation
**Use when**: Working in psql, migrations, manual DDL

## 2. CREATE TVIEW Syntax (Explicit)
```sql
CREATE TVIEW tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  tb_post.fk_user,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'userId', tb_user.id
  ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
```
**Pros**: Clear intent, extension-specific
**Cons**: Requires hook support
**Use when**: Code clarity is important

## 3. Function Syntax (Programmatic)
```sql
SELECT pg_tviews_create('post', $$
  SELECT
    tb_post.pk_post,
    tb_post.id,
    tb_post.fk_user,
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title,
      'userId', tb_user.id
    ) as data
  FROM tb_post
  INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user
$$);
```
**Pros**: Best for dynamic SQL, scripting
**Cons**: More verbose
**Use when**: Application code, scripts, dynamic creation

**Trinity Pattern Notes**:
- `pk_post` and `fk_user` are integers (SERIAL/INTEGER)
- `id` is UUID (for external API)
- All columns qualified with table name
- JSONB keys use camelCase

All three methods produce identical results.
```

**Acceptance Criteria**:
- [ ] New syntax comparison guide created
- [ ] README.md links to syntax guide
- [ ] Quickstart mentions all three approaches
- [ ] Each example follows trinity pattern exactly
- [ ] Each example includes trinity pattern notes

---

### Task 1.3: Create Migration & Upgrade Guide
**Effort**: 4-6 hours

**New File**: `docs/operations/upgrades.md`

**Contents**: Standard upgrade/rollback procedures (no trinity pattern needed here)

**Acceptance Criteria**:
- [ ] Upgrade guide created
- [ ] SQL upgrade scripts in place
- [ ] Version compatibility matrix documented
- [ ] Rollback procedures tested

---

### Task 1.4: Security Documentation
**Effort**: 3-4 hours

**New File**: `docs/operations/security.md`

**Key Sections** (with trinity pattern examples):

```markdown
# Security Guide

> **Trinity Pattern Reference**: All examples follow the pattern from [00-TRINITY-PATTERN-REFERENCE.md](../../.phases/excellence-roadmap/00-TRINITY-PATTERN-REFERENCE.md)

## SQL Injection Prevention

```sql
-- ✅ SAFE: Function parameters are escaped
SELECT pg_tviews_create('my_entity', $$
  SELECT
    tb_my_entity.pk_my_entity,  -- INTEGER pk
    tb_my_entity.id,             -- UUID
    jsonb_build_object(
      'id', tb_my_entity.id
    ) as data
  FROM tb_my_entity
$$);

-- ❌ UNSAFE: Never concatenate user input
SELECT pg_tviews_create(user_provided_name, user_provided_sql);
```

## Column-Level Security

```sql
-- ❌ BAD: Including sensitive data
CREATE TVIEW tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'password_hash', tb_user.password_hash  -- Don't expose!
  ) as data
FROM tb_user;

-- ✅ GOOD: Exclude sensitive columns
CREATE TVIEW tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'username', tb_user.username,
    'email', tb_user.email
  ) as data
FROM tb_user;
```
```

**Acceptance Criteria**:
- [ ] Security guide created
- [ ] All examples follow trinity pattern
- [ ] GRANT/REVOKE patterns documented
- [ ] SQL injection risks explained
- [ ] RLS examples provided

---

### Task 1.5: API Reference Completeness
**Effort**: 4-5 hours

**Goal**: Document ALL public functions with examples

**Functions to Document**:
- [ ] `pg_tviews_drop()`
- [ ] `pg_tviews_cascade()`
- [ ] `pg_tviews_convert_table()`
- [ ] `pg_tviews_install_stmt_triggers()`
- [ ] `pg_tviews_health_check()` (when implemented)
- [ ] `pg_tviews_queue_realtime` (view)
- [ ] `pg_tviews_cache_stats` (view)

**Acceptance Criteria**:
- [ ] All public functions documented
- [ ] Each function has 2+ examples following trinity pattern
- [ ] Error conditions listed
- [ ] Performance notes included
- [ ] Cross-references to related functions

---

### Task 1.6: Add Troubleshooting Flowcharts
**Effort**: 2-3 hours

**Update**: `docs/operations/troubleshooting.md`

**Add Mermaid Flowcharts** (visual decision trees for common issues)

**Acceptance Criteria**:
- [ ] Decision trees for common issues
- [ ] Visual flowcharts using Mermaid
- [ ] Step-by-step debugging procedures
- [ ] Links to trinity pattern reference where SQL examples appear

---

## Phase 1 Acceptance Criteria

- [ ] All SQL examples use qualified column names (0 violations)
- [ ] All examples follow trinity pattern (singular names, INTEGER pk/fk, UUID id)
- [ ] Syntax comparison guide created and linked
- [ ] Upgrade guide with rollback procedures documented
- [ ] Security guide covers access control, SQL injection, RLS
- [ ] All public functions documented with examples
- [ ] Troubleshooting flowcharts added
- [ ] Documentation score: **95/100 ✅**

---

## Verification Commands

```bash
# 1. Check for unqualified columns
grep -r "SELECT.*'id', id[^_a-z]" docs/ README.md
# Should return 0 results

# 2. Check for plural table names
grep -r "tb_[a-z]*s\|tv_[a-z]*s\|v_[a-z]*s" docs/ README.md .phases/
# Should return 0 results (except in explanatory text)

# 3. Check for snake_case in JSONB
grep -r "jsonb_build_object.*'[a-z_]*_[a-z_]*'" docs/ README.md
# Should return 0 results (camelCase only)

# 4. Verify all files updated
git diff --stat docs/ README.md .phases/
```

---

**Next Phase**: [02-testing-quality.md](./02-testing-quality.md)
