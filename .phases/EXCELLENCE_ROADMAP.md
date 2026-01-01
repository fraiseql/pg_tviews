# pg_tviews Excellence Roadmap
# Bringing All Quality Metrics to 95-100/100

**Current Overall Score**: 87/100
**Target Overall Score**: 95+/100
**Timeline**: 4-6 weeks (80-120 hours)
**Status**: Planning Phase

---

## Executive Summary

This roadmap addresses all identified gaps from the QA Assessment to achieve excellence across all quality categories. The plan is divided into 4 major phases, each targeting specific quality improvements.

### Current vs. Target Scores

| Category | Current | Target | Gap | Priority |
|----------|---------|--------|-----|----------|
| Code Correctness | 92/100 | 98/100 | +6 | Medium |
| Architecture | 90/100 | 96/100 | +6 | Medium |
| Documentation | 85/100 | 95/100 | +10 | **High** |
| Testing | 82/100 | 95/100 | +13 | **High** |
| Performance | 88/100 | 95/100 | +7 | Medium |
| Production Ready | 84/100 | 98/100 | +14 | **High** |

### Effort Allocation

- **Phase 1**: Documentation Excellence (20-30 hours)
- **Phase 2**: Testing & Quality Assurance (25-35 hours)
- **Phase 3**: Production Readiness (20-30 hours)
- **Phase 4**: Performance & Optimization (15-25 hours)

---

## Phase 1: Documentation Excellence
**Goal**: 85/100 → 95/100
**Effort**: 20-30 hours
**Priority**: High

### Objectives

1. Fix all SQL examples with unqualified column references
2. Create comprehensive migration guides
3. Standardize example formatting
4. Add security documentation
5. Improve API reference completeness

### Task Breakdown

#### Task 1.1: Fix Unqualified Column References (P1)
**Effort**: 3-4 hours
**Files**: 34 instances across documentation

**Search Pattern**:
```bash
grep -r "SELECT.*as pk_" docs/ README.md .phases/ \
  | grep -v "tb_\." \
  | grep "SELECT.*id as pk_"
```

**Fix Strategy**:
```sql
# BEFORE (incorrect)
SELECT id as pk_post,
       jsonb_build_object('id', id, 'title', title) as data
FROM tb_post;

# AFTER (correct)
SELECT tb_post.pk_post,
       tb_post.id,
       jsonb_build_object(
         'id', tb_post.id,
         'title', tb_post.title
       ) as data
FROM tb_post;

-- Note: pk_post is SERIAL (integer), id is UUID
-- Always qualify all columns with table name
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

#### Task 1.2: Standardize TVIEW Creation Examples
**Effort**: 2-3 hours

**Problem**: Three different syntaxes shown without explanation
```sql
-- Method 1: DDL syntax (hook intercepts)
CREATE TABLE tv_post AS SELECT ...;

-- Method 2: CREATE TVIEW syntax (extension)
CREATE TABLE tv_post AS SELECT ...;

-- Method 3: Function syntax
SELECT pg_tviews_create('post', 'SELECT ...');
```

**Solution**: Create unified examples guide

**New File**: `docs/getting-started/syntax-comparison.md`

```markdown
# TVIEW Creation Syntax Guide

pg_tviews supports three equivalent ways to create TVIEWs:

## 1. DDL Syntax (Recommended for Interactive Use)
```sql
CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'userId', tb_post.fk_user
  ) as data
FROM tb_post;
```
**Pros**: Natural SQL syntax, familiar to DBAs
**Cons**: Less explicit about TVIEW creation
**Use when**: Working in psql, migrations, manual DDL

## 2. CREATE TVIEW Syntax (Explicit)
```sql
CREATE TABLE tv_post AS
SELECT
  tb_post.pk_post,
  tb_post.id,
  jsonb_build_object(
    'id', tb_post.id,
    'title', tb_post.title,
    'userId', tb_post.fk_user
  ) as data
FROM tb_post;
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
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title,
      'userId', tb_post.fk_user
    ) as data
  FROM tb_post
$$);
```
**Pros**: Best for dynamic SQL, scripting
**Cons**: More verbose
**Use when**: Application code, scripts, dynamic creation

**Note**: `pk_post` and `fk_user` are integers (SERIAL). `id` is UUID.

All three methods produce identical results.
```

**Acceptance Criteria**:
- [ ] New syntax comparison guide created
- [ ] README.md links to syntax guide
- [ ] Quickstart mentions all three approaches
- [ ] Each example includes comment noting equivalence

---

#### Task 1.3: Create Migration & Upgrade Guide
**Effort**: 4-6 hours

**New File**: `docs/operations/upgrades.md`

**Contents**:
```markdown
# Upgrade Guide

## Version Compatibility Matrix

| From Version | To Version | Breaking Changes | Migration Required |
|--------------|------------|------------------|--------------------|
| 0.1.0-beta.1 | 0.2.0 | None | No |
| 0.2.0 | 1.0.0 | TBD | TBD |

## Pre-Upgrade Checklist

- [ ] Backup pg_tview_meta table
- [ ] Backup all tv_* tables (optional, can be recreated)
- [ ] Note PostgreSQL version
- [ ] Check extension dependencies (jsonb_delta)
- [ ] Review changelog for breaking changes

## Upgrade Procedure

### Standard Upgrade (No Breaking Changes)
```sql
BEGIN;
  -- Backup metadata
  CREATE TABLE pg_tview_meta_backup AS SELECT * FROM pg_tview_meta;

  -- Upgrade extension
  ALTER EXTENSION pg_tviews UPDATE TO '0.2.0';

  -- Verify version
  SELECT pg_tviews_version();
COMMIT;
```

### Rollback Procedure
```sql
BEGIN;
  -- Restore metadata if needed
  DROP TABLE pg_tview_meta;
  CREATE TABLE pg_tview_meta AS SELECT * FROM pg_tview_meta_backup;

  -- Downgrade extension
  ALTER EXTENSION pg_tviews UPDATE TO '0.1.0-beta.1';
COMMIT;
```

## Breaking Changes Log

### Version 1.0.0 (Planned)
- TBD

### Version 0.2.0 (Planned)
- None expected
```

**New File**: `sql/pg_tviews--0.1.0-beta.1--0.2.0.sql`
```sql
-- Upgrade script from 0.1.0-beta.1 to 0.2.0
-- Currently empty (no schema changes yet)
-- Add ALTER TABLE statements here when schema evolves
```

**Acceptance Criteria**:
- [ ] Upgrade guide created
- [ ] SQL upgrade scripts in place
- [ ] Version compatibility matrix documented
- [ ] Rollback procedures tested

---

#### Task 1.4: Security Documentation
**Effort**: 3-4 hours

**New File**: `docs/operations/security.md`

**Contents**:
```markdown
# Security Guide

## Access Control

### Extension Installation
```sql
-- Only superusers can install extensions
CREATE EXTENSION pg_tviews;  -- Requires superuser
```

### TVIEW Creation Permissions
```sql
-- Grant TVIEW creation to specific users
GRANT CREATE ON SCHEMA public TO app_user;
GRANT EXECUTE ON FUNCTION pg_tviews_create(text, text) TO app_user;
```

### Read-Only Access
```sql
-- Grant read-only access to TVIEWs
GRANT SELECT ON ALL TABLES IN SCHEMA public TO readonly_user;
-- Future TVIEWs
ALTER DEFAULT PRIVILEGES IN SCHEMA public
  GRANT SELECT ON TABLES TO readonly_user;
```

## SQL Injection Prevention

pg_tviews uses **parameterized queries** and **proper escaping** internally:

```sql
-- SAFE: Function parameters are escaped
SELECT pg_tviews_create('my_entity', $$
  SELECT
    tb_my_entity.pk_my_entity,
    tb_my_entity.id,
    tb_my_entity.data
  FROM tb_my_entity
$$);

-- UNSAFE: Never concatenate user input into SQL
-- BAD EXAMPLE (don't do this):
SELECT pg_tviews_create(user_provided_name, user_provided_sql);
```

**Best Practices**:
1. ✅ Use prepared statements in application code
2. ✅ Validate entity names before TVIEW creation
3. ✅ Whitelist allowed characters in entity names (alphanumeric + underscore)
4. ❌ Never trust user input for SQL construction

## Data Protection

### Row-Level Security (RLS)
```sql
-- Enable RLS on base tables (tv_* inherits)
ALTER TABLE tb_post ENABLE ROW LEVEL SECURITY;

CREATE POLICY post_owner_policy ON tb_post
  FOR ALL
  TO app_user
  USING (user_id = current_user_id());
```

### Column-Level Security
```sql
-- Sensitive columns should not be in JSONB data
-- BAD: Including sensitive data
CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'password_hash', tb_user.password_hash  -- ❌ Don't expose!
  ) as data
FROM tb_user;

-- GOOD: Exclude sensitive columns
CREATE TABLE tv_user AS
SELECT
  tb_user.pk_user,
  tb_user.id,
  jsonb_build_object(
    'id', tb_user.id,
    'username', tb_user.username,
    'email', tb_user.email  -- OK for display
  ) as data
FROM tb_user;

-- Note: pk_user is SERIAL (integer), id is UUID
```

## Audit Logging

```sql
-- Track TVIEW creation/modification
CREATE TABLE pg_tview_audit_log (
  operation TEXT NOT NULL,
  entity TEXT NOT NULL,
  performed_by TEXT NOT NULL DEFAULT current_user,
  performed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  details JSONB
);

-- Trigger on pg_tview_meta changes
-- (Implementation TBD)
```

## Known Security Considerations

1. **Extension Trust**: pg_tviews is a trusted extension (runs with elevated privileges)
2. **Trigger Safety**: Triggers run in SECURITY DEFINER context
3. **Catalog Access**: Requires read access to PostgreSQL catalogs
4. **SPI Usage**: Event triggers use SPI in safe contexts only

## Security Checklist

- [ ] Only trusted users can create TVIEWs
- [ ] Sensitive data excluded from JSONB `data` columns
- [ ] Row-level security configured on base tables
- [ ] Audit logging enabled (if required)
- [ ] Regular security updates applied
- [ ] Extension code reviewed for SQL injection risks
```

**Acceptance Criteria**:
- [ ] Security guide created
- [ ] GRANT/REVOKE patterns documented
- [ ] SQL injection risks explained
- [ ] RLS examples provided
- [ ] Audit logging strategy outlined

---

#### Task 1.5: API Reference Completeness
**Effort**: 4-5 hours

**Goal**: Document ALL public functions with examples

**Template** (save as `docs/function-template.md`):
```markdown
### function_name()

**Signature**:
```sql
function_name(param1 TYPE, param2 TYPE) RETURNS RETURN_TYPE
```

**Description**:
Brief description of what the function does (1-2 sentences).

**Parameters**:
- `param1` (TYPE): Description of parameter 1
- `param2` (TYPE): Description of parameter 2

**Returns**:
- `RETURN_TYPE`: Description of return value

**Example**:
```sql
-- Basic usage
SELECT function_name('value1', 'value2');
-- Returns: result

-- Advanced usage
SELECT function_name(...);
```

**Error Conditions**:
- `ERROR_CODE`: When this happens
- `ERROR_CODE`: When that happens

**Notes**:
- Additional information
- Performance considerations
- Related functions

**See Also**:
- [Related Function](#related-function)
```

**Functions to Document** (check `docs/reference/api.md` and add missing):
- [x] `pg_tviews_version()`
- [x] `pg_tviews_check_jsonb_delta()`
- [x] `pg_tviews_create()`
- [ ] `pg_tviews_drop()`
- [ ] `pg_tviews_cascade()`
- [ ] `pg_tviews_convert_table()`
- [ ] `pg_tviews_install_stmt_triggers()`
- [ ] `pg_tviews_health_check()` (when implemented)
- [ ] `pg_tviews_queue_realtime` (view)
- [ ] `pg_tviews_cache_stats` (view)

**Acceptance Criteria**:
- [ ] All public functions documented using template
- [ ] Each function has at least 2 examples
- [ ] Error conditions listed
- [ ] Performance notes included where relevant
- [ ] Cross-references to related functions

---

#### Task 1.6: Add Troubleshooting Flowcharts
**Effort**: 2-3 hours

**Update**: `docs/operations/troubleshooting.md`

**Add Mermaid Flowcharts**:
```markdown
## Troubleshooting Decision Tree

### TVIEW Not Updating Automatically

```mermaid
flowchart TD
    A[TVIEW not updating?] --> B{Triggers installed?}
    B -->|No| C[Run: SELECT * FROM pg_trigger WHERE tgname LIKE 'tview%']
    C --> D[Recreate TVIEW]
    B -->|Yes| E{Transaction committed?}
    E -->|No| F[Run COMMIT]
    E -->|Yes| G{Check queue}
    G --> H[SELECT * FROM pg_tviews_queue_realtime]
    H --> I{Queue empty?}
    I -->|Yes| J[Check base table PK]
    I -->|No| K[Queue processing failed - check logs]
    J --> L[Verify pk_* column matches base table PK]
```

### Performance Issues

```mermaid
flowchart TD
    A[Slow cascade updates?] --> B{jsonb_delta installed?}
    B -->|No| C[Install jsonb_delta for 1.5-3× speedup]
    B -->|Yes| D{Indexes created?}
    D -->|No| E[Create indexes on pk_*, fk_* columns]
    D -->|Yes| F{Large cascades?>100 rows?}
    F -->|Yes| G[Enable statement-level triggers]
    F -->|No| H[Check dependency depth]
    H --> I[SELECT * FROM pg_tview_meta WHERE entity = 'X']
```
```

**Acceptance Criteria**:
- [ ] Decision trees for common issues
- [ ] Visual flowcharts using Mermaid
- [ ] Step-by-step debugging procedures
- [ ] Links to relevant documentation sections

---

### Phase 1 Acceptance Criteria

- [ ] All SQL examples use qualified column names (0 violations)
- [ ] Syntax comparison guide created and linked
- [ ] Upgrade guide with rollback procedures documented
- [ ] Security guide covers access control, SQL injection, RLS
- [ ] All public functions documented with examples
- [ ] Troubleshooting flowcharts added
- [ ] Documentation score: 95/100 ✅

---

## Phase 2: Testing & Quality Assurance
**Goal**: 82/100 → 95/100
**Effort**: 25-35 hours
**Priority**: High

### Objectives

1. Fix test build with --no-default-features
2. Add concurrent DDL tests
3. Implement large-scale stress tests (1M+ rows)
4. Add integration tests for edge cases
5. Improve test assertions and validation
6. Add test coverage reporting

### Task Breakdown

#### Task 2.1: Fix Test Build Issue (P1)
**Effort**: 1-2 hours

**Problem**:
```bash
cargo pgrx test pg17 --no-default-features
# ERROR: #[pg_test] macro fails without pg_test feature
```

**Root Cause**: Test functions in `src/refresh/main.rs` use `#[pg_test]` unconditionally

**Fix**: Make test code conditional on feature flag

**File**: `src/refresh/main.rs`

```rust
// BEFORE (broken)
#[pg_test]
fn test_refresh_single_row() {
    // ...
}

// AFTER (fixed)
#[cfg(any(test, feature = "pg_test"))]
#[pg_test]
fn test_refresh_single_row() {
    // ...
}
```

**Files to Update**:
- `src/refresh/main.rs` (6 test functions)
- Search for all `#[pg_test]` in src/: `grep -r "#\[pg_test\]" src/`

**Verification**:
```bash
# Should compile without errors
cargo pgrx test pg17 --no-default-features

# Should also work with features
cargo pgrx test pg17
```

**Acceptance Criteria**:
- [ ] Tests compile with --no-default-features
- [ ] Tests run successfully with default features
- [ ] CI/CD can use either configuration
- [ ] Add note in DEVELOPMENT.md about test features

---

#### Task 2.2: Concurrent DDL Tests (P2)
**Effort**: 6-8 hours

**New File**: `test/sql/70_concurrent_ddl.sql`

**Test Scenarios**:

```sql
-- Test 1: Concurrent TVIEW creation (should not deadlock)
-- Requires pg_regress with multiple sessions
-- Or: Use dblink for multi-connection tests

-- Setup
CREATE EXTENSION IF NOT EXISTS dblink;

-- Session 1: Create tv_post
SELECT dblink_connect('conn1', 'dbname=postgres');
SELECT dblink_send_query('conn1', $$
  CREATE TABLE tv_post AS
  SELECT
    tb_post.pk_post,
    tb_post.id,
    jsonb_build_object(
      'id', tb_post.id,
      'title', tb_post.title
    ) as data
  FROM tb_post;
$$);

-- Session 2: Create tv_user (should succeed concurrently)
SELECT dblink_connect('conn2', 'dbname=postgres');
SELECT dblink_send_query('conn2', $$
  CREATE TABLE tv_user AS
  SELECT
    tb_user.pk_user,
    tb_user.id,
    jsonb_build_object(
      'id', tb_user.id,
      'name', tb_user.name
    ) as data
  FROM tb_user;
$$);

-- Wait for completion
SELECT dblink_get_result('conn1');
SELECT dblink_get_result('conn2');

-- Verify both succeeded
SELECT COUNT(*) FROM pg_tview_meta;  -- Should be 2

-- Test 2: Concurrent DROP (different TVIEWs)
-- Test 3: Concurrent CREATE and DROP (same TVIEW - one should fail)
-- Test 4: Concurrent refresh operations
-- Test 5: CREATE TVIEW during active transaction

-- Cleanup
SELECT dblink_disconnect('conn1');
SELECT dblink_disconnect('conn2');
```

**Test Coverage**:
- [ ] Concurrent CREATE on different TVIEWs
- [ ] Concurrent DROP on different TVIEWs
- [ ] CREATE and DROP on same TVIEW (conflict detection)
- [ ] Concurrent refresh operations
- [ ] TVIEW creation during active transaction
- [ ] Deadlock avoidance verification

**Acceptance Criteria**:
- [ ] 5+ concurrent scenarios tested
- [ ] No deadlocks observed
- [ ] Proper error messages for conflicts
- [ ] Transaction isolation maintained
- [ ] Metadata consistency verified after each test

---

#### Task 2.3: Large-Scale Stress Tests (P2)
**Effort**: 8-10 hours

**New File**: `test/sql/comprehensive_benchmarks/scenarios/02_stress_test_large_scale.sql`

**Test Datasets**:
```sql
-- Dataset 1: 1 million rows
-- Note: id is UUID, pk_item is integer primary key
CREATE TABLE tb_stress_item (
  pk_item BIGSERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_category INTEGER,  -- foreign key to category (integer)
  value INTEGER,
  data_field TEXT
);

-- Insert 1M rows (use generate_series)
INSERT INTO tb_stress_item (fk_category, value, data_field)
SELECT
  (random() * 100)::INTEGER,
  (random() * 1000000)::INTEGER,
  'data_' || gs
FROM generate_series(1, 1000000) gs;

-- Create TVIEW
CREATE TABLE tv_stress_item AS
SELECT
  tb_stress_item.pk_item,
  tb_stress_item.id,
  jsonb_build_object(
    'id', tb_stress_item.id,
    'categoryId', tb_stress_item.fk_category,
    'value', tb_stress_item.value
  ) as data
FROM tb_stress_item;

-- Measure cascade performance
\timing on
UPDATE tb_stress_item
SET value = tb_stress_item.value + 1
WHERE tb_stress_item.fk_category = 1;
COMMIT;
\timing off
```

**Stress Test Scenarios**:
1. **Large Dataset**: 1M rows, single TVIEW
2. **Deep Cascade**: 5-level dependency chain, 100K rows each
3. **Wide Cascade**: 1 base table → 10 TVIEWs, 100K rows
4. **Bulk Operations**: INSERT 10K rows at once
5. **Memory Pressure**: Monitor memory usage during large cascades

**Metrics to Collect**:
- Memory usage (pg_stat_activity)
- Disk I/O (pg_stat_io)
- Query execution time
- Transaction commit latency
- Cache hit rates

**New File**: `test/sql/comprehensive_benchmarks/stress_test_results.md`

**Template**:
```markdown
# Stress Test Results

## Dataset: 1M Rows Single TVIEW

| Operation | Rows Affected | Time (ms) | Memory (MB) | Notes |
|-----------|---------------|-----------|-------------|-------|
| TVIEW Creation | 1,000,000 | TBD | TBD | Initial population |
| Single-row Update | 1 | TBD | TBD | Cascade update |
| Bulk Update (1K) | 1,000 | TBD | TBD | Batch cascade |
| Bulk Update (10K) | 10,000 | TBD | TBD | Large batch |

## Dataset: 5-Level Cascade (100K rows per level)

| Level | Entity | Rows | Cascade Time (ms) |
|-------|--------|------|-------------------|
| 1 | base_table | 100,000 | - |
| 2 | tv_level1 | 100,000 | TBD |
| 3 | tv_level2 | 100,000 | TBD |
| 4 | tv_level3 | 100,000 | TBD |
| 5 | tv_level4 | 100,000 | TBD |

**Total Cascade Time**: TBD ms
**Rows Updated**: 400,000
**Performance**: X rows/sec
```

**Acceptance Criteria**:
- [ ] 1M+ row tests execute successfully
- [ ] Deep cascade (5+ levels) tested
- [ ] Wide cascade (10+ TVIEWs) tested
- [ ] Memory usage profiled and documented
- [ ] Performance baselines established
- [ ] No crashes or OOM errors
- [ ] Results documented in stress_test_results.md

---

#### Task 2.4: Edge Case Integration Tests
**Effort**: 5-6 hours

**New File**: `test/sql/80_edge_cases.sql`

**Test Cases**:
```sql
-- Test 1: Empty base table
CREATE TABLE tb_empty (
  pk_empty SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid()
);
CREATE TABLE tv_empty AS
SELECT
  tb_empty.pk_empty,
  tb_empty.id,
  '{}'::jsonb as data
FROM tb_empty;
-- Should succeed with 0 rows

-- Test 2: NULL values in JSONB
CREATE TABLE tb_null (
  pk_null SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  nullable_field TEXT
);
INSERT INTO tb_null (nullable_field) VALUES (NULL);
CREATE TABLE tv_null AS
SELECT
  tb_null.pk_null,
  tb_null.id,
  jsonb_build_object(
    'id', tb_null.id,
    'field', tb_null.nullable_field
  ) as data
FROM tb_null;
-- Verify: SELECT data->'field' FROM tv_null; -- Should be JSON null

-- Test 3: Very large JSONB documents (>1MB)
CREATE TABLE tb_large_jsonb (
  pk_large SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  large_data TEXT
);
INSERT INTO tb_large_jsonb (large_data)
VALUES (repeat('x', 2000000));  -- 2MB text field
CREATE TABLE tv_large_jsonb AS
SELECT
  tb_large_jsonb.pk_large,
  tb_large_jsonb.id,
  jsonb_build_object(
    'id', tb_large_jsonb.id,
    'data', tb_large_jsonb.large_data
  ) as data
FROM tb_large_jsonb;
-- Should handle large documents

-- Test 4: Unicode and special characters
CREATE TABLE tb_unicode (
  pk_unicode SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  emoji_field TEXT
);
INSERT INTO tb_unicode (emoji_field) VALUES ('🚀 PostgreSQL 🐘');
CREATE TABLE tv_unicode AS
SELECT
  tb_unicode.pk_unicode,
  tb_unicode.id,
  jsonb_build_object(
    'id', tb_unicode.id,
    'emoji', tb_unicode.emoji_field
  ) as data
FROM tb_unicode;
-- Verify: SELECT data->>'emoji' FROM tv_unicode;

-- Test 5: Circular FK references (self-referential)
CREATE TABLE tb_tree (
  pk_tree SERIAL PRIMARY KEY,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  fk_parent INTEGER REFERENCES tb_tree(pk_tree),
  name TEXT
);
INSERT INTO tb_tree (fk_parent, name) VALUES (NULL, 'root');
INSERT INTO tb_tree (fk_parent, name) VALUES (1, 'child');
CREATE TABLE tv_tree AS
SELECT
  tb_tree.pk_tree,
  tb_tree.id,
  jsonb_build_object(
    'id', tb_tree.id,
    'parentId', tb_tree.fk_parent,
    'name', tb_tree.name
  ) as data
FROM tb_tree;
-- TVIEW should handle without infinite loops

-- Test 6: Transaction rollback
BEGIN;
  CREATE TABLE tv_rollback_test AS
  SELECT
    tb_stress_item.pk_item as pk_rollback_test,
    tb_stress_item.id,
    tb_stress_item.data
  FROM tb_stress_item
  LIMIT 1000;
ROLLBACK;
-- Verify TVIEW doesn't exist
SELECT COUNT(*) FROM pg_tview_meta WHERE entity = 'rollback_test';
-- Should be 0

-- Test 7: Savepoint handling
BEGIN;
  CREATE TABLE tv_savepoint AS
  SELECT
    tb_stress_item.pk_item as pk_savepoint,
    tb_stress_item.id,
    tb_stress_item.data
  FROM tb_stress_item
  LIMIT 100;
  SAVEPOINT sp1;
  DROP TABLE tv_savepoint;
  ROLLBACK TO sp1;
  -- tv_savepoint should still exist
COMMIT;

-- Test 8: Very long entity names (PostgreSQL identifier limit: 63 chars)
CREATE TABLE tv_this_is_a_very_long_tview_name_that_approaches_the_limit_x AS
SELECT
  tb_stress_item.pk_item as pk_this_is_a_very_long_tview_name_that_approaches_the_limit_x,
  tb_stress_item.id,
  tb_stress_item.data
FROM tb_stress_item
LIMIT 10;
-- Should truncate gracefully

-- Test 9: Special characters in data (JSON escaping)
INSERT INTO tb_unicode (emoji_field) VALUES ('{"key": "value with \"quotes\""}');
-- Should escape properly in JSONB

-- Test 10: Base table with composite primary key
-- (pg_tviews might not support this - verify error message)
CREATE TABLE tb_composite (
  pk_composite_1 INTEGER,
  pk_composite_2 INTEGER,
  id UUID NOT NULL DEFAULT gen_random_uuid(),
  data TEXT,
  PRIMARY KEY (pk_composite_1, pk_composite_2)
);
-- Attempting to create TVIEW should give clear error
-- Note: Trinity pattern requires single integer PK
```

**Acceptance Criteria**:
- [ ] 10+ edge cases tested
- [ ] Empty table handling verified
- [ ] NULL value behavior documented
- [ ] Large JSONB documents supported
- [ ] Unicode/special characters handled
- [ ] Transaction rollback/savepoint tested
- [ ] Error messages clear for unsupported scenarios

---

#### Task 2.5: Test Assertions & Validation
**Effort**: 4-5 hours

**Goal**: Add explicit assertions to all tests

**Pattern**:
```sql
-- BEFORE (weak validation)
CREATE TABLE tv_test AS SELECT ...;
-- Test passes if no error

-- AFTER (strong validation)
-- Trinity pattern: tb_test (pk_test SERIAL, id UUID) -> tv_test (pk_test, id, data JSONB)
CREATE TABLE tv_test AS
SELECT
  tb_test.pk_test,
  tb_test.id,
  jsonb_build_object(
    'id', tb_test.id,
    'field', tb_test.field
  ) as data
FROM tb_test;

-- Verify metadata
SELECT COUNT(*) = 1 FROM pg_tview_meta WHERE pg_tview_meta.entity = 'test';

-- Verify data
SELECT COUNT(*) FROM tv_test;  -- Expected: X rows
SELECT tv_test.data->>'field' FROM tv_test WHERE tv_test.pk_test = 1;  -- Expected: 'value'

-- Verify triggers
SELECT COUNT(*) >= 1 FROM pg_trigger
WHERE pg_trigger.tgname LIKE '%tview%'
  AND pg_trigger.tgrelid = 'tb_test'::regclass;

-- Verify backing view
SELECT COUNT(*) = 1 FROM pg_views WHERE pg_views.viewname = 'v_test';
```

**Files to Update**:
- `test/sql/10_schema_inference_simple.sql`
- `test/sql/40_refresh_trigger_dynamic_pk.sql`
- `test/sql/42_cascade_fk_lineage.sql`
- All other test files

**Acceptance Criteria**:
- [ ] Every test has explicit SELECT assertions
- [ ] Expected vs. actual values compared
- [ ] Metadata verified (pg_tview_meta, pg_trigger, pg_views)
- [ ] Data correctness validated
- [ ] Test output shows PASS/FAIL clearly

---

#### Task 2.6: Test Coverage Reporting
**Effort**: 3-4 hours

**Goal**: Measure and report code coverage

**Setup**: Use `cargo-llvm-cov` for Rust code coverage

```bash
# Install coverage tool
cargo install cargo-llvm-cov

# Run tests with coverage
cargo llvm-cov --html --open

# Generate report
cargo llvm-cov --lcov --output-path target/coverage.lcov
```

**New File**: `.github/workflows/coverage.yml` (if using GitHub Actions)

```yaml
name: Code Coverage

on: [push, pull_request]

jobs:
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true
      - name: Install pgrx
        run: cargo install cargo-pgrx
      - name: Install coverage tool
        run: cargo install cargo-llvm-cov
      - name: Run tests with coverage
        run: cargo llvm-cov --lcov --output-path coverage.lcov
      - name: Upload to Codecov
        uses: codecov/codecov-action@v3
        with:
          files: ./coverage.lcov
```

**New File**: `docs/development/testing.md`

```markdown
# Testing Guide

## Running Tests

```bash
# All tests
cargo pgrx test pg17

# Specific test
cargo pgrx test pg17 -- --test 10_schema_inference

# With coverage
cargo llvm-cov --html
```

## Coverage Targets

- **Current Coverage**: TBD%
- **Target Coverage**: 85%+

| Module | Coverage | Target |
|--------|----------|--------|
| ddl/create.rs | TBD% | 90%+ |
| ddl/drop.rs | TBD% | 90%+ |
| refresh/main.rs | TBD% | 85%+ |
| dependency/graph.rs | TBD% | 80%+ |
```

**Acceptance Criteria**:
- [ ] Coverage tool integrated
- [ ] Coverage reports generated
- [ ] Coverage targets defined per module
- [ ] CI/CD runs coverage checks
- [ ] Coverage badge added to README

---

### Phase 2 Acceptance Criteria

- [ ] Test build fixed (--no-default-features works)
- [ ] Concurrent DDL tests implemented (5+ scenarios)
- [ ] Large-scale stress tests (1M+ rows) passing
- [ ] Edge cases tested (10+ scenarios)
- [ ] All tests have explicit assertions
- [ ] Test coverage reporting enabled (target: 85%+)
- [ ] Testing score: 95/100 ✅

---

## Phase 3: Production Readiness
**Goal**: 84/100 → 98/100
**Effort**: 20-30 hours
**Priority**: High

### Objectives

1. Complete monitoring infrastructure
2. Implement health check system
3. Create operational runbooks
4. Add resource limit documentation
5. Implement audit logging
6. Create disaster recovery procedures

### Task Breakdown

#### Task 3.1: Complete Monitoring Infrastructure (P1)
**Effort**: 6-8 hours

**Missing Functions to Implement**:

**File**: `src/lib.rs`

```rust
/// Health check function for production monitoring
///
/// Returns a comprehensive health status including:
/// - Extension version
/// - jsonb_delta availability
/// - Metadata consistency
/// - Orphaned triggers
/// - Queue status
#[pg_extern]
fn pg_tviews_health_check() -> TableIterator<'static, (
    name!(status, String),
    name!(component, String),
    name!(message, String),
    name!(severity, String),
)> {
    let mut results = Vec::new();

    // Check 1: Extension loaded
    results.push((
        "OK".to_string(),
        "extension".to_string(),
        format!("pg_tviews version {}", env!("CARGO_PKG_VERSION")),
        "info".to_string(),
    ));

    // Check 2: jsonb_delta availability
    let has_jsonb_delta = Spi::get_one::<bool>(
        "SELECT COUNT(*) > 0 FROM pg_extension WHERE extname = 'jsonb_delta'"
    ).unwrap_or(Some(false)).unwrap_or(false);

    if has_jsonb_delta {
        results.push((
            "OK".to_string(),
            "jsonb_delta".to_string(),
            "jsonb_delta extension available (optimized mode)".to_string(),
            "info".to_string(),
        ));
    } else {
        results.push((
            "WARNING".to_string(),
            "jsonb_delta".to_string(),
            "jsonb_delta not installed (falling back to standard JSONB)".to_string(),
            "warning".to_string(),
        ));
    }

    // Check 3: Metadata consistency
    let orphaned_meta = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_tview_meta m
         WHERE NOT EXISTS (
           SELECT 1 FROM pg_class WHERE relname = 'tv_' || m.entity
         )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_meta > 0 {
        results.push((
            "ERROR".to_string(),
            "metadata".to_string(),
            format!("{} orphaned metadata entries found", orphaned_meta),
            "error".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "metadata".to_string(),
            "All metadata entries valid".to_string(),
            "info".to_string(),
        ));
    }

    // Check 4: Orphaned triggers
    let orphaned_triggers = Spi::get_one::<i64>(
        "SELECT COUNT(*) FROM pg_trigger
         WHERE tgname LIKE 'tview_%'
           AND tgrelid NOT IN (
             SELECT ('tb_' || entity)::regclass::oid
             FROM pg_tview_meta
           )"
    ).unwrap_or(Some(0)).unwrap_or(0);

    if orphaned_triggers > 0 {
        results.push((
            "WARNING".to_string(),
            "triggers".to_string(),
            format!("{} orphaned triggers found", orphaned_triggers),
            "warning".to_string(),
        ));
    } else {
        results.push((
            "OK".to_string(),
            "triggers".to_string(),
            "All triggers properly linked".to_string(),
            "info".to_string(),
        ));
    }

    // Check 5: Cache status
    // (Implementation depends on cache architecture)

    TableIterator::new(results)
}
```

**Monitoring Views to Implement**:

**File**: `src/metadata.rs` (add to extension_sql!)

```sql
-- Queue monitoring view
CREATE OR REPLACE VIEW pg_tviews_queue_realtime AS
SELECT
    current_setting('application_name') as session,
    pg_backend_pid() as backend_pid,
    txid_current() as transaction_id,
    0 as queue_size,  -- TODO: Implement queue introspection
    ARRAY[]::TEXT[] as entities,
    NOW() as last_enqueued;

-- Cache statistics view
CREATE OR REPLACE VIEW pg_tviews_cache_stats AS
SELECT
    'graph_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size
UNION ALL
SELECT
    'table_cache' as cache_type,
    0::BIGINT as entries,
    '0 bytes' as estimated_size;

-- Performance summary view
CREATE OR REPLACE VIEW pg_tviews_performance_summary AS
SELECT
    entity,
    COUNT(*) as total_refreshes,
    0.0 as avg_refresh_ms,
    NOW() as last_refresh
FROM pg_tview_meta
GROUP BY entity;
```

**Acceptance Criteria**:
- [ ] `pg_tviews_health_check()` function implemented
- [ ] Returns status for 5+ components
- [ ] Severity levels: info, warning, error
- [ ] `pg_tviews_queue_realtime` view created
- [ ] `pg_tviews_cache_stats` view created
- [ ] `pg_tviews_performance_summary` view created
- [ ] Documentation updated in docs/operations/monitoring.md

---

#### Task 3.2: Operational Runbooks
**Effort**: 4-6 hours

**New File**: `docs/operations/runbooks.md`

```markdown
# Operational Runbooks

## Runbook 1: TVIEW Not Updating

**Symptom**: Data changes in base tables not reflected in tv_* tables

**Diagnosis Steps**:
```sql
-- 1. Check if triggers exist
-- Trinity pattern: tb_your_table has pk_your_table (integer), id (UUID)
SELECT
  pg_trigger.tgname,
  pg_trigger.tgrelid::regclass,
  pg_trigger.tgenabled
FROM pg_trigger
WHERE pg_trigger.tgname LIKE 'tview%'
  AND pg_trigger.tgrelid = 'tb_your_table'::regclass;

-- 2. Check metadata
SELECT * FROM pg_tview_meta WHERE pg_tview_meta.entity = 'your_entity';

-- 3. Check for errors in logs
-- (Review PostgreSQL logs)

-- 4. Manual refresh test
-- Note: pk_your_table is integer (SERIAL), id is UUID
UPDATE tb_your_table
SET some_field = tb_your_table.some_field
WHERE tb_your_table.pk_your_table = 1;
COMMIT;
SELECT * FROM tv_your_entity WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:
- If triggers missing: Recreate TVIEW
- If metadata corrupt: Clean up and recreate
- If errors in logs: Address root cause

---

## Runbook 2: Slow Cascade Updates

**Symptom**: Cascade updates taking >1 second

**Diagnosis Steps**:
```sql
-- 1. Check if jsonb_delta installed
SELECT pg_tviews_check_jsonb_delta();

-- 2. Check dependency depth
SELECT
  pg_tview_meta.entity,
  array_length(pg_tview_meta.dependencies, 1) as dep_count
FROM pg_tview_meta
ORDER BY dep_count DESC;

-- 3. Check for missing indexes
SELECT
  pg_indexes.schemaname,
  pg_indexes.tablename,
  pg_indexes.indexname
FROM pg_indexes
WHERE pg_indexes.tablename LIKE 'tv_%'
  AND pg_indexes.indexname NOT LIKE '%pkey%';

-- 4. Analyze query plans
-- Note: pk_your_entity is integer, id is UUID
EXPLAIN ANALYZE
UPDATE tv_your_entity
SET data = tv_your_entity.data
WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:
- Install jsonb_delta if missing (1.5-3× speedup)
- Create indexes on fk_* columns
- Enable statement-level triggers for bulk operations
- Consider flattening deep dependency chains

---

## Runbook 3: Out of Memory During Cascade

**Symptom**: PostgreSQL OOM killer or "out of memory" errors

**Diagnosis Steps**:
```sql
-- 1. Check cascade size
-- Trinity pattern: All entities use singular names (tv_post, not tv_posts)
SELECT
  pg_tview_meta.entity,
  pg_size_pretty(pg_relation_size('tv_' || pg_tview_meta.entity)) as tview_size,
  array_length(pg_tview_meta.dependencies, 1) as cascade_depth
FROM pg_tview_meta
ORDER BY pg_relation_size('tv_' || pg_tview_meta.entity) DESC;

-- 2. Check work_mem setting
SHOW work_mem;

-- 3. Monitor memory during cascade
SELECT
  pg_stat_activity.pid,
  pg_stat_activity.query,
  pg_stat_activity.state,
  pg_size_pretty(pg_backend_memory_contexts.total_bytes)
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
WHERE pg_stat_activity.backend_type = 'client backend';
```

**Resolution**:
- Increase work_mem (session or global)
- Batch large updates
- Consider partitioning large TVIEWs
- Implement rate limiting for bulk operations

---

## Runbook 4: Extension Upgrade Failed

**Symptom**: ALTER EXTENSION pg_tviews UPDATE fails

**Diagnosis Steps**:
```sql
-- 1. Check current version
SELECT * FROM pg_extension WHERE pg_extension.extname = 'pg_tviews';

-- 2. Check for version mismatch
SELECT pg_tviews_version();

-- 3. Review upgrade script
-- cat $(pg_config --sharedir)/extension/pg_tviews--oldver--newver.sql
```

**Resolution**:
1. Backup metadata: `CREATE TABLE pg_tview_meta_backup AS SELECT * FROM pg_tview_meta;`
2. If upgrade fails, rollback: `ALTER EXTENSION pg_tviews UPDATE TO 'old_version';`
3. Restore metadata if needed
4. Contact maintainer if persistent issue

---

## Runbook 5: Orphaned Triggers After TVIEW Drop

**Symptom**: Triggers remain after dropping TVIEW

**Diagnosis Steps**:
```sql
-- Find orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
SELECT
  pg_trigger.tgname,
  pg_class.relname
FROM pg_trigger
JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
WHERE pg_trigger.tgname LIKE 'tview_%'
  AND NOT EXISTS (
    SELECT 1 FROM pg_tview_meta
    WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
  );
```

**Resolution**:
```sql
-- Drop orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
DO $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN
        SELECT
          pg_trigger.tgname,
          pg_class.relname
        FROM pg_trigger
        JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
        WHERE pg_trigger.tgname LIKE 'tview_%'
          AND NOT EXISTS (
            SELECT 1 FROM pg_tview_meta
            WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
          )
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', r.tgname, r.relname);
    END LOOP;
END $$;
```
```

**Acceptance Criteria**:
- [ ] 5+ operational runbooks created
- [ ] Each runbook has diagnosis and resolution steps
- [ ] SQL queries provided for each step
- [ ] Escalation procedures documented
- [ ] Linked from docs/operations.md

---

#### Task 3.3: Resource Limit Documentation
**Effort**: 2-3 hours

**Update File**: `docs/reference/limits.md` (new)

```markdown
# Resource Limits and Recommendations

## Tested Limits

Based on benchmark testing and production experience:

| Resource | Tested Limit | Recommended Max | Notes |
|----------|--------------|-----------------|-------|
| TVIEW Size | 10M rows | 5M rows | Beyond 5M, consider partitioning |
| Dependency Depth | 10 levels | 5 levels | Hard limit: 10, soft limit: 5 for performance |
| Cascade Width | 50 TVIEWs | 20 TVIEWs | One base table → N TVIEWs |
| JSONB Data Size | 10 MB | 1 MB | Per-row JSONB document size |
| Concurrent Refresh | 100 sessions | 50 sessions | Parallel cascade updates |
| Batch Size | 100K rows | 10K rows | Single UPDATE affecting TVIEWs |

## PostgreSQL Configuration Recommendations

### For Small Deployments (<100K rows per TVIEW)
```sql
-- postgresql.conf
work_mem = 64MB
shared_buffers = 256MB
effective_cache_size = 1GB
maintenance_work_mem = 128MB
```

### For Medium Deployments (100K-1M rows)
```sql
work_mem = 128MB
shared_buffers = 1GB
effective_cache_size = 4GB
maintenance_work_mem = 512MB
max_parallel_workers_per_gather = 4
```

### For Large Deployments (>1M rows)
```sql
work_mem = 256MB
shared_buffers = 4GB
effective_cache_size = 16GB
maintenance_work_mem = 2GB
max_parallel_workers_per_gather = 8
random_page_cost = 1.1  # For SSD
```

## Capacity Planning

### Estimating TVIEW Size
```sql
-- Current size
-- Trinity pattern: tv_your_entity has pk_your_entity (int), id (UUID), data (JSONB)
SELECT pg_size_pretty(pg_relation_size('tv_your_entity'));

-- With indexes (includes pk_your_entity primary key, id index)
SELECT pg_size_pretty(pg_total_relation_size('tv_your_entity'));

-- Projection: If base table has 1M rows
-- Estimated TVIEW size ≈ base_table_size × 1.5 to 2.0
-- (due to JSONB denormalization + UUID storage)
```

### Estimating Cascade Performance
```
Single-row cascade time = 5-8ms (with jsonb_delta)
Batch cascade (N rows) ≈ 5ms + (N × 0.5ms)
Example: 1000 rows ≈ 505ms
```

## Scaling Recommendations

### Horizontal Scaling
- Use read replicas for TVIEW queries
- Primary handles writes and cascade updates
- Replicas serve SELECT queries

### Vertical Scaling
- More RAM → larger shared_buffers, work_mem
- More CPU → increase max_parallel_workers
- Faster storage → reduce random_page_cost

### Partitioning Strategy
For TVIEWs >5M rows:
```sql
-- Partition by date (for time-series data)
-- Note: Use singular names (tv_event, not tv_events)
CREATE TABLE tv_event_y2025_m01 PARTITION OF tv_event
FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');

-- Partition by hash (for general data)
-- Note: Use singular names (tv_user, not tv_users)
CREATE TABLE tv_user_0 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 0);

CREATE TABLE tv_user_1 PARTITION OF tv_user
FOR VALUES WITH (MODULUS 10, REMAINDER 1);

-- Trinity pattern reminder:
-- - Table: tb_user (pk_user SERIAL, id UUID)
-- - TVIEW: tv_user (pk_user, id, data JSONB)
-- - Always singular, never plural
```
```

**Acceptance Criteria**:
- [ ] Tested limits documented
- [ ] PostgreSQL config recommendations provided
- [ ] Capacity planning formulas given
- [ ] Scaling strategies explained
- [ ] Partitioning examples included

---

#### Task 3.4: Audit Logging Implementation
**Effort**: 4-5 hours

**New File**: `src/audit.rs`

```rust
use pgrx::prelude::*;

/// Audit log entry
#[derive(Debug, PostgresType, Serialize, Deserialize)]
pub struct AuditLogEntry {
    operation: String,
    entity: String,
    performed_by: String,
    performed_at: pgrx::Timestamp,
    details: JsonB,
}

/// Log TVIEW creation
pub fn log_create(entity: &str, definition: &str) -> spi::Result<()> {
    let current_user = Spi::get_one::<String>("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('CREATE', '{}', '{}', '{}'::jsonb)",
        entity.replace("'", "''"),
        current_user.replace("'", "''"),
        serde_json::json!({
            "definition": definition,
            "version": env!("CARGO_PKG_VERSION")
        })
    ))?;

    Ok(())
}

/// Log TVIEW drop
pub fn log_drop(entity: &str) -> spi::Result<()> {
    let current_user = Spi::get_one::<String>("SELECT current_user")?
        .unwrap_or_else(|| "unknown".to_string());

    Spi::run(&format!(
        "INSERT INTO pg_tview_audit_log (operation, entity, performed_by, details)
         VALUES ('DROP', '{}', '{}', '{{}}'::jsonb)",
        entity.replace("'", "''"),
        current_user.replace("'", "''")
    ))?;

    Ok(())
}
```

**Metadata Table**:

**File**: `src/metadata.rs` (add to extension_sql!)

```sql
CREATE TABLE IF NOT EXISTS public.pg_tview_audit_log (
    log_id BIGSERIAL PRIMARY KEY,
    operation TEXT NOT NULL,  -- CREATE, DROP, ALTER, REFRESH
    entity TEXT NOT NULL,
    performed_by TEXT NOT NULL DEFAULT current_user,
    performed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    details JSONB,
    client_addr INET DEFAULT inet_client_addr(),
    client_port INTEGER DEFAULT inet_client_port()
);

CREATE INDEX idx_audit_log_entity ON pg_tview_audit_log(entity);
CREATE INDEX idx_audit_log_timestamp ON pg_tview_audit_log(performed_at DESC);

COMMENT ON TABLE pg_tview_audit_log IS 'Audit log for TVIEW operations';
```

**Integrate into DDL operations**:

**File**: `src/ddl/create.rs`

```rust
// Add after successful TVIEW creation
crate::audit::log_create(entity_name, select_sql)?;
```

**File**: `src/ddl/drop.rs`

```rust
// Add after successful TVIEW drop
crate::audit::log_drop(entity_name)?;
```

**Acceptance Criteria**:
- [ ] Audit log table created
- [ ] CREATE operations logged
- [ ] DROP operations logged
- [ ] User and timestamp captured
- [ ] Client connection info recorded
- [ ] Query interface documented

---

#### Task 3.5: Disaster Recovery Procedures
**Effort**: 3-4 hours

**New File**: `docs/operations/disaster-recovery.md`

```markdown
# Disaster Recovery Procedures

## Backup Strategy

### What to Backup

**Critical (Must Backup)**:
- `pg_tview_meta` table (TVIEW definitions)
- Base tables (`tb_*`)

**Optional (Can Recreate)**:
- TVIEW tables (`tv_*`) - can be rebuilt from definitions
- Backing views (`v_*`) - auto-created with TVIEWs

### Backup Commands

```bash
# Backup metadata only (small, fast)
pg_dump -t pg_tview_meta -t pg_tview_helpers > tview_metadata_backup.sql

# Backup entire database
pg_dump dbname > full_backup.sql

# Backup with pg_basebackup (for PITR)
pg_basebackup -D /backup/location -Ft -z -P
```

## Recovery Scenarios

### Scenario 1: Corrupted TVIEW Data

**Problem**: tv_* data is inconsistent with base tables

**Recovery**:
```sql
-- Option A: Drop and recreate
DROP TABLE tv_your_entity CASCADE;

-- Recreate from metadata
-- Trinity pattern: tv_your_entity has pk_your_entity, id (UUID), data (JSONB)
SELECT pg_tviews_create(
    pg_tview_meta.entity,
    pg_tview_meta.definition
)
FROM pg_tview_meta
WHERE pg_tview_meta.entity = 'your_entity';

-- Option B: Full refresh (if supported)
-- REFRESH MATERIALIZED VIEW tv_your_entity;  -- Not implemented yet
```

### Scenario 2: Lost pg_tview_meta Table

**Problem**: Metadata table deleted or corrupted

**Recovery**:
```sql
-- Restore from backup
psql dbname < tview_metadata_backup.sql

-- Verify restoration
SELECT COUNT(*) FROM pg_tview_meta;

-- Recreate triggers (they may be lost)
SELECT pg_tviews_reinstall_triggers();  -- Function to implement
```

### Scenario 3: Extension Corruption

**Problem**: Extension files corrupted or deleted

**Recovery**:
```bash
# Reinstall extension files
cd /path/to/pg_tviews
cargo pgrx install --release

# In PostgreSQL
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

# Restore metadata
psql dbname < tview_metadata_backup.sql

# Recreate TVIEWs from metadata
# Trinity pattern: All TVIEWs have pk_{entity} (integer), id (UUID), data (JSONB)
SELECT pg_tviews_create(
    pg_tview_meta.entity,
    pg_tview_meta.definition
)
FROM pg_tview_meta;
```

### Scenario 4: Point-in-Time Recovery (PITR)

**Problem**: Need to restore to specific point in time

**Recovery**:
```bash
# Stop PostgreSQL
systemctl stop postgresql

# Restore base backup
rm -rf /var/lib/postgresql/data
tar -xzf /backup/base.tar.gz -C /var/lib/postgresql/data

# Configure recovery
cat > /var/lib/postgresql/data/recovery.conf <<EOF
restore_command = 'cp /backup/wal/%f %p'
recovery_target_time = '2025-12-10 14:30:00'
EOF

# Start PostgreSQL (will replay WAL to target time)
systemctl start postgresql

# Verify TVIEWs
SELECT COUNT(*) FROM pg_tview_meta;
```

## Automated Backup Script

```bash
#!/bin/bash
# backup_tviews.sh

BACKUP_DIR="/backups/pg_tviews"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
DB_NAME="your_database"

mkdir -p $BACKUP_DIR

# Backup metadata
pg_dump -t pg_tview_meta -t pg_tview_helpers -t pg_tview_audit_log \
    $DB_NAME > $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Compress
gzip $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Cleanup old backups (keep last 30 days)
find $BACKUP_DIR -name "tview_metadata_*.sql.gz" -mtime +30 -delete

echo "Backup completed: $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql.gz"
```

## Testing Recovery Procedures

**Test Plan**: Run recovery drills quarterly

```bash
# 1. Create test database
createdb test_recovery

# 2. Create sample TVIEWs
psql test_recovery < sample_tviews.sql

# 3. Backup metadata
pg_dump -t pg_tview_meta test_recovery > recovery_test_backup.sql

# 4. Simulate disaster (drop TVIEWs)
psql test_recovery -c "DROP EXTENSION pg_tviews CASCADE"

# 5. Recover
psql test_recovery -c "CREATE EXTENSION pg_tviews"
psql test_recovery < recovery_test_backup.sql

# 6. Verify
psql test_recovery -c "SELECT COUNT(*) FROM pg_tview_meta"

# 7. Cleanup
dropdb test_recovery
```
```

**Acceptance Criteria**:
- [ ] Backup strategy documented
- [ ] 4+ recovery scenarios covered
- [ ] Automated backup script provided
- [ ] Recovery testing procedure documented
- [ ] RTO/RPO targets defined

---

### Phase 3 Acceptance Criteria

- [ ] Health check function implemented and tested
- [ ] Monitoring views created (queue, cache, performance)
- [ ] 5+ operational runbooks written
- [ ] Resource limits documented with recommendations
- [ ] Audit logging implemented for DDL operations
- [ ] Disaster recovery procedures documented and tested
- [ ] Production Readiness score: 98/100 ✅

---

## Phase 4: Performance & Optimization
**Goal**: 88/100 → 95/100
**Effort**: 15-25 hours
**Priority**: Medium

### Objectives

1. Add index optimization guide
2. Implement query plan analysis tools
3. Create performance tuning utilities
4. Add cache size configuration
5. Optimize prepared statement handling
6. Document performance best practices

### Task Breakdown

#### Task 4.1: Index Optimization Guide (P2)
**Effort**: 3-4 hours

**New File**: `docs/operations/index-optimization.md`

```markdown
# Index Optimization Guide

## Automatic Indexes

pg_tviews automatically creates:
- PRIMARY KEY index on `pk_<entity>` column
- (Other indexes TBD - verify in code)

## Recommended Manual Indexes

### 1. Foreign Key Indexes

**Why**: Speed up cascade updates and JOIN operations

```sql
-- For each fk_* column in tv_* tables
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);
CREATE INDEX idx_tv_comment_fk_post ON tv_comment(fk_post);
```

**When to Create**: Always create for fk_* columns used in cascades

### 2. JSONB GIN Indexes

**Why**: Enable fast JSONB queries (WHERE data @> '{}')

```sql
-- GIN index for JSONB containment queries
CREATE INDEX idx_tv_post_data_gin ON tv_post USING GIN(data);

-- Specific JSONB path index (PostgreSQL 14+)
CREATE INDEX idx_tv_post_data_title
ON tv_post USING GIN((data -> 'title'));
```

**When to Create**: If you query JSONB data frequently

### 3. UUID Filtering Indexes

**Why**: Speed up GraphQL queries by UUID

```sql
-- Index on id (UUID) column for GraphQL queries
-- Note: id is UUID, pk_post is integer
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- NO need for UUID foreign key indexes - use integer fk_* columns instead
-- Foreign keys in Trinity pattern are integers (fk_user, not user_id)
-- See: Foreign Key Indexes section above
```

**When to Create**: Always for GraphQL Cascade integration - but only on `id` (UUID), not on FKs

### 4. Composite Indexes

**Why**: Optimize multi-column queries

```sql
-- For queries filtering by both fk_user and status
CREATE INDEX idx_tv_post_user_status
ON tv_post(fk_user, (data->>'status'));
```

**When to Create**: Based on actual query patterns

## Index Strategy by Use Case

### Small TVIEWs (<10K rows)
- PRIMARY KEY (automatic)
- fk_* columns
- Skip JSONB indexes (table scan is fast enough)

### Medium TVIEWs (10K-1M rows)
- PRIMARY KEY (automatic)
- fk_* columns
- GIN index on data column
- UUID id column

### Large TVIEWs (>1M rows)
- PRIMARY KEY (automatic)
- fk_* columns
- GIN index on data column
- UUID id column
- Specific JSONB path indexes for frequent queries
- Consider partitioning (see docs/operations/partitioning.md)

## Index Maintenance

```sql
-- Check index usage
SELECT schemaname, tablename, indexname, idx_scan
FROM pg_stat_user_indexes
WHERE tablename LIKE 'tv_%'
ORDER BY idx_scan ASC;

-- Find unused indexes (idx_scan = 0 after significant runtime)
SELECT schemaname, tablename, indexname,
       pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE tablename LIKE 'tv_%'
  AND idx_scan = 0
  AND indexname NOT LIKE '%_pkey';

-- Reindex if bloated
REINDEX TABLE tv_your_entity;
```

## Performance Impact

| Index Type | Creation Time (1M rows) | Size Overhead | Query Speedup |
|------------|-------------------------|---------------|---------------|
| B-tree (int) | ~30 sec | +15% | 100-1000× |
| B-tree (uuid) | ~45 sec | +20% | 50-500× |
| GIN (jsonb) | ~2 min | +30% | 10-100× |

## Automated Index Recommendation

```sql
-- Function to suggest indexes (to implement)
-- Trinity pattern: All TVIEWs have pk_{entity} (int PK), id (UUID), data (JSONB)
CREATE OR REPLACE FUNCTION pg_tviews_suggest_indexes(entity_name TEXT)
RETURNS TABLE(index_suggestion TEXT, reason TEXT) AS $$
BEGIN
    -- Check for missing fk_* indexes (foreign keys are integers)
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_' ||
        information_schema.columns.column_name ||
        ' ON tv_' || entity_name || '(' ||
        information_schema.columns.column_name || ')' as index_suggestion,
        'Foreign key column without index (fk_* columns are integers)' as reason
    FROM information_schema.columns
    WHERE information_schema.columns.table_name = 'tv_' || entity_name
      AND information_schema.columns.column_name LIKE 'fk_%'
      AND information_schema.columns.column_name NOT IN (
        SELECT pg_attribute.attname
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
      );

    -- Suggest UUID index if missing
    RETURN QUERY
    SELECT
        'CREATE INDEX idx_tv_' || entity_name || '_id ON tv_' ||
        entity_name || '(id)' as index_suggestion,
        'UUID column without index (for GraphQL queries)' as reason
    WHERE NOT EXISTS (
        SELECT 1
        FROM pg_index
        JOIN pg_attribute ON pg_attribute.attrelid = pg_index.indrelid
          AND pg_attribute.attnum = ANY(pg_index.indkey)
        WHERE pg_index.indrelid = ('tv_' || entity_name)::regclass
          AND pg_attribute.attname = 'id'
    );
END;
$$ LANGUAGE plpgsql;
```
```

**Acceptance Criteria**:
- [ ] Index recommendations documented
- [ ] Index strategy by TVIEW size
- [ ] Index maintenance queries provided
- [ ] Performance impact measured
- [ ] Index suggestion function implemented

---

#### Task 4.2: Query Plan Analysis Tools
**Effort**: 4-5 hours

**New File**: `src/lib.rs` (add function)

```rust
/// Analyze cascade update query plan
///
/// Returns EXPLAIN output for a cascade update operation
#[pg_extern]
fn pg_tviews_analyze_cascade(
    entity: &str,
    pk_value: i64
) -> TableIterator<'static, (name!(query_plan, String),)> {
    let tview_name = format!("tv_{}", entity);
    let view_name = format!("v_{}", entity);

    // Get the refresh query that would execute
    // Trinity pattern: v_{entity} view returns pk_{entity}, id (UUID), data (JSONB)
    let refresh_query = format!(
        "SELECT {}.pk_{}, {}.id, {}.data FROM {} WHERE {}.pk_{} = {}",
        view_name, entity, view_name, view_name, view_name, view_name, entity, pk_value
    );

    // EXPLAIN ANALYZE the query
    let explain_query = format!("EXPLAIN (ANALYZE, BUFFERS) {}", refresh_query);

    let results = Spi::connect(|client| {
        let rows = client.select(&explain_query, None, None)?;
        let mut plans = Vec::new();

        for row in rows {
            if let Some(plan_line) = row[1].value::<String>()? {
                plans.push((plan_line,));
            }
        }

        Ok::<_, spi::Error>(plans)
    }).unwrap_or_default();

    TableIterator::new(results)
}

/// Show cascade dependency path
///
/// Returns the dependency chain for a given entity
#[pg_extern]
fn pg_tviews_show_cascade_path(entity: &str) -> TableIterator<'static, (
    name!(depth, i32),
    name!(entity_name, String),
    name!(dependency_type, String),
)> {
    // Query dependency graph from metadata
    let query = format!(
        "WITH RECURSIVE dep_tree AS (
            SELECT entity, 0 as depth, ARRAY[entity] as path
            FROM pg_tview_meta
            WHERE entity = '{}'

            UNION ALL

            SELECT m.entity, dt.depth + 1, dt.path || m.entity
            FROM dep_tree dt
            JOIN pg_tview_meta m ON m.dependencies && ARRAY[('tv_' || dt.entity)::regclass::oid]
            WHERE NOT (m.entity = ANY(dt.path))  -- Prevent cycles
              AND dt.depth < 10
        )
        SELECT depth, entity_name, 'cascade' as dependency_type
        FROM dep_tree
        ORDER BY depth",
        entity.replace("'", "''")
    );

    let results = Spi::connect(|client| {
        let rows = client.select(&query, None, None)?;
        let mut paths = Vec::new();

        for row in rows {
            let depth = row["depth"].value::<i32>()?.unwrap_or(0);
            let entity_name = row["entity_name"].value::<String>()?.unwrap_or_default();
            let dep_type = row["dependency_type"].value::<String>()?.unwrap_or_default();

            paths.push((depth, entity_name, dep_type));
        }

        Ok::<_, spi::Error>(paths)
    }).unwrap_or_default();

    TableIterator::new(results)
}
```

**Documentation**:

**File**: `docs/operations/performance-analysis.md` (new)

```markdown
# Performance Analysis

## Cascade Update Analysis

```sql
-- Analyze a specific cascade operation
-- Note: pk_post is integer (SERIAL), id is UUID
SELECT * FROM pg_tviews_analyze_cascade('post', 123);

-- Sample output:
                    query_plan
----------------------------------------------------------
 Seq Scan on v_post  (cost=0.00..15.50 rows=1 width=48)
   Filter: (pk_post = 123)
   Buffers: shared hit=8
 Planning Time: 0.123 ms
 Execution Time: 0.456 ms

-- Trinity pattern: v_post view has pk_post (int), id (UUID), data (JSONB)
```

## Dependency Path Visualization

```sql
-- Show cascade dependency chain
SELECT * FROM pg_tviews_show_cascade_path('user');

-- Sample output:
 depth | entity_name | dependency_type
-------+-------------+-----------------
     0 | user        | cascade
     1 | post        | cascade
     2 | comment     | cascade
     3 | notification| cascade
```

## Performance Bottleneck Identification

```sql
-- Find slowest cascades
SELECT entity,
       COUNT(*) as cascade_count,
       AVG(refresh_duration_ms) as avg_ms,
       MAX(refresh_duration_ms) as max_ms
FROM pg_tview_performance_log  -- To implement
WHERE timestamp > NOW() - INTERVAL '1 hour'
GROUP BY entity
ORDER BY avg_ms DESC;
```
```

**Acceptance Criteria**:
- [ ] Cascade analysis function implemented
- [ ] Dependency path visualization function
- [ ] Query plan output formatted
- [ ] Performance bottleneck queries documented
- [ ] Examples in documentation

---

#### Task 4.3: Cache Size Configuration
**Effort**: 3-4 hours

**New GUC Parameter**:

**File**: `src/config/mod.rs`

```rust
use pgrx::prelude::*;

/// Graph cache size (number of entries)
static mut GRAPH_CACHE_SIZE: usize = 1000;

/// Prepared statement cache size
static mut PREPARED_STMT_CACHE_SIZE: usize = 500;

pub fn init_guc() {
    unsafe {
        GucRegistry::define_int_guc(
            "pg_tviews.graph_cache_size",
            "Maximum number of entries in dependency graph cache",
            "Controls memory usage vs. performance trade-off",
            &raw mut GRAPH_CACHE_SIZE as *mut i32,
            100,     // min
            100000,  // max
            1000,    // default
            GucContext::Suset,
            GucFlags::default(),
        );

        GucRegistry::define_int_guc(
            "pg_tviews.prepared_stmt_cache_size",
            "Maximum number of cached prepared statements",
            "Higher values reduce planning overhead",
            &raw mut PREPARED_STMT_CACHE_SIZE as *mut i32,
            50,      // min
            10000,   // max
            500,     // default
            GucContext::Suset,
            GucFlags::default(),
        );
    }
}

pub fn get_graph_cache_size() -> usize {
    unsafe { GRAPH_CACHE_SIZE }
}

pub fn get_prepared_stmt_cache_size() -> usize {
    unsafe { PREPARED_STMT_CACHE_SIZE }
}
```

**File**: `src/lib.rs` (call in _PG_init)

```rust
#[pg_guard]
pub extern "C" fn _PG_init() {
    // ... existing initialization ...

    crate::config::init_guc();
}
```

**Documentation**:

**File**: `docs/reference/configuration.md` (new)

```markdown
# Configuration Parameters

## pg_tviews.graph_cache_size

**Type**: Integer
**Default**: 1000
**Range**: 100 to 100000
**Context**: Superuser (requires restart)

Controls the size of the dependency graph cache. Higher values improve performance for complex dependency chains but use more memory.

**Memory Usage**: ~100 bytes per entry
**Recommendation**:
- Small deployments (<50 TVIEWs): 500
- Medium deployments (50-200 TVIEWs): 1000 (default)
- Large deployments (>200 TVIEWs): 5000

```sql
-- Set in postgresql.conf
pg_tviews.graph_cache_size = 5000

-- Or ALTER SYSTEM
ALTER SYSTEM SET pg_tviews.graph_cache_size = 5000;
SELECT pg_reload_conf();
```

## pg_tviews.prepared_stmt_cache_size

**Type**: Integer
**Default**: 500
**Range**: 50 to 10000
**Context**: Superuser (requires restart)

Controls prepared statement cache size. Higher values reduce query planning overhead.

**Memory Usage**: ~500 bytes per entry
**Recommendation**:
- Low concurrency (<10 connections): 200
- Medium concurrency (10-50 connections): 500 (default)
- High concurrency (>50 connections): 2000

```sql
ALTER SYSTEM SET pg_tviews.prepared_stmt_cache_size = 2000;
SELECT pg_reload_conf();
```
```

**Acceptance Criteria**:
- [ ] GUC parameters registered
- [ ] Cache sizes configurable
- [ ] Configuration documented
- [ ] Memory usage estimates provided
- [ ] Recommendations by deployment size

---

#### Task 4.4: Performance Best Practices
**Effort**: 3-4 hours

**New File**: `docs/user-guides/performance-best-practices.md`

```markdown
# Performance Best Practices

## 1. Install jsonb_delta Extension

**Impact**: 1.5-3× faster cascade updates

```sql
CREATE EXTENSION jsonb_delta;
CREATE EXTENSION pg_tviews;

-- Verify
SELECT pg_tviews_check_jsonb_delta();  -- Should return true
```

## 2. Create Appropriate Indexes

**Impact**: 100-1000× faster queries

```sql
-- Always index foreign keys
CREATE INDEX idx_tv_post_fk_user ON tv_post(fk_user);

-- Index UUID columns for GraphQL
CREATE INDEX idx_tv_post_id ON tv_post(id);

-- GIN index for JSONB queries
CREATE INDEX idx_tv_post_data ON tv_post USING GIN(data);
```

## 3. Enable Statement-Level Triggers for Bulk Operations

**Impact**: 2-5× faster for bulk updates

```sql
SELECT pg_tviews_install_stmt_triggers();
```

**Use when**: Frequently updating >100 rows at once

## 4. Optimize JSONB Structure

**Impact**: Smaller storage, faster queries

```sql
-- ❌ BAD: Deeply nested, redundant data
-- This would be inside a CREATE TABLE tv_post AS SELECT ... FROM ...
jsonb_build_object(
    'post', jsonb_build_object(
        'id', tb_post.id,  -- UUID
        'title', tb_post.title,
        'author', jsonb_build_object(
            'id', tb_user.id,  -- UUID
            'name', tb_user.name,
            'email', tb_user.email,
            'address', jsonb_build_object(...)  -- Too deep!
        )
    )
)

-- ✅ GOOD: Flat structure, essential data only
-- Note: id is UUID, pk_post and fk_user are integers
jsonb_build_object(
    'id', tb_post.id,           -- UUID for GraphQL
    'title', tb_post.title,
    'userId', tb_user.id,        -- UUID for GraphQL (from JOIN)
    'userName', tb_user.name,
    'authorPk', tb_post.fk_user  -- Integer FK for cascade
)
```

## 5. Limit Dependency Depth

**Impact**: Faster cascades, easier debugging

**Recommendation**: Keep dependency chains ≤5 levels

```sql
-- Check current depth
SELECT entity, array_length(dependency_paths, 1) as max_depth
FROM pg_tview_meta
ORDER BY max_depth DESC;

-- If depth >5, consider flattening
-- Instead of: tv_a → tv_b → tv_c → tv_d → tv_e
-- Use: tv_a → tv_e (denormalize intermediate data)
```

## 6. Batch Related Updates

**Impact**: Fewer cascade operations

```sql
-- ❌ BAD: Many small transactions
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 1;
COMMIT;
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 2;
COMMIT;

-- ✅ GOOD: Single transaction
BEGIN;
  UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post IN (1, 2, 3, ...);
COMMIT;

-- Note: pk_post is integer (SERIAL), id is UUID
-- For GraphQL queries by UUID, use: WHERE tb_post.id = '<uuid>'
```

## 7. Monitor and Tune work_mem

**Impact**: Faster sorts and joins in cascade queries

```sql
-- Check current setting
SHOW work_mem;

-- Increase for complex cascades (per session)
SET work_mem = '128MB';

-- Or globally in postgresql.conf
work_mem = 128MB
```

## 8. Use Partitioning for Large TVIEWs

**Impact**: Faster queries, easier maintenance

```sql
-- Partition by date range
-- Note: Trinity pattern - pk_event is SERIAL, id is UUID
CREATE TABLE tv_event (
    pk_event BIGSERIAL,
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    created_at TIMESTAMPTZ NOT NULL,
    data JSONB
) PARTITION BY RANGE (created_at);

-- Create partitions (note: singular naming)
CREATE TABLE tv_event_2025_01 PARTITION OF tv_event
FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');

CREATE TABLE tv_event_2025_02 PARTITION OF tv_event
FOR VALUES FROM ('2025-02-01') TO ('2025-03-01');
```

## 9. Regular Maintenance

**Impact**: Consistent performance over time

```bash
# Weekly: Vacuum and analyze
vacuumdb --analyze --verbose dbname

# Monthly: Reindex if needed
psql -c "REINDEX DATABASE dbname"

# Check for bloat
SELECT schemaname, tablename,
       pg_size_pretty(pg_relation_size(schemaname||'.'||tablename)) as size
FROM pg_tables
WHERE tablename LIKE 'tv_%'
ORDER BY pg_relation_size(schemaname||'.'||tablename) DESC;
```

## 10. Profile Before Optimizing

**Impact**: Focus effort on actual bottlenecks

```sql
-- Enable timing
\timing on

-- Profile a cascade
-- Note: pk_post is integer (SERIAL), id is UUID
EXPLAIN (ANALYZE, BUFFERS)
UPDATE tb_post SET title = 'New' WHERE tb_post.pk_post = 1;

-- Or by UUID for GraphQL
EXPLAIN (ANALYZE, BUFFERS)
UPDATE tb_post SET title = 'New' WHERE tb_post.id = 'uuid-here';

-- Check slow queries
SELECT query, mean_exec_time, calls
FROM pg_stat_statements
WHERE query LIKE '%tv_%'
ORDER BY mean_exec_time DESC
LIMIT 10;
```

## Performance Checklist

Before going to production:
- [ ] jsonb_delta extension installed
- [ ] Indexes created on pk_*, fk_*, id columns
- [ ] GIN indexes on data columns (for TVIEWs >10K rows)
- [ ] Statement-level triggers enabled (for bulk operations)
- [ ] Dependency depth ≤5 levels
- [ ] work_mem tuned (≥64MB for medium deployments)
- [ ] Autovacuum configured
- [ ] Monitoring enabled (pg_stat_statements)
```

**Acceptance Criteria**:
- [ ] 10 best practices documented
- [ ] Performance impact quantified for each
- [ ] Code examples provided
- [ ] Performance checklist included
- [ ] Profiling tools explained

---

### Phase 4 Acceptance Criteria

- [ ] Index optimization guide with recommendations
- [ ] Query plan analysis tools implemented
- [ ] Cache configuration parameters added
- [ ] Performance best practices documented
- [ ] Tuning utilities created and documented
- [ ] Performance score: 95/100 ✅

---

## Timeline and Milestones

### Week 1-2: Documentation Excellence (Phase 1)
- Fix unqualified column references
- Standardize examples
- Create migration guides
- Add security documentation

**Milestone**: Documentation score 95/100 ✅

### Week 3-4: Testing & Quality (Phase 2)
- Fix test build issues
- Add concurrent DDL tests
- Implement stress tests
- Improve test assertions

**Milestone**: Testing score 95/100 ✅

### Week 5: Production Readiness (Phase 3)
- Complete monitoring infrastructure
- Create operational runbooks
- Implement audit logging
- Document disaster recovery

**Milestone**: Production Readiness score 98/100 ✅

### Week 6: Performance & Optimization (Phase 4)
- Index optimization guide
- Query analysis tools
- Cache configuration
- Best practices documentation

**Milestone**: Performance score 95/100 ✅

---

## Success Criteria

### Target Scores

| Category | Current | Target | Status |
|----------|---------|--------|--------|
| Code Correctness | 92/100 | 98/100 | Phase 2 |
| Architecture | 90/100 | 96/100 | Phase 3 |
| Documentation | 85/100 | 95/100 | **Phase 1** |
| Testing | 82/100 | 95/100 | **Phase 2** |
| Performance | 88/100 | 95/100 | Phase 4 |
| Production Ready | 84/100 | 98/100 | **Phase 3** |
| **OVERALL** | **87/100** | **95/100** | All Phases |

### Definition of "Excellent"

Each category achieves 95-100/100 when:

**Code Correctness (98/100)**:
- 0 P0/P1 issues
- <5 TODO comments
- Test build works with all configurations
- No panics in any code path
- 90%+ code coverage

**Architecture (96/100)**:
- All monitoring implemented
- Audit logging complete
- Resource limits documented
- Disaster recovery tested

**Documentation (95/100)**:
- All examples verified working
- Security guide complete
- Migration guides tested
- API reference 100% complete

**Testing (95/100)**:
- Concurrent tests passing
- 1M+ row stress tests
- 85%+ code coverage
- All edge cases covered

**Performance (95/100)**:
- Optimization guide complete
- Analysis tools available
- Configuration documented
- Best practices established

**Production Readiness (98/100)**:
- Monitoring complete
- Runbooks tested
- Audit trail enabled
- Recovery procedures verified

---

## Risk Assessment

### Low Risk Tasks
- Documentation fixes (Phase 1)
- Test improvements (Phase 2)
- Best practices guides (Phase 4)

### Medium Risk Tasks
- Monitoring implementation (Phase 3)
- Stress tests (Phase 2)
- Query analysis tools (Phase 4)

### High Risk Tasks
- None identified (all tasks are enhancements, not rewrites)

### Mitigation Strategies
- Incremental changes with frequent testing
- Feature flags for new functionality
- Comprehensive rollback procedures
- Beta testing period before 1.0 release

---

## Post-Excellence Maintenance

After achieving 95/100 across all categories:

### Monthly
- Review and resolve new TODOs
- Update documentation for API changes
- Run stress tests on new hardware

### Quarterly
- Security audit
- Performance benchmarking
- Disaster recovery drill
- Dependency updates

### Annually
- Comprehensive QA re-assessment
- Capacity planning review
- Architectural review

---

**End of Excellence Roadmap**

*This roadmap will evolve as pg_tviews matures. Adjust priorities based on user feedback and production needs.*
