# Phase Integration Tests

This directory contains comprehensive integration tests for all implemented phases of the pg_tviews todo fixes.

## Test Coverage

### Phase 1: Savepoint Depth Tracking
- **File**: `phase_1_savepoint_depth_integration.sql`
- **Tests**: Transaction nesting, savepoint depth calculation, queue persistence across rollbacks
- **Key Features**: `get_savepoint_depth()` function, savepoint-aware queue handling

### Phase 2: GUC Configuration System
- **File**: `phase_2_guc_configuration_integration.sql`
- **Tests**: Runtime configuration via GUC variables, propagation depth limits, cache settings
- **Key Features**: `pg_tviews.max_propagation_depth`, `pg_tviews.graph_cache_enabled`, etc.

### Phase 3: Queue Introspection
- **File**: `phase_3_queue_introspection_integration.sql`
- **Tests**: Real-time queue monitoring, transaction queue status, monitoring view accuracy
- **Key Features**: `pg_tviews_queue_info()` function, `pg_tviews_queue_realtime` view

### Phase 4: Dynamic Primary Key Detection
- **File**: `phase_4_dynamic_pk_detection_integration.sql`
- **Tests**: Automatic entity detection, PK column resolution, multi-entity trigger support
- **Key Features**: `extract_pk(entity)` function, `derive_entity_from_table()` helper

### Phase 5: Cached Plan Refresh Integration
- **File**: `phase_5_cached_plan_refresh_integration.sql`
- **Tests**: Cached vs uncached refresh performance, cache invalidation, fallback logic
- **Key Features**: `refresh_entity_pk()` dispatcher, prepared statement caching

### Phase 6: TEXT[][] Extraction Workaround
- **File**: `phase_6_text_array_extraction_integration.sql`
- **Tests**: Dependency path extraction, nested JSONB refresh, complex relationships
- **Key Features**: `extract_text_2d_array()` workaround, nested object/array updates

## Running the Tests

### Prerequisites

1. **PostgreSQL Extension**: pg_tviews must be installed and loaded
2. **Test Database**: A PostgreSQL database for testing
3. **psql**: PostgreSQL client for running SQL scripts

### Quick Start

```bash
# Run all integration tests
./test/run_all_phase_integration_tests.sh

# Run individual phase tests
psql -f test/sql/phase_1_savepoint_depth_integration.sql
psql -f test/sql/phase_2_guc_configuration_integration.sql
# ... etc
```

### Manual Testing

Each test file can be run individually:

```bash
# Test savepoint depth tracking
psql -f test/sql/phase_1_savepoint_depth_integration.sql

# Test GUC configuration
psql -f test/sql/phase_2_guc_configuration_integration.sql

# Test queue introspection
psql -f test/sql/phase_3_queue_introspection_integration.sql

# Test dynamic PK detection
psql -f test/sql/phase_4_dynamic_pk_detection_integration.sql

# Test cached refresh
psql -f test/sql/phase_5_cached_plan_refresh_integration.sql

# Test TEXT[][] extraction
psql -f test/sql/phase_6_text_array_extraction_integration.sql
```

## Test Results Interpretation

### ✅ PASS Criteria

- **Phase 1**: Savepoint depth correctly tracked, queue persists/restores across rollbacks
- **Phase 2**: GUC variables work, configuration affects behavior, settings persist
- **Phase 3**: Queue statistics accurate, monitoring view shows real data
- **Phase 4**: PK detection works for `tb_<entity>` tables, triggers fire correctly
- **Phase 5**: Cached refresh faster than uncached, fallback works on cache miss
- **Phase 6**: Dependency paths extracted, nested JSONB refresh works correctly

### Expected Output

Each test should complete without errors and show expected data in TVIEW tables. The automated runner will report:

```
🧪 Starting Comprehensive Integration Tests for All Phases
==========================================================
✅ PASSED: Savepoint Depth Integration
✅ PASSED: GUC Configuration Integration
✅ PASSED: Queue Introspection Integration
✅ PASSED: Dynamic PK Detection Integration
✅ PASSED: Cached Plan Refresh Integration
✅ PASSED: TEXT[][] Extraction Integration
==========================================================
🎉 ALL INTEGRATION TESTS PASSED!
```

## Troubleshooting

### Common Issues

1. **Extension Not Loaded**
   ```sql
   CREATE EXTENSION IF NOT EXISTS pg_tviews;
   SELECT pg_tviews_health_check();
   ```

2. **Permission Issues**
   ```sql
   -- Ensure test user has necessary permissions
   GRANT ALL ON SCHEMA public TO test_user;
   ```

3. **Cleanup Between Runs**
   ```sql
   -- Clean up any leftover test tables
   DROP TABLE IF EXISTS tb_* CASCADE;
   SELECT pg_tviews_drop('test_entity');
   ```

4. **Performance Variations**
   - Cached refresh performance may vary based on system load
   - Use `\timing on` in psql to measure actual performance differences

### Debug Mode

Run tests with verbose output:
```bash
psql -f test/sql/phase_1_savepoint_depth_integration.sql -e
```

## Test Maintenance

When adding new phases or modifying existing functionality:

1. **Update Tests**: Add corresponding integration tests
2. **Update Runner**: Add new phase to `run_all_phase_integration_tests.sh`
3. **Update README**: Document new test coverage
4. **Verify Compatibility**: Ensure tests work with existing functionality

## Performance Benchmarks

For performance-critical phases (especially Phase 5), the tests include timing measurements. Compare results across runs to ensure performance improvements are maintained.

## CI/CD Integration

These tests can be integrated into CI/CD pipelines:

```yaml
# Example GitHub Actions step
- name: Run Integration Tests
  run: |
    ./test/run_all_phase_integration_tests.sh
  env:
    PGHOST: localhost
    PGUSER: postgres
    PGPASSWORD: test_password
```