# Phase 5.2: Upgrade & Migration Guides

**Objective**: Create comprehensive upgrade procedures for PostgreSQL versions and pg_tviews extension upgrades

**Priority**: MEDIUM
**Estimated Time**: 1-2 days
**Blockers**: Phase 2, 3 complete

---

## Context

**Current State**: Limited upgrade documentation for production systems

**Why This Matters**:
- PostgreSQL upgrades are high-risk operations
- Extension upgrades can cause data loss if done incorrectly
- Users need tested, step-by-step procedures
- Upgrade failures can cause extended downtime
- Proper upgrade planning prevents data corruption

**Deliverable**: Complete upgrade guides for all supported PostgreSQL versions with tested procedures

---

## Upgrade Scenarios to Cover

### Category 1: PostgreSQL Version Upgrades

1. **Minor Version Upgrades** (e.g., 15.1 → 15.5)
   - Simple restart, usually safe
   - Requires extension reinstall if needed

2. **Major Version Upgrades** (e.g., 15 → 16 → 17)
   - Use pg_upgrade
   - Extension must be reinstalled
   - Requires more extensive testing

3. **Direct vs. Logical Upgrades**
   - pg_upgrade (in-place, faster)
   - pg_dump + pg_restore (logical, safer)

### Category 2: Extension Upgrades

4. **Minor Extension Updates** (e.g., 0.1.0 → 0.1.1)
   - SQL upgrade scripts
   - No data migration needed

5. **Major Extension Updates** (e.g., 0.1.x → 0.2.x)
   - May require schema changes
   - Data migration procedures
   - Breaking API changes

6. **Downgrade Procedures** (fallback if upgrade fails)
   - Restore from backup
   - Documented rollback steps

---

## Implementation Steps

### Step 1: Create Upgrade Guide Directory

**Create**: `docs/operations/upgrade/`

```
docs/operations/upgrade/
├── README.md
├── postgresql/
│   ├── minor-version-upgrade.md
│   ├── pg15-to-pg16.md
│   ├── pg16-to-pg17.md
│   ├── testing-upgrade.md
│   └── troubleshooting-upgrades.md
├── extension/
│   ├── 0.1-to-0.2-migration.md
│   ├── extension-minor-update.md
│   └── breaking-changes.md
├── backwards-compatibility/
│   ├── api-compatibility.md
│   └── sql-compatibility.md
└── scripts/
    ├── pre-upgrade-checks.sh
    ├── upgrade-extension.sql
    └── post-upgrade-validation.sql
```

### Step 2: PostgreSQL Minor Version Upgrade Guide

**Create**: `docs/operations/upgrade/postgresql/minor-version-upgrade.md`

```markdown
# PostgreSQL Minor Version Upgrade

## Scope
Upgrading within same major version (e.g., 15.1 → 15.5)

## Prerequisites
- PostgreSQL 15.x currently running
- At least 20GB free disk space
- Backup of production database
- Maintenance window scheduled (< 30 min downtime)
- Read-only mode working on applications

## Impact
- Downtime: 5-15 minutes
- TVIEWs: No changes needed
- Data: No migration required
- Compatibility: Fully compatible

## Step-by-Step Procedure

### Phase 1: Pre-Upgrade (1 hour before)

**Step 1: Backup Database**
```bash
# Full logical backup
sudo -u postgres pg_dump mydb | gzip > /backups/mydb-pre-upgrade.sql.gz

# Verify backup
gunzip -c /backups/mydb-pre-upgrade.sql.gz | head -20
echo "✅ Backup successful"
```

**Step 2: Run Pre-Upgrade Checks**
```bash
# Run health check
psql -f docs/operations/runbooks/scripts/health-check.sql > /tmp/pre-upgrade-check.txt

# Verify all TVIEWs are healthy
grep -i error /tmp/pre-upgrade-check.txt
# Should show 0 errors

# Note current version
psql -tAc "SELECT version();" > /tmp/pg-version-before.txt
psql -tAc "SELECT pg_tviews_version();" > /tmp/ext-version-before.txt

cat /tmp/pg-version-before.txt
cat /tmp/ext-version-before.txt
```

**Step 3: Notify Applications**
```bash
# Send notification
echo "PostgreSQL upgrade in 1 hour, brief downtime expected"

# Set read-only mode on application if available
# curl -X POST http://app:8080/admin/readonly -d "reason=maintenance"
```

**Step 4: Verify Queue Empty**
```sql
SELECT COUNT(*) FROM pg_tviews_get_queue();
-- Must be 0 or very small
```

### Phase 2: Stop Services

**Step 5: Stop Applications**
```bash
# Tell apps to stop accepting connections
# systemctl stop myapp

# Wait for in-flight requests
sleep 30

# Verify no open connections
psql -c "SELECT COUNT(*) FROM pg_stat_activity WHERE usename != 'postgres';"
# Should return 0
```

**Step 6: Stop PostgreSQL**
```bash
sudo systemctl stop postgresql

# Verify stopped
sleep 5
pg_isready && echo "❌ PostgreSQL still running" || echo "✅ PostgreSQL stopped"
```

### Phase 3: Upgrade PostgreSQL

**Step 7: Download and Install New Version**
```bash
# Update package manager
sudo apt update

# Install new PostgreSQL version (same major version)
sudo apt install postgresql-15=15.5-1.pgdg22.04+1

# Verify installed
psql --version
# Should show 15.5
```

**Step 8: Start PostgreSQL**
```bash
sudo systemctl start postgresql

# Verify running
pg_isready && echo "✅ PostgreSQL started"

# Wait for recovery if any
sleep 10

# Verify extension still works
psql -c "SELECT pg_tviews_version();"
```

### Phase 4: Post-Upgrade Validation

**Step 9: Verify Database Health**
```bash
# Check all TVIEWs still exist
psql -c "SELECT COUNT(*) FROM pg_tviews_metadata;"

# Verify version updated
psql -tAc "SELECT version();" > /tmp/pg-version-after.txt
diff /tmp/pg-version-before.txt /tmp/pg-version-after.txt

# Run health check
psql -f docs/operations/runbooks/scripts/health-check.sql
# All should show ✅

# Verify data integrity
psql -c "SELECT entity_name, COUNT(*) FROM pg_tviews_metadata GROUP BY entity_name;"
```

**Step 10: Refresh All TVIEWs**
```sql
-- Refresh to ensure all working
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name);
  END LOOP;
END $$;
```

**Step 11: Restart Applications**
```bash
# Start applications
# systemctl start myapp

# Wait for readiness
sleep 10

# Verify can connect
curl http://app:8080/health
echo "✅ Application healthy"

# Disable read-only mode
# curl -X POST http://app:8080/admin/readonly -d "reason="
```

**Step 12: Final Verification**
```bash
# Smoke test: verify functionality
psql -c "INSERT INTO test_tview_backing (data) VALUES ('test');
         SELECT COUNT(*) FROM test_tview;"
# Count should increase

# Check for errors
psql -c "SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL;"
# Should be 0
```

### Phase 5: Post-Upgrade Cleanup

**Step 13: Update Documentation**
```bash
# Update version in docs
# Update runbook with new version

# Notify team of successful upgrade
# Send upgrade completion notification
```

## Rollback Procedure (if needed)

**⚠️ Only if something goes wrong**

```bash
# Step 1: Stop PostgreSQL
sudo systemctl stop postgresql

# Step 2: Downgrade package
sudo apt install postgresql-15=15.4-1.pgdg22.04+1

# Step 3: Start PostgreSQL
sudo systemctl start postgresql

# Step 4: Verify
psql -c "SELECT version();"

# Step 5: Refresh TVIEWs
psql -c "SELECT pg_tviews_refresh('entity_name');"
```

## Success Criteria

- ✅ PostgreSQL version updated
- ✅ All TVIEWs still exist and functional
- ✅ Health check passes
- ✅ Applications can connect
- ✅ No errors in pg_tviews_metadata
- ✅ Queue is empty
- ✅ Data integrity verified

## Estimated Time

- Pre-upgrade checks: 15 min
- Backup: 10-30 min (depends on DB size)
- Upgrade: 5-10 min
- Post-upgrade validation: 10 min
- **Total**: ~45-65 minutes

## Rollback Plan

- Total rollback time: ~15-30 minutes
- Data is safe (backup available)
- Previous version available in apt repository

## Support

If issues occur:
1. Check PostgreSQL logs: `journalctl -u postgresql -n 50`
2. Review upgrade checklist again
3. Consult [Troubleshooting](./troubleshooting-upgrades.md)
4. If critical: restore from backup
```

### Step 3: PostgreSQL Major Version Upgrade Guide

**Create**: `docs/operations/upgrade/postgresql/pg15-to-pg16.md`

```markdown
# PostgreSQL 15 to 16 Upgrade

## Scope
Major version upgrade using pg_upgrade or pg_dump/restore

## Prerequisites
- PostgreSQL 15.x currently running (at stable version like 15.5)
- PostgreSQL 16.x installed alongside 15.x
- At least 2x current database size in free space
- Backup of production database
- 2-4 hour maintenance window (major version upgrade)
- Read-only mode capability

## Impact
- Downtime: 30-120 minutes (depends on database size)
- TVIEWs: Extension must be reinstalled
- Data: Safe with proper backup
- Compatibility: May need extension rebuild

## Decision: pg_upgrade vs. pg_dump/restore

### Use pg_upgrade if:
- ✅ Database < 100GB
- ✅ No failing pg_upgrade checks
- ✅ Can spare 2x database space
- ✅ Need to minimize downtime

### Use pg_dump/restore if:
- ✅ Database > 500GB (too slow for pg_upgrade)
- ✅ Want safest possible upgrade
- ✅ pg_upgrade checks fail
- ✅ Willing to accept longer downtime

## Step-by-Step: pg_upgrade Method

### Phase 1: Preparation (before maintenance window)

**Step 1: Verify Support Matrix**
```bash
# Current version
psql -tAc "SELECT version();"
# Must be 15.x (recommend 15.5+)

# New version must be ready
/usr/lib/postgresql/16/bin/postgres --version
# Should show PostgreSQL 16.x
```

**Step 2: Check Compatibility**
```bash
# Run pg_upgrade checks
sudo -u postgres /usr/lib/postgresql/16/bin/pg_upgrade \
  --old-bindir=/usr/lib/postgresql/15/bin \
  --new-bindir=/usr/lib/postgresql/16/bin \
  --old-datadir=/var/lib/postgresql/15/main \
  --new-datadir=/var/lib/postgresql/16/main \
  --check

# Review output for any issues
# Common issues:
# - Extension incompatible (must rebuild)
# - Data type compatibility issues
# - Function signature changes
```

**Step 3: Full Backup**
```bash
# Logical backup (safest)
sudo -u postgres pg_dump mydb > /backups/mydb-pg15.sql

# Or physical backup
sudo tar -czf /backups/pg15-cluster.tar.gz /var/lib/postgresql/15/

# Verify backup
ls -lh /backups/mydb-pg15.sql
# Should be non-empty
```

**Step 4: Pre-Upgrade Analysis**
```sql
-- Run pre-upgrade checks
SELECT COUNT(*) as tview_count FROM pg_tviews_metadata;
SELECT pg_tviews_version();
SELECT COUNT(*) as queue_size FROM pg_tviews_get_queue();
SELECT COUNT(*) FROM pg_prepared_xacts;

-- Should have:
-- - Non-zero TVIEW count
-- - Empty queue
-- - No prepared xacts
```

### Phase 2: Maintenance Window - Stop Services

**Step 5: Notify & Stop Applications**
```bash
echo "Starting PostgreSQL 15→16 upgrade"
echo "Database will be offline for 1-2 hours"

# Stop application
# systemctl stop myapp

# Wait for connections to close
sleep 30

# Verify no connections
sudo -u postgres psql -tAc "SELECT COUNT(*) FROM pg_stat_activity WHERE usename != 'postgres';"
# Must return 0
```

**Step 6: Stop PostgreSQL 15**
```bash
sudo systemctl stop postgresql

# Verify stopped
sleep 5
! pg_isready && echo "✅ PostgreSQL stopped"
```

### Phase 3: Perform Upgrade

**Step 7: Run pg_upgrade**
```bash
# Set permissions
sudo chown postgres:postgres /var/lib/postgresql/16/main
sudo chmod 700 /var/lib/postgresql/16/main

# Run upgrade
sudo -u postgres /usr/lib/postgresql/16/bin/pg_upgrade \
  --old-bindir=/usr/lib/postgresql/15/bin \
  --new-bindir=/usr/lib/postgresql/16/bin \
  --old-datadir=/var/lib/postgresql/15/main \
  --new-datadir=/var/lib/postgresql/16/main \
  --link

# Expected output:
# Performing Consistency Checks
# ...
# Creating dump of global objects
# ...
# Transferring user relation files
# ...
# Analyzing all user relations
# ...
# pg_upgrade run successfully
```

**Step 8: Update PostgreSQL Configuration**
```bash
# Update postgresql.conf for version 16
# Copy any custom settings from 15 to 16
sudo cp /etc/postgresql/15/main/postgresql.conf \
        /etc/postgresql/16/main/postgresql.conf

# Or merge custom settings manually
```

**Step 9: Start PostgreSQL 16**
```bash
sudo systemctl start postgresql

# Wait for startup
sleep 10

# Verify running with new version
psql -c "SELECT version();"
# Should show PostgreSQL 16.x

# Check status
pg_isready && echo "✅ PostgreSQL 16 started"
```

### Phase 4: Extension Rebuild

**Step 10: Rebuild pg_tviews Extension**
```bash
# Check if extension is installed
psql -c "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';"

# If not found (likely - major version upgrade), reinstall:
cd ~/pg_tviews  # or wherever source is

# Build for PostgreSQL 16
cargo pgrx install --release --pg-config=/usr/lib/postgresql/16/bin/pg_config

# Verify installed
psql -c "SELECT pg_tviews_version();"
```

**Step 11: Verify Metadata**
```sql
-- Check all TVIEWs still have metadata
SELECT COUNT(*) FROM pg_tviews_metadata;
-- Should match pre-upgrade count

-- Check for any missing metadata
SELECT entity_name FROM pg_tviews_metadata
WHERE backing_table_name IS NULL;
-- Should return 0 rows

-- Check for errors
SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL;
-- Should be 0
```

### Phase 5: Validation

**Step 12: Run Comprehensive Validation**
```bash
# Health check
psql -f docs/operations/runbooks/scripts/health-check.sql

# Test each TVIEW
psql <<'EOF'
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name, force => true);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Successfully refreshed % TVIEWs', count;
END $$;
EOF
```

**Step 13: Performance Baseline**
```bash
# Compare upgrade impact
time psql -c "SELECT COUNT(*) FROM large_tview;" > /tmp/post-upgrade-perf.txt

# Should be similar to pre-upgrade performance
```

**Step 14: Restart Applications**
```bash
# Start application
# systemctl start myapp

# Wait for readiness
sleep 15

# Smoke test
curl http://app:8080/health
# Should return 200 OK

# Monitor for errors
tail -f /var/log/myapp.log
```

### Phase 6: Cleanup

**Step 15: Clean Up Old Cluster**
```bash
# Only after verifying 16 is working well (24 hours)
sudo -u postgres /usr/lib/postgresql/16/bin/pg_upgrade --delete-old-cluster

# Or manually remove:
sudo rm -rf /var/lib/postgresql/15/main
sudo apt remove postgresql-15

# Verify removed
dpkg -l | grep postgresql-15
# Should show nothing
```

## Rollback Procedure (if problems arise)

**⚠️ Only use if necessary - restores from pre-upgrade state**

```bash
# Step 1: Stop PostgreSQL 16
sudo systemctl stop postgresql

# Step 2: Restore old cluster
sudo rm -rf /var/lib/postgresql/16/main
sudo cp /var/lib/postgresql/15/main /var/lib/postgresql/15/main.backup
# (if you still have it)

# Step 3: Start PostgreSQL 15 again
# Reinstall PostgreSQL 15 if needed

# Step 4: Restore from backup if needed
sudo -u postgres pg_restore < /backups/mydb-pg15.sql
```

## Success Criteria

- ✅ PostgreSQL version 16
- ✅ pg_tviews extension installed
- ✅ All TVIEWs present and functional
- ✅ Health check passes
- ✅ Queue empty
- ✅ No errors
- ✅ Applications can connect
- ✅ Performance similar to before

## Estimated Time

- Checks: 15 min
- Backup: 30-60 min
- pg_upgrade: 10-30 min (depends on DB size)
- Extension rebuild: 5-10 min
- Validation: 15-30 min
- **Total**: 2-3 hours

## Known Issues & Solutions

### Extension File Not Found
```bash
# If pg_tviews.so not found:
cd ~/pg_tviews
cargo pgrx install --release --pg-config=/usr/lib/postgresql/16/bin/pg_config
```

### Function Signature Mismatch
```sql
-- If functions fail:
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;
-- Recreate TVIEWs
```

### Memory Issues During Upgrade
```bash
# If pg_upgrade runs out of memory:
# Reduce parallelism
pg_upgrade --jobs=1 ...
```

## Next Major Version

Expect upgrades to PostgreSQL 17 to follow similar procedure.

## References
- [PostgreSQL Upgrade Documentation](https://www.postgresql.org/docs/current/upgrading.html)
- [Troubleshooting](./troubleshooting-upgrades.md)
- [Emergency Procedures](../runbooks/04-incident-response/emergency-procedures.md)
```

### Step 4: Extension Major Version Upgrade Guide

**Create**: `docs/operations/upgrade/extension/0.1-to-0.2-migration.md`

```markdown
# pg_tviews 0.1.x to 0.2.x Migration

## Scope
Major feature version upgrade with schema changes and API improvements

## Prerequisites
- pg_tviews 0.1.5 or later currently installed
- PostgreSQL 15+ (minimum supported version)
- Backup of database
- Read-only mode available
- Test database for validation

## Impact Summary

| Item | Impact | Notes |
|------|--------|-------|
| **Downtime** | 15-30 min | Brief, for cutover |
| **Data Loss** | None | With proper backup |
| **API Changes** | Yes | See Breaking Changes |
| **TVIEWs** | Unchanged | Work as before |
| **Metadata** | Schema changed | Auto-migrated |
| **Performance** | Better | ~10% improvement |

## Breaking Changes in 0.2.0

### 1. API Changes

**Removed Functions**:
- `pg_tviews_refresh_async()` - Use `SELECT pg_tviews_refresh()` instead

**Changed Parameters**:
- `pg_tviews_refresh(name TEXT, force BOOLEAN)`
  → `pg_tviews_refresh(entity_name TEXT, force BOOLEAN DEFAULT false)`

**New Functions**:
- `pg_tviews_stats()` - Get statistics
- `pg_tviews_validate()` - Validate TVIEW integrity

### 2. Metadata Schema

**New Columns in `pg_tviews_metadata`**:
- `stats_last_computed TIMESTAMP`
- `query_plan TEXT`
- `source_hash BYTEA`

**Removed Columns**:
- `internal_version` (no longer used)

### 3. Configuration Changes

**New Settings**:
- `pg_tviews.enable_stats` (default: true)
- `pg_tviews.stats_sample_size` (default: 10000)

## Step-by-Step Upgrade

### Phase 1: Pre-Upgrade (1 hour before)

**Step 1: Backup Database**
```bash
# Full backup
sudo -u postgres pg_dump mydb -Fc > /backups/mydb-pre-0.2.sql

# Verify
pg_restore -l /backups/mydb-pre-0.2.sql | head -20
echo "✅ Backup successful"
```

**Step 2: Verify Current Version**
```sql
SELECT pg_tviews_version() as version;
-- Should return 0.1.x
```

**Step 3: Document Current State**
```sql
-- Save TVIEW definitions
\d+ tv_*

-- Save metadata
SELECT entity_name, backing_table_name
FROM pg_tviews_metadata
ORDER BY entity_name;

-- Save in file for reference
\o /tmp/pre-upgrade-state.sql
SELECT 'CREATE TABLE tv_backup AS SELECT * FROM pg_tviews_metadata;';
\o

-- Count rows
SELECT entity_name, (SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES
                      WHERE TABLE_NAME = entity_name) as exists
FROM pg_tviews_metadata;
```

### Phase 2: Stop Operations

**Step 4: Set Read-Only Mode**
```sql
-- Disable refresh triggers to prevent new queue entries
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
  RAISE NOTICE 'All TVIEW refresh triggers disabled';
END $$;

-- Verify queue is empty
SELECT COUNT(*) FROM pg_tviews_get_queue();
-- Wait if not empty
```

**Step 5: Final Backup Check**
```bash
# Ensure backup is valid
sudo -u postgres pg_restore -l /backups/mydb-pre-0.2.sql | wc -l
# Should show 100+ objects
```

### Phase 3: Upgrade Extension

**Step 6: Install New Extension Version**
```bash
# If upgrading from binary release:
# 1. Update Debian package
sudo apt update
sudo apt install pg-tviews=0.2.0-1

# If upgrading from source:
cd ~/pg_tviews
git checkout v0.2.0
cargo pgrx install --release --pg-config=/usr/lib/postgresql/15/bin/pg_config
```

**Step 7: Run SQL Migration Script**
```bash
# This creates new columns, updates metadata
psql mydb -f docs/upgrade/0.1-to-0.2-migration.sql

# Script should:
# - Add new metadata columns
# - Migrate 0.1.x data to 0.2 format
# - Update version markers
# - Rebuild internal structures

# Verify no errors
echo $?  # Should be 0
```

**Step 8: Verify Extension Upgraded**
```sql
SELECT pg_tviews_version();
-- Should return 0.2.0

-- Check new columns exist
SELECT stats_last_computed, query_plan
FROM pg_tviews_metadata
LIMIT 1;
-- Should return columns (may be NULL)

-- Check new functions work
SELECT COUNT(*) FROM pg_tviews_stats();
-- Should return row count
```

### Phase 4: Re-enable Operations

**Step 9: Re-enable Refresh Triggers**
```sql
-- Re-enable all triggers
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
  RAISE NOTICE 'All TVIEW refresh triggers re-enabled';
END $$;
```

**Step 10: Refresh All TVIEWs**
```sql
-- Force refresh all to populate new statistics
DO $$
DECLARE
  rec RECORD;
  count INT := 0;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name, force => true);
    count := count + 1;
  END LOOP;
  RAISE NOTICE 'Refreshed % TVIEWs', count;
END $$;
```

**Step 11: Verify All TVIEWs Working**
```sql
-- Check stats computed
SELECT entity_name,
  stats_last_computed IS NOT NULL as has_stats
FROM pg_tviews_metadata
ORDER BY entity_name;

-- All should have stats

-- Test new API
SELECT COUNT(*) as tview_count FROM pg_tviews_stats();

-- Validate TVIEWs
SELECT * FROM pg_tviews_validate();
-- Should return 0 rows (no issues) or issues list
```

### Phase 5: Application Updates

**Step 12: Update Application Code**

For applications using removed functions:

**Before (0.1.x)**:
```python
# Old API - no longer supported
db.execute("SELECT pg_tviews_refresh_async('my_tview')")
```

**After (0.2.x)**:
```python
# New API
db.execute("SELECT pg_tviews_refresh('my_tview')")
# Now returns immediately (internally async)
```

**Step 13: Test Application**
```bash
# Deploy application updates
# systemctl restart myapp

# Monitor logs
tail -f /var/log/myapp.log

# Look for deprecation warnings or errors
grep -i "tview\|refresh" /var/log/myapp.log

# Smoke test
curl http://app:8080/health
```

### Phase 6: Validation

**Step 14: Performance Baseline**
```bash
# Before upgrade (from logs):
# Query time: 150ms, Memory: 256MB

# After upgrade:
time psql -c "SELECT COUNT(*) FROM large_tview;"
# Should be faster or same

# Memory usage:
psql -c "SELECT pg_relation_size('large_tview');"
```

**Step 15: Final Checklist**
```bash
# All checks
psql -f docs/operations/runbooks/scripts/health-check.sql

# Should show:
# ✅ pg_tviews version 0.2.0
# ✅ All TVIEWs have stats
# ✅ No errors
# ✅ Queue empty
# ✅ Performance good
```

## Rollback Procedure

**Only if critical issues found within 1 hour**

```bash
# Step 1: Stop application
# systemctl stop myapp

# Step 2: Restore from backup
sudo -u postgres pg_restore --clean /backups/mydb-pre-0.2.sql

# Step 3: Downgrade extension
cd ~/pg_tviews
git checkout v0.1.5
cargo pgrx install --release --pg-config=/usr/lib/postgresql/15/bin/pg_config

# Step 4: Verify
psql -c "SELECT pg_tviews_version();"
# Should return 0.1.5

# Step 5: Restart app
# systemctl start myapp
```

## Success Criteria

- ✅ Extension version is 0.2.0
- ✅ All TVIEWs still exist
- ✅ Health check passes
- ✅ New functions available
- ✅ Statistics populated
- ✅ Application works
- ✅ No data corruption
- ✅ Performance same or better

## Estimated Time

- Backup: 10-30 min
- Upgrade: 5-10 min
- Migration SQL: 2-5 min
- Refresh all: 5-15 min
- Validation: 10-15 min
- **Total**: 45-75 minutes

## Breaking Changes Reference

See [0.1-to-0.2 Breaking Changes](./breaking-changes.md) for full API documentation.

## Support

If issues occur:
1. Restore from backup
2. Downgrade to 0.1.x
3. Contact support with logs
4. Reference issue tracker

## Next Steps

After successful upgrade:
- Monitor logs for 24 hours
- Monitor performance metrics
- Test all client applications
- Document any customizations made for upgrade
```

### Step 5: Create Pre-Upgrade Check Script

**Create**: `docs/operations/upgrade/scripts/pre-upgrade-checks.sh`

```bash
#!/bin/bash
set -euo pipefail

# Pre-upgrade validation script
# Run before any PostgreSQL or extension upgrade

echo "=== pg_tviews Pre-Upgrade Checks ==="
echo "Started: $(date)"

# Check 1: PostgreSQL is running
echo -n "PostgreSQL running... "
pg_isready && echo "✅" || { echo "❌"; exit 1; }

# Check 2: pg_tviews extension installed
echo -n "pg_tviews installed... "
psql -tAc "SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';" && echo "✅" || echo "⚠️  (may be normal)"

# Check 3: Database size
echo -n "Database size... "
DB_SIZE=$(psql -tAc "SELECT pg_size_pretty(pg_database_size(current_database()))")
echo "$DB_SIZE"

# Check 4: TVIEWs count
echo -n "TVIEW count... "
TVIEW_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_metadata" 2>/dev/null || echo "0")
echo "$TVIEW_COUNT TVIEWs"

# Check 5: Queue empty
echo -n "Queue size... "
QUEUE_SIZE=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_get_queue()" 2>/dev/null || echo "unknown")
echo "$QUEUE_SIZE entries"
if [ "$QUEUE_SIZE" != "0" ] && [ "$QUEUE_SIZE" != "unknown" ]; then
  echo "⚠️  Queue not empty - may cause issues"
fi

# Check 6: Disk space
echo -n "Free disk space... "
FREE_SPACE=$(df -h / | tail -1 | awk '{print $4}')
echo "$FREE_SPACE"

# Check 7: Backup exists
echo -n "Recent backup... "
if [ -f "/backups/latest.sql" ]; then
  BACKUP_AGE=$(find /backups/latest.sql -mtime -1)
  echo "✅ (< 24 hours old)"
else
  echo "❌ No backup found - MUST BACKUP BEFORE UPGRADE"
  exit 1
fi

# Check 8: No errors in metadata
echo -n "Metadata errors... "
ERROR_COUNT=$(psql -tAc "SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL" 2>/dev/null || echo "0")
if [ "$ERROR_COUNT" = "0" ]; then
  echo "✅ None"
else
  echo "⚠️  $ERROR_COUNT TVIEWs have errors"
fi

echo ""
echo "=== Summary ==="
echo "✅ Ready to proceed with upgrade"
echo "Completed: $(date)"
```

### Step 6: Create Upgrade Troubleshooting Guide

**Create**: `docs/operations/upgrade/postgresql/troubleshooting-upgrades.md`

```markdown
# Upgrade Troubleshooting Guide

## Common Issues and Solutions

### Issue 1: pg_upgrade Fails with "Function Signature Mismatch"

**Symptom**:
```
pg_upgrade: error: Function with OID xyz from namespace pg_tviews...
has changed signature since the old cluster was migrated.
```

**Cause**: Extension changed between versions

**Solution**:
```bash
# Option 1: Use pg_dump/restore instead
pg_dump -Fc olddb > olddb.dump
pg_restore -d newdb olddb.dump

# Option 2: Manually drop and recreate extension
psql -c "DROP EXTENSION pg_tviews CASCADE;"
psql -c "CREATE EXTENSION pg_tviews;"
```

### Issue 2: Extension Won't Load After Upgrade

**Symptom**:
```
ERROR: could not open extension control file
```

**Cause**: Extension binary not found or wrong version

**Solution**:
```bash
# Rebuild extension for new PostgreSQL version
cd ~/pg_tviews
cargo pgrx install --release --pg-config=/usr/lib/postgresql/NEW_VERSION/bin/pg_config

# Verify
psql -c "SELECT pg_tviews_version();"
```

### Issue 3: TVIEWs Not Refreshing After Upgrade

**Symptom**:
- Queries work but data is stale
- Manual refresh works
- Auto-refresh triggers not firing

**Solution**:
```sql
-- Re-enable triggers
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN
    SELECT DISTINCT trigger_name, event_object_table
    FROM information_schema.triggers
    WHERE trigger_name LIKE 'pg_tviews_%' AND is_enabled = false
  LOOP
    EXECUTE format('ALTER TABLE %I ENABLE TRIGGER %I',
                  rec.event_object_table, rec.trigger_name);
  END LOOP;
END $$;

-- Force refresh all
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name, force => true);
  END LOOP;
END $$;
```

### Issue 4: Out of Memory During pg_upgrade

**Symptom**:
```
pg_upgrade: error: Child process exited abnormally
Temp table: pg_temp_4294967295.pg_upgrade_dump_globals
```

**Cause**: Database too large, running out of memory

**Solution**:
```bash
# Cancel pg_upgrade and use pg_dump/restore instead
# Or reduce parallel jobs
pg_upgrade --jobs=1 --verbose ...

# Also, increase shared_buffers temporarily
# In postgresql.conf: shared_buffers = 2GB (from 256MB)
# Then restart and try again
```

### Issue 5: Prepared Transactions Block Upgrade

**Symptom**:
```
pg_upgrade: error: Could not connect to database: (13 prepared transactions exist)
```

**Cause**: 2PC transactions in prepared state

**Solution**:
```sql
-- First, try to commit/rollback prepared
SELECT gid FROM pg_prepared_xacts;

-- For each:
COMMIT PREPARED 'gid_value';
-- or
ROLLBACK PREPARED 'gid_value';

-- If stuck, may need to kill client and restart PostgreSQL
```

### Issue 6: Performance Much Worse After Upgrade

**Symptom**:
- Refresh takes 10x longer
- Queries are slow
- High CPU/memory usage

**Solution**:
```bash
# Step 1: Analyze tables
psql -c "VACUUM ANALYZE;"

# Step 2: Check statistics
psql -c "SELECT relpages, reltuples FROM pg_class WHERE relname = 'my_tview';"

# Step 3: Reindex if needed
psql -c "REINDEX DATABASE mydb;"

# Step 4: Compare plans
EXPLAIN SELECT * FROM my_tview LIMIT 10;
# Compare with pre-upgrade plan

# Step 5: If still slow, investigate indexes
# May need to rebuild or create new indexes
```

### Issue 7: Metadata Inconsistency After Upgrade

**Symptom**:
```
ERROR: Metadata not found for TVIEW: my_tview
```

**Cause**: Metadata table not migrated properly

**Solution**:
```sql
-- Verify metadata structure
\d pg_tviews_metadata

-- Restore from backup if corrupted
-- Or re-convert TVIEWs

SELECT pg_tviews_convert_existing_table('my_tview');
```

## Rollback Checklist

Before you rollback, verify:

- [ ] You have backup from before upgrade
- [ ] Backup is readable and valid
- [ ] You have space to restore (1.5x database size)
- [ ] Team is aware of downtime
- [ ] You've documented the failure

### Rollback Steps

```bash
# 1. Stop PostgreSQL new version
sudo systemctl stop postgresql

# 2. Restore from backup
sudo -u postgres pg_restore --clean /backups/pre-upgrade.dump

# 3. Start old PostgreSQL
# Reinstall old version if needed

# 4. Verify
psql -c "SELECT pg_tviews_version();"
```

## Prevention Checklist

- [ ] Test upgrade on dev database first
- [ ] Always have recent backup
- [ ] Verify backup is restorable
- [ ] Schedule upgrade in low-traffic window
- [ ] Have rollback plan documented
- [ ] Test rollback procedure
- [ ] Monitor closely after upgrade (24 hours)

## Getting Help

1. Check this guide first
2. Review PostgreSQL upgrade logs: `journalctl -u postgresql -n 100`
3. Check pg_tviews logs if available
4. Review upgrade script output
5. Contact PostgreSQL/pg_tviews support with logs
```

---

## Verification Commands

```bash
# Verify all upgrade guides exist
test -f docs/operations/upgrade/postgresql/minor-version-upgrade.md
test -f docs/operations/upgrade/postgresql/pg15-to-pg16.md
test -f docs/operations/upgrade/extension/0.1-to-0.2-migration.md

# Verify scripts exist and are executable
test -x docs/operations/upgrade/scripts/pre-upgrade-checks.sh

# Verify markdown formatting
mdl docs/operations/upgrade/**/*.md 2>/dev/null || echo "mdl not installed"

# Check all guides for required sections
for file in docs/operations/upgrade/**/*.md; do
  echo "Checking $file"
  grep -q "Prerequisites" "$file" || echo "  ❌ Missing Prerequisites"
  grep -q "Rollback" "$file" || echo "  ❌ Missing Rollback"
  grep -q "Success Criteria" "$file" || echo "  ❌ Missing Success Criteria"
done
```

---

## Acceptance Criteria

- [ ] Minor PostgreSQL version upgrade guide created and tested
- [ ] Major PostgreSQL version upgrade guide (pg15→pg16) created
- [ ] Extension major version upgrade guide (0.1→0.2) created
- [ ] PostgreSQL upgrade guide includes both pg_upgrade and pg_dump methods
- [ ] All guides include pre-upgrade checks
- [ ] All guides include rollback procedures
- [ ] All guides include success criteria
- [ ] Pre-upgrade check script created and functional
- [ ] Troubleshooting guide covers 7+ common issues
- [ ] No hardcoded database names (use parameterized versions)
- [ ] All procedures tested on sample database

---

## DO NOT

- ❌ Write upgrade guides without testing first
- ❌ Include upgrade procedures that skip backups
- ❌ Forget rollback procedures
- ❌ Use in-place upgrades without pg_upgrade checks
- ❌ Assume all upgrades are safe (major versions need testing)
- ❌ Write procedures without downtime estimates
- ❌ Skip validation steps
- ❌ Leave ambiguous instructions (specific commands only)

---

## Rollback Plan

No rollback needed - this phase only adds documentation.

Update guides if new versions discovered to have upgrade issues:
```bash
git add docs/operations/upgrade/
git commit -m "docs(upgrade): Add upgrade guides and procedures"
```

---

## Next Steps

After completion:
- Commit with message: `docs(ops): Add comprehensive upgrade guides and procedures [PHASE5.2]`
- Test guides with sample database
- Have ops team review for accuracy
- Proceed to **Phase 5.3: Disaster Recovery Procedures**
