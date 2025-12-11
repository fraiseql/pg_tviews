# Phase 2: Testing & Quality Assurance

**Goal**: 82/100 → 95/100
**Effort**: 25-35 hours
**Priority**: High

> **⚠️ TRINITY PATTERN REQUIRED**: All test SQL examples MUST follow the trinity pattern.
> **See**: [00-TRINITY-PATTERN-REFERENCE.md](./00-TRINITY-PATTERN-REFERENCE.md)
>
> **Test Data Pattern**:
> ```sql
> CREATE TABLE tb_test (
>   pk_test SERIAL PRIMARY KEY,    -- INTEGER
>   id UUID NOT NULL,               -- UUID
>   fk_parent INTEGER,              -- INTEGER FK
>   ...
> );
> ```

---


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


---

**Previous Phase**: [01-documentation-excellence.md](./01-documentation-excellence.md)
**Next Phase**: [03-production-readiness.md](./03-production-readiness.md)
