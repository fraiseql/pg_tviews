# Testing Guide

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

## Overview

This guide covers running tests, measuring code coverage, and contributing test improvements to pg_tviews.

## Test Types

### 1. Rust Unit Tests (`cargo test --lib`)

- **Location**: `src/**/*.rs` (inline `#[test]` functions)
- **Purpose**: Test individual functions and modules
- **Coverage**: Core business logic, data structures, algorithms
- **Runtime**: Fast (< 1 second)

### 2. pgrx Integration Tests (`cargo pgrx test pg17`)

- **Location**: `src/**/*.rs` (inline `#[pg_test]` functions)
- **Purpose**: Test PostgreSQL extension functionality
- **Coverage**: SQL interactions, extension loading, triggers
- **Runtime**: Medium (10-30 seconds)

### 3. SQL Integration Tests (`psql -f test/sql/*.sql`)

- **Location**: `test/sql/*.sql`
- **Purpose**: End-to-end testing with real PostgreSQL
- **Coverage**: Complete workflows, performance, edge cases
- **Runtime**: Slow (1-5 minutes)

## Running Tests

### Quick Test Commands

```bash
# Run all Rust unit tests (fast)
cargo test --lib

# Run pgrx integration tests (requires PostgreSQL)
cargo pgrx test pg17

# Run specific test
cargo pgrx test pg17 -- --test test_refresh_single_row

# Run SQL integration tests
psql -d test_db -f test/sql/40_refresh_trigger_dynamic_pk.sql

# Run all SQL tests
for file in test/sql/*.sql; do
    echo "Running $file..."
    psql -d test_db -f "$file"
done
```

### Test Database Setup

```bash
# Create test database
createdb pg_tviews_test

# Enable extensions
psql -d pg_tviews_test -c "CREATE EXTENSION jsonb_delta;"
psql -d pg_tviews_test -c "CREATE EXTENSION pg_tviews;"

# Run tests
psql -d pg_tviews_test -f test/sql/40_refresh_trigger_dynamic_pk.sql
```

## Code Coverage

### Setup Coverage Tools

```bash
# Install LLVM coverage tools
cargo install cargo-llvm-cov

# Verify installation
cargo llvm-cov --version
```

### Generate Coverage Reports

```bash
# Generate HTML report
cargo llvm-cov --html --open

# Generate LCOV for CI/CD
cargo llvm-cov --lcov --output-path coverage.lcov

# Include integration tests
cargo llvm-cov --features pg_test --html
```

### Coverage Targets by Module

| Module | Current Coverage | Target | Priority |
|--------|------------------|--------|----------|
| `src/refresh/main.rs` | TBD% | 85% | High |
| `src/ddl/create.rs` | TBD% | 90% | High |
| `src/ddl/drop.rs` | TBD% | 90% | High |
| `src/dependency/graph.rs` | TBD% | 80% | Medium |
| `src/schema/types.rs` | TBD% | 75% | Medium |
| `src/metadata.rs` | TBD% | 80% | Medium |
| `src/hooks.rs` | TBD% | 70% | Low |

### Coverage Goals

- **Overall Target**: 85% line coverage
- **Critical Path**: 90%+ coverage for DDL and refresh operations
- **Integration Tests**: Cover all SQL test scenarios
- **Edge Cases**: Cover error conditions and boundary cases

## CI/CD Integration

### GitHub Actions Workflow

```yaml
# .github/workflows/coverage.yml
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

### Coverage Badge

Add to README.md:
```markdown
[![codecov](https://codecov.io/gh/fraiseql/pg_tviews/branch/main/graph/badge.svg)](https://codecov.io/gh/fraiseql/pg_tviews)
```

## Test Organization

### File Naming Convention

```
test/sql/
├── 00_extension_loading.sql      # Basic extension tests
├── 10_schema_inference_*.sql     # Schema analysis tests
├── 40_refresh_trigger_*.sql      # Trigger and refresh tests
├── 42_cascade_fk_*.sql           # Cascade functionality
├── 50_array_columns.sql          # Array column handling
├── 60_2pc_support.sql            # Two-phase commit
├── 70_concurrent_ddl.sql         # Concurrent operations
└── 80_edge_cases.sql             # Edge cases and error handling
```

### Test Structure Pattern

```sql
-- Test header
-- Test [NUMBER]: [DESCRIPTION]
-- Purpose: [WHAT IT TESTS]
-- Expected: [EXPECTED BEHAVIOR]

-- Setup
BEGIN;
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;
-- ... setup code ...

-- Test execution
-- ... test code ...

-- Assertions (return boolean values)
SELECT COUNT(*) = expected_value as test_condition;

-- Cleanup
ROLLBACK;
```

## Debugging Test Failures

### Common Issues

#### Extension Not Loaded
```sql
-- Check extension status
SELECT * FROM pg_extension WHERE extname = 'pg_tviews';

-- Reload if needed
DROP EXTENSION pg_tviews;
CREATE EXTENSION pg_tviews;
```

#### Permission Issues
```sql
-- Grant test permissions
GRANT ALL ON ALL TABLES IN SCHEMA public TO pg_tviews_test_user;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO pg_tviews_test_user;
```

#### Trigger Issues
```sql
-- Check trigger status
SELECT tgname, tgenabled FROM pg_trigger WHERE tgname LIKE '%tview%';

-- Reinstall triggers
SELECT pg_tviews_install_stmt_triggers();
```

### Test Debugging Tools

```sql
-- Enable detailed logging
ALTER SYSTEM SET log_statement = 'all';
ALTER SYSTEM SET log_min_messages = 'debug1';

-- Check PostgreSQL logs
tail -f /var/log/postgresql/postgresql-*.log

-- Debug specific test
cargo pgrx test pg17 -- --nocapture --test test_name
```

## Performance Testing

### Benchmark Tests

```bash
# Run performance benchmarks
cd test/sql/comprehensive_benchmarks
./run_benchmarks.sh

# Analyze results
python3 generate_report.py
```

### Profiling Tests

```sql
-- Enable query profiling
SET track_functions = 'all';
SET track_io_timing = 'on';

-- Run test with profiling
-- Check pg_stat_statements for slow queries
SELECT query, calls, total_time, mean_time
FROM pg_stat_statements
WHERE query LIKE '%tv_%'
ORDER BY total_time DESC;
```

## Contributing Tests

### Adding New Tests

1. **Choose appropriate location**:
   - Rust unit tests: Add `#[test]` to existing modules
   - pgrx tests: Add `#[cfg(any(test, feature = "pg_test"))] #[pg_test]`
   - SQL tests: Add to `test/sql/` with numbered filename

2. **Follow naming conventions**:
   - Functions: `test_descriptive_name`
   - Files: `NN_descriptive_name.sql`

3. **Include assertions**:
   - Return boolean values for pass/fail
   - Test both success and error cases
   - Verify data correctness, not just absence of errors

4. **Add documentation**:
   - Comment test purpose and expectations
   - Update this guide if adding new test types

### Test Maintenance

- **Keep tests fast**: Split slow tests into separate files
- **Update on API changes**: Fix tests when functionality changes
- **Remove obsolete tests**: Delete tests for removed features
- **Regular review**: Audit test coverage quarterly

## Troubleshooting

### Test Fails Intermittently

1. Check for race conditions in concurrent tests
2. Verify test isolation (each test should be independent)
3. Check for external dependencies (network, filesystem)

### Coverage Not Updating

1. Ensure `cargo-llvm-cov` is installed
2. Check that tests are running with coverage enabled
3. Verify source files are included in coverage analysis

### CI/CD Coverage Issues

1. Check GitHub Actions logs for coverage upload failures
2. Verify LCOV file is generated correctly
3. Ensure Codecov token is configured (if private repo)

## See Also

- [Development Guide](../DEVELOPMENT.md) - General development setup
- [Performance Tuning](../operations/performance-tuning.md) - Performance testing
- [Troubleshooting](../operations/troubleshooting.md) - Debugging production issues