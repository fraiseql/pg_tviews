# pg_tviews Failure Modes Test Suite

This directory contains comprehensive tests for failure modes and recovery procedures in pg_tviews.

## Test Categories

### Database Failures (`db-failures/`)
Tests for PostgreSQL-level failures:
- **Crash Recovery**: PostgreSQL crash during TVIEW refresh
- **Disk Full**: Out of disk space during operations
- **Out of Memory**: Memory exhaustion scenarios

### Extension Failures (`extension-failures/`)
Tests for pg_tviews-specific failures:
- **Circular Dependencies**: TVIEW dependency cycles
- **Metadata Corruption**: Missing or damaged metadata
- **Queue Corruption**: Orphaned queue persistence entries

### Operational Failures (`operational/`)
Tests for operational scenarios:
- **PostgreSQL Upgrade**: Major version upgrades
- **Backup/Restore**: Logical backup and restore
- **Concurrent DDL**: DDL operations during refresh

## Running Tests

### Run All Tests
```bash
cd test/failure-modes
./run_all_tests.sh
```

### Run Specific Category
```bash
# Database failures
./db-failures/test-crash-recovery.sh
./db-failures/test-disk-full.sh
./db-failures/test-oom.sh

# Extension failures
./extension-failures/test-circular-deps.sh
./extension-failures/test-metadata-corruption.sh
./extension-failures/test-queue-corruption.sh

# Operational failures
./operational/test-upgrade.sh
./operational/test-backup-restore.sh
./operational/test-concurrent-ddl.sh
```

### Test Prerequisites

- PostgreSQL running with pg_tviews installed
- sudo access for system-level operations (crash simulation, disk mounting)
- Sufficient disk space for test databases

## Test Libraries

### `lib/simulate-failure.sh`
Utility functions for simulating various failure conditions:
- `simulate_pg_crash()`: Restart PostgreSQL to simulate crash
- `simulate_disk_full()`: Create small tmpfs to simulate disk full
- `simulate_network_partition()`: Kill active connections

### `lib/verify-recovery.sh`
Functions for verifying system recovery:
- `verify_tview_integrity()`: Check TVIEW structure and metadata
- `verify_tview_refresh()`: Test that refresh still works
- `verify_queue_clean()`: Ensure no orphaned queue entries
- `verify_full_recovery()`: Comprehensive recovery verification

## Expected Behavior

### Test Results
- **PASS**: Test completed successfully, recovery worked as expected
- **Expected Failure**: Test induced failure that was handled gracefully
- **Unexpected Failure**: Test revealed a bug or unhandled failure mode

### Data Consistency
All tests verify that:
- No data corruption occurs
- TVIEW remains consistent with backing table
- Queue is properly cleaned up
- Extension functionality is preserved

## Adding New Tests

1. Create test script in appropriate category directory
2. Follow naming convention: `test-<failure-type>.sh`
3. Include setup, failure simulation, and recovery verification
4. Make script executable: `chmod +x test-name.sh`
5. Update this README if adding new categories

## Troubleshooting

### Common Issues

**Permission Denied**: Tests requiring sudo may fail in restricted environments
**PostgreSQL Not Starting**: Check PostgreSQL logs after crash simulation
**Disk Full Not Triggered**: Test may need adjustment for available RAM

### Debug Mode
Run tests with verbose output:
```bash
bash -x ./test-name.sh
```

## Related Documentation

- [FAILURE_MODES.md](../../docs/operations/FAILURE_MODES.md) - User-facing failure documentation
- [TROUBLESHOOTING.md](../../docs/operations/TROUBLESHOOTING.md) - General troubleshooting guide