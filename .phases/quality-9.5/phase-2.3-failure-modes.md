# Phase 2.3: Failure Mode Analysis

**Objective**: Identify, document, and test all failure modes and recovery procedures

**Priority**: HIGH
**Estimated Time**: 1-2 days
**Blockers**: Phase 2.1, 2.2 complete

---

## Context

**Current State**: Limited documentation of failure scenarios and recovery

**Why This Matters**:
- Production systems fail in unexpected ways
- Users need clear recovery procedures
- Failure modes should degrade gracefully, not corrupt data
- Documentation prevents panic during incidents

**Deliverable**: Comprehensive failure mode documentation with tested recovery procedures

---

## Failure Modes to Analyze

### Category 1: Database Failures

1. **PostgreSQL crash during refresh**
   - Mid-transaction crash
   - During 2PC prepare
   - During cascade refresh

2. **Disk full during refresh**
   - WAL writes fail
   - Table writes fail

3. **Out of memory**
   - Large TVIEW refresh
   - Deep dependency chain

4. **Connection loss**
   - Client disconnects mid-transaction
   - Network partition

### Category 2: Extension Failures

5. **Circular dependency detected**
   - User creates circular TVIEW dependencies
   - Recovery: Break cycle

6. **Metadata corruption**
   - `pg_tviews_metadata` table damaged
   - Missing dependency entries

7. **Queue persistence corruption**
   - Orphaned queue entries
   - Duplicate entries

8. **Trigger malfunction**
   - Trigger disabled/dropped
   - Trigger fires recursively

### Category 3: Operational Failures

9. **PostgreSQL upgrade**
   - Major version upgrade (15→16→17)
   - Extension version mismatch

10. **Backup/restore**
    - TVIEW state not restored
    - Backing table missing after restore

11. **Replication lag**
    - Replica has stale TVIEW data
    - Cascade refresh on replica

12. **Concurrent DDL**
    - DROP TVIEW during refresh
    - ALTER TABLE during refresh

---

## Implementation Steps

### Step 1: Create Failure Test Suite

**Create**: `test/failure-modes/`

```
test/failure-modes/
├── README.md
├── db-failures/
│   ├── test-crash-recovery.sh
│   ├── test-disk-full.sh
│   └── test-oom.sh
├── extension-failures/
│   ├── test-circular-deps.sh
│   ├── test-metadata-corruption.sh
│   └── test-queue-corruption.sh
├── operational/
│   ├── test-upgrade.sh
│   ├── test-backup-restore.sh
│   └── test-concurrent-ddl.sh
└── lib/
    ├── simulate-failure.sh
    └── verify-recovery.sh
```

### Step 2: Database Failure Tests

**Create**: `test/failure-modes/db-failures/test-crash-recovery.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing crash recovery..."

# Setup
psql <<EOF
CREATE TABLE tb_crash_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_crash_test AS SELECT pk_test, data FROM tb_crash_test;
SELECT pg_tviews_convert_existing_table('tv_crash_test');
EOF

# Insert some data
psql -c "INSERT INTO tb_crash_test (data) SELECT 'row-' || i FROM generate_series(1, 1000) i;"

echo "Simulating crash during refresh..."

# Start a long-running refresh in background
psql <<EOF &
BEGIN;
INSERT INTO tb_crash_test (data) VALUES ('crash-test');
-- Simulate long-running transaction
SELECT pg_sleep(5);
COMMIT;
EOF

# Wait a bit, then simulate crash
sleep 2
sudo systemctl restart postgresql

# Wait for PostgreSQL to come back up
sleep 5
until pg_isready; do sleep 1; done

echo "Checking recovery..."

# Verify TVIEW is consistent
BACKING_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tb_crash_test;")
TVIEW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_crash_test;")

echo "Backing table: $BACKING_COUNT rows"
echo "TVIEW: $TVIEW_COUNT rows"

# After recovery, TVIEW should be consistent
# The crashed transaction should have rolled back
if [ "$TVIEW_COUNT" -eq 1000 ]; then
    echo "✅ PASS: TVIEW recovered correctly after crash"
else
    echo "❌ FAIL: TVIEW in inconsistent state"
    exit 1
fi

# Check for orphaned queue entries
QUEUE_SIZE=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_get_queue();")
if [ "$QUEUE_SIZE" -eq 0 ]; then
    echo "✅ PASS: No orphaned queue entries"
else
    echo "⚠️  WARNING: Queue has $QUEUE_SIZE orphaned entries"
fi

echo "✅ Crash recovery test passed"
```

**Create**: `test/failure-modes/db-failures/test-disk-full.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing disk full scenario..."

# Create a small tmpfs to simulate disk full
sudo mkdir -p /tmp/pg_test_small
sudo mount -t tmpfs -o size=100M tmpfs /tmp/pg_test_small

# Create a test tablespace
psql <<EOF
CREATE TABLESPACE test_small LOCATION '/tmp/pg_test_small';
CREATE TABLE tb_disk_full (pk_test SERIAL PRIMARY KEY, data TEXT) TABLESPACE test_small;
CREATE TABLE tv_disk_full AS SELECT pk_test, data FROM tb_disk_full;
SELECT pg_tviews_convert_existing_table('tv_disk_full');
EOF

echo "Filling disk..."

# Try to insert until disk is full
set +e
psql <<EOF
INSERT INTO tb_disk_full (data)
SELECT repeat('x', 10000)
FROM generate_series(1, 100000);
EOF
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ Expected failure: Disk full"
else
    echo "⚠️  Disk didn't fill (test may not be valid)"
fi

echo "Checking TVIEW consistency after disk full..."

# Verify TVIEW is still accessible
TVIEW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_disk_full;" || echo "ERROR")

if [ "$TVIEW_COUNT" != "ERROR" ]; then
    echo "✅ PASS: TVIEW remains accessible after disk full"
else
    echo "❌ FAIL: TVIEW corrupted"
    exit 1
fi

# Cleanup
sudo umount /tmp/pg_test_small
sudo rm -rf /tmp/pg_test_small

echo "✅ Disk full test passed"
```

### Step 3: Extension Failure Tests

**Create**: `test/failure-modes/extension-failures/test-circular-deps.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing circular dependency detection..."

# Setup
psql <<EOF
CREATE TABLE tb_a (pk_a SERIAL PRIMARY KEY, fk_b INT, data TEXT);
CREATE TABLE tb_b (pk_b SERIAL PRIMARY KEY, fk_a INT, data TEXT);

-- Create tv_a that references tb_b
CREATE TABLE tv_a AS
SELECT a.pk_a, a.data, b.data as b_data
FROM tb_a a
LEFT JOIN tb_b b ON a.fk_b = b.pk_b;

SELECT pg_tviews_convert_existing_table('tv_a');

-- Try to create tv_b that references tv_a (creates cycle)
CREATE TABLE tv_b AS
SELECT b.pk_b, b.data, a.data as a_data
FROM tb_b b
LEFT JOIN tv_a a ON b.fk_a = a.pk_a;
EOF

echo "Attempting to create circular dependency..."

set +e
psql -c "SELECT pg_tviews_convert_existing_table('tv_b');" 2>&1 | tee /tmp/circular-error.txt
RESULT=$?
set -e

if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Circular dependency detected and prevented"
    grep -q "circular" /tmp/circular-error.txt && echo "✅ Error message mentions 'circular'"
else
    echo "❌ FAIL: Circular dependency allowed"
    exit 1
fi

# Verify tv_a still works
psql -c "SELECT COUNT(*) FROM tv_a;" > /dev/null
echo "✅ PASS: Existing TVIEW (tv_a) still functional"

echo "✅ Circular dependency test passed"
```

**Create**: `test/failure-modes/extension-failures/test-metadata-corruption.sh`

```bash
#!/bin/bash
set -euo pipefail

echo "Testing metadata corruption recovery..."

# Setup
psql <<EOF
CREATE TABLE tb_meta_test (pk_test SERIAL PRIMARY KEY, data TEXT);
CREATE TABLE tv_meta_test AS SELECT pk_test, data FROM tb_meta_test;
SELECT pg_tviews_convert_existing_table('tv_meta_test');
INSERT INTO tb_meta_test (data) VALUES ('test-1'), ('test-2');
EOF

echo "Simulating metadata corruption..."

# Corrupt metadata (delete entry)
psql -c "DELETE FROM pg_tviews_metadata WHERE entity_name = 'tv_meta_test';"

echo "Attempting refresh with corrupted metadata..."

set +e
psql -c "INSERT INTO tb_meta_test (data) VALUES ('test-3');" 2>&1 | tee /tmp/metadata-error.txt
RESULT=$?
set -e

# Should fail gracefully
if [ $RESULT -ne 0 ]; then
    echo "✅ PASS: Graceful failure on missing metadata"
    grep -qi "metadata not found" /tmp/metadata-error.txt && echo "✅ Clear error message"
else
    echo "⚠️  WARNING: Operation succeeded despite missing metadata"
fi

echo "Testing metadata recovery..."

# Re-convert to fix metadata
psql -c "SELECT pg_tviews_convert_existing_table('tv_meta_test');"

# Verify recovery
psql -c "INSERT INTO tb_meta_test (data) VALUES ('test-4');"
COUNT=$(psql -tAc "SELECT COUNT(*) FROM tv_meta_test WHERE data = 'test-4';")

if [ "$COUNT" -eq 1 ]; then
    echo "✅ PASS: Metadata recovered, refresh working"
else
    echo "❌ FAIL: Recovery unsuccessful"
    exit 1
fi

echo "✅ Metadata corruption test passed"
```

### Step 4: Create Failure Mode Documentation

**Create**: `docs/operations/FAILURE_MODES.md`

```markdown
# Failure Modes and Recovery Procedures

## Database Failures

### PostgreSQL Crash During Refresh

**Symptoms**:
- Transaction in progress when PostgreSQL crashes
- TVIEW may be out of sync with backing table

**Recovery**:
1. PostgreSQL will automatically roll back uncommitted transactions
2. TVIEW will be consistent with pre-crash state
3. Re-run refresh if needed:
   ```sql
   SELECT pg_tviews_refresh('entity_name');
   ```

**Prevention**: Use 2PC for critical transactions requiring atomicity across systems.

### Disk Full

**Symptoms**:
- `ERROR: could not extend file` messages
- Transactions fail

**Recovery**:
1. Free up disk space
2. Check TVIEW consistency:
   ```sql
   -- Compare row counts
   SELECT COUNT(*) FROM backing_table;
   SELECT COUNT(*) FROM tview_table;
   ```
3. If inconsistent, force refresh:
   ```sql
   SELECT pg_tviews_refresh('entity_name', force => true);
   ```

**Prevention**: Monitor disk usage, set up alerts at 80% capacity.

### Out of Memory

**Symptoms**:
- `ERROR: out of memory` during large refresh
- PostgreSQL may restart

**Recovery**:
1. Increase `work_mem` for session:
   ```sql
   SET work_mem = '256MB';
   SELECT pg_tviews_refresh('large_entity');
   RESET work_mem;
   ```
2. Consider batch refresh instead of full refresh

**Prevention**: Use incremental refresh, avoid `force => true` on large TVIEWs.

---

## Extension Failures

### Circular Dependency

**Symptoms**:
```
ERROR: Circular dependency detected: tv_a -> tv_b -> tv_a
```

**Recovery**:
1. Identify cycle in dependencies
2. Break cycle by dropping one TVIEW:
   ```sql
   DROP TABLE tv_b CASCADE;
   ```
3. Recreate without circular reference

**Prevention**: Design TVIEW dependency graph as DAG (directed acyclic graph).

### Metadata Corruption

**Symptoms**:
- `ERROR: Metadata not found for TVIEW: entity_name`
- Triggers exist but no metadata entry

**Recovery**:
1. Re-convert TVIEW:
   ```sql
   SELECT pg_tviews_convert_existing_table('entity_name');
   ```
2. Verify metadata:
   ```sql
   SELECT * FROM pg_tviews_metadata WHERE entity_name = 'entity_name';
   ```

**Prevention**: Do not manually modify `pg_tviews_metadata` table.

### Orphaned Queue Entries

**Symptoms**:
- Queue persistence table has old entries
- 2PC transactions never committed/rolled back

**Recovery**:
1. List orphaned entries:
   ```sql
   SELECT gid, prepared FROM pg_tview_queue_persistence
   WHERE age(now(), prepared) > interval '1 hour';
   ```
2. Manually clean up:
   ```sql
   DELETE FROM pg_tview_queue_persistence
   WHERE gid = 'orphaned_transaction_id';
   ```

**Prevention**: Always commit or rollback prepared transactions promptly.

---

## Operational Failures

### PostgreSQL Version Upgrade

**Procedure**:
1. Before upgrade:
   ```bash
   # Backup
   pg_dump mydb > backup.sql

   # Note pg_tviews version
   psql -c "SELECT pg_tviews_version();"
   ```

2. Upgrade PostgreSQL:
   ```bash
   # Standard PostgreSQL upgrade procedure
   pg_upgrade ...
   ```

3. Reinstall pg_tviews:
   ```bash
   cargo pgrx install --release --pg-config=/path/to/new/pg_config
   ```

4. Verify:
   ```sql
   SELECT pg_tviews_version();
   SELECT entity_name, COUNT(*) FROM pg_tviews_metadata;
   ```

**Known Issues**: Extension must be reinstalled after major PostgreSQL upgrades.

### Backup and Restore

**Backup** (recommended):
```bash
# Full logical backup includes TVIEW definition and data
pg_dump -Fc mydb > mydb.dump
```

**Restore**:
```bash
pg_restore -d mydb_restored mydb.dump
```

**Verification**:
```sql
-- Check all TVIEWs restored
SELECT entity_name FROM pg_tviews_metadata;

-- Verify refresh works
INSERT INTO backing_table VALUES (...);
-- Check TVIEW updated
```

**Caution**: Physical backups (PITR) may have consistency issues if taken during refresh.

### Concurrent DDL

**Scenario**: `DROP TABLE tv_entity` while refresh in progress

**Behavior**:
- Refresh transaction will fail
- Error message: `relation "tv_entity" does not exist`
- No data corruption

**Recovery**: None needed - transaction rolled back cleanly.

**Prevention**: Use DDL locks or maintenance windows for TVIEW DDL.

---

## Emergency Procedures

### Force Refresh All TVIEWs

```sql
-- Refresh all TVIEWs (use with caution on large databases)
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
        RAISE NOTICE 'Refreshing %', rec.entity_name;
        PERFORM pg_tviews_refresh(rec.entity_name, force => true);
    END LOOP;
END $$;
```

### Disable All TVIEW Triggers (Emergency)

```sql
-- Disable refresh triggers (stops automatic refresh)
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN
        SELECT DISTINCT trigger_name, event_object_table
        FROM information_schema.triggers
        WHERE trigger_name LIKE 'pg_tviews_%'
    LOOP
        EXECUTE format('ALTER TABLE %I DISABLE TRIGGER %I',
                      rec.event_object_table, rec.trigger_name);
    END LOOP;
END $$;
```

### Re-enable Triggers

```sql
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN
        SELECT DISTINCT trigger_name, event_object_table
        FROM information_schema.triggers
        WHERE trigger_name LIKE 'pg_tviews_%'
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE TRIGGER %I',
                      rec.event_object_table, rec.trigger_name);
    END LOOP;
END $$;
```

---

## Monitoring and Alerts

### Key Metrics to Monitor

1. **Queue size**: Should be 0 between transactions
   ```sql
   SELECT COUNT(*) FROM pg_tviews_get_queue();
   ```

2. **Orphaned prepared transactions**:
   ```sql
   SELECT COUNT(*) FROM pg_prepared_xacts
   WHERE age(now(), prepared) > interval '1 hour';
   ```

3. **TVIEW consistency** (periodic check):
   ```sql
   SELECT entity_name,
          (SELECT COUNT(*) FROM backing_table) as backing_count,
          (SELECT COUNT(*) FROM tview_table) as tview_count
   FROM pg_tviews_metadata;
   ```

### Alert Thresholds

- **Critical**: Queue size > 1000 for > 5 minutes
- **Warning**: Orphaned 2PC transactions > 10
- **Info**: TVIEW refresh took > 1 second

---

## Support

For issues not covered here, see:
- [GitHub Issues](https://github.com/fraiseql/pg_tviews/issues)
- [Troubleshooting Guide](./TROUBLESHOOTING.md)
```

---

## Verification Commands

```bash
# Run all failure mode tests
cd test/failure-modes
./run_all_tests.sh

# Test specific failure mode
./db-failures/test-crash-recovery.sh

# Verify documentation
mdl docs/operations/FAILURE_MODES.md  # Markdown linter
```

---

## Acceptance Criteria

- [ ] All failure modes documented with recovery procedures
- [ ] Database failure tests pass (crash, disk full, OOM)
- [ ] Extension failure tests pass (circular deps, metadata corruption)
- [ ] Operational failure tests pass (upgrade, backup/restore)
- [ ] FAILURE_MODES.md document created and reviewed
- [ ] Emergency procedures tested
- [ ] Monitoring queries validated
- [ ] No data corruption in any failure scenario

---

## DO NOT

- ❌ Assume any failure is "impossible" - test it
- ❌ Skip testing recovery procedures - must verify they work
- ❌ Write vague recovery docs - provide exact SQL commands
- ❌ Ignore failure modes that are "rare" - document all

---

## Rollback Plan

No rollback needed - this phase only adds tests and documentation.

---

## Next Steps

After completion:
- Commit with message: `docs(ops): Add comprehensive failure mode analysis and recovery [PHASE2.3]`
- Share FAILURE_MODES.md with production users
- Proceed to **Phase 2.4: Security Audit**
