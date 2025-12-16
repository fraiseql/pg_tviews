# Phase 5.3: Disaster Recovery Procedures

**Objective**: Create comprehensive backup, restore, and disaster recovery procedures

**Priority**: MEDIUM
**Estimated Time**: 1-2 days
**Blockers**: Phase 2, 3 complete

---

## Context

**Current State**: No documented disaster recovery procedures

**Why This Matters**:
- Data loss is a business-critical event
- Recovery time is measured in business impact dollars
- Without procedures, recovery becomes chaotic and error-prone
- RTO (Recovery Time Objective) and RPO (Recovery Point Objective) must be defined
- Every hour of downtime costs money

**Deliverable**: Complete DR procedures with tested backup/restore and recovery scenarios

---

## Disaster Recovery Planning

### Key Metrics

**RTO (Recovery Time Objective)**: Maximum acceptable downtime
- Target: < 1 hour
- Critical systems: < 15 minutes

**RPO (Recovery Point Objective)**: Maximum acceptable data loss
- Target: < 15 minutes of transactions
- Critical systems: < 5 minutes

### Failure Scenarios to Cover

1. **Data Corruption**
   - TVIEW data inconsistent
   - Metadata corrupted
   - Backing table damaged

2. **Complete Database Loss**
   - Entire PostgreSQL cluster down
   - Data directory inaccessible
   - Hardware failure

3. **Partial Loss**
   - Single table missing
   - Single TVIEW corrupted
   - Metadata missing

4. **Replication Issues**
   - Replica out of sync
   - Standby can't follow primary
   - Failover needed

---

## Implementation Steps

### Step 1: Create Disaster Recovery Structure

**Create**: `docs/operations/disaster-recovery/`

```
docs/operations/disaster-recovery/
├── README.md
├── backup-strategy/
│   ├── backup-types.md
│   ├── backup-frequency.md
│   ├── backup-testing.md
│   └── backup-retention.md
├── recovery-procedures/
│   ├── full-database-restore.md
│   ├── point-in-time-recovery.md
│   ├── partial-recovery.md
│   ├── tview-recovery.md
│   └── metadata-recovery.md
├── failover-procedures/
│   ├── planned-failover.md
│   ├── unplanned-failover.md
│   ├── failback-procedure.md
│   └── replica-resync.md
├── runbooks/
│   ├── data-corruption-checklist.md
│   ├── hardware-failure-response.md
│   ├── network-partition-response.md
│   └── ransomware-response.md
└── scripts/
    ├── create-backup.sh
    ├── restore-backup.sh
    ├── verify-backup.sh
    ├── test-recovery.sh
    └── cleanup-after-recovery.sh
```

### Step 2: Backup Strategy Document

**Create**: `docs/operations/disaster-recovery/backup-strategy/backup-types.md`

```markdown
# Backup Strategy for pg_tviews

## Backup Types

### 1. Logical Backups (pg_dump)

**What**: SQL dump of entire database

**Advantages**:
- ✅ Portable across versions (15→16→17)
- ✅ Human-readable (can inspect/edit)
- ✅ Version-independent restore
- ✅ Partial restore possible

**Disadvantages**:
- ❌ Slower on large databases (100GB+ slower)
- ❌ Larger files (often 50% of original)
- ❌ Can't do point-in-time recovery

**When to Use**:
- Regular nightly backups
- Before major changes
- Before PostgreSQL upgrades
- Databases < 100GB

**Procedure**:
```bash
# Full database backup
sudo -u postgres pg_dump -Fc mydb > /backups/mydb-$(date +%Y%m%d).dump

# Backup with compression and parallel jobs (faster)
sudo -u postgres pg_dump -Fc -j 4 mydb > /backups/mydb-$(date +%Y%m%d).dump

# Backup specific schema only
sudo -u postgres pg_dump -Fc -n public mydb > /backups/mydb-schema-$(date +%Y%m%d).dump

# Verification
pg_restore -l /backups/mydb-*.dump | wc -l
# Should show 100+ objects
```

### 2. Physical Backups (PITR)

**What**: PostgreSQL cluster files + WAL archive

**Advantages**:
- ✅ Very fast backup
- ✅ Very fast restore
- ✅ Point-in-time recovery supported
- ✅ Incremental backups possible
- ✅ Full cluster backup (pg_basebackup)

**Disadvantages**:
- ❌ Version-specific (can't upgrade across versions)
- ❌ Large disk space needed (2x database)
- ❌ Complex setup
- ❌ Not human-readable

**When to Use**:
- Large databases (> 100GB)
- Continuous availability required
- Short RTO needed (< 30 min)
- Replication setup

**Procedure**:
```bash
# Create base backup for replication
sudo -u postgres pg_basebackup \
  -D /var/lib/postgresql/16/standby \
  -F tar \
  -z \
  -W  # ask for password
  -v

# Use streaming replication
# In recovery.conf (or postgresql.auto.conf):
# standby_mode = 'on'
# primary_conninfo = 'host=primary port=5432'
# restore_command = 'cp /pg_wal_archive/%f %p'
```

### 3. Continuous WAL Archiving

**What**: Archive transaction logs for point-in-time recovery

**Advantages**:
- ✅ RPO of minutes (granular)
- ✅ Continuous backup
- ✅ Minimal overhead

**Disadvantages**:
- ❌ Requires separate archive storage
- ❌ Complex setup

**When to Use**:
- Production systems
- RTO < 15 minutes
- Cannot tolerate data loss

**Procedure**:
```bash
# In postgresql.conf
wal_level = replica
archive_mode = on
archive_command = 'test ! -f /pg_wal_archive/%f && cp %p /pg_wal_archive/%f'
archive_timeout = 300  # 5 minutes

# Monitor archive
ls -la /pg_wal_archive/ | tail -20
# Should have ~one file per 5 minutes
```

### 4. Snapshots (if using cloud storage)

**What**: Cloud storage snapshots of database volume

**Advantages**:
- ✅ Instant backup
- ✅ Minimal CPU impact
- ✅ Fast recovery

**Disadvantages**:
- ❌ Cloud provider dependent
- ❌ Cost (snapshot storage)
- ❌ Limited history (usually 30 days)

**When to Use**:
- Cloud-hosted PostgreSQL
- Can't use physical backups
- Frequent backups needed

## Recommended Backup Combination

For most pg_tviews deployments:

1. **Daily Logical Backup** (pg_dump)
   - Time: 6 AM UTC
   - Retention: 30 days
   - Storage: /backups/logical/

2. **Weekly Full Physical Backup** (pg_basebackup)
   - Time: Sunday 1 AM UTC
   - Retention: 12 weeks
   - Storage: /backups/physical/

3. **Continuous WAL Archive** (optional, if PITR needed)
   - Archive to: /pg_wal_archive/
   - Retention: 7 days
   - Used for point-in-time recovery

## Backup Schedule

```
Mon Tue Wed Thu Fri  Sat Sun
 └─ Daily logical (6 AM)
 └─ Daily logical (6 AM)
 └─ Daily logical (6 AM)
 └─ Daily logical (6 AM)
 └─ Daily logical (6 AM)
 └─ Daily logical + weekly physical (6 AM + 1 AM)
 └─ Daily logical (6 AM)
```

## Backup Locations

```
Primary:    /backups/           (local SSD for speed)
Secondary:  /mnt/backup-store/  (larger, slower storage)
Offsite:    AWS S3 / GCS        (redundancy)
```

## Backup Size Estimates

```
Database Size | Logical Dump | Physical + WAL | Monthly Cost (S3)
10 GB         | 3-5 GB       | 10 GB          | $2-3
50 GB         | 15-25 GB     | 50 GB          | $10-15
100 GB        | 30-50 GB     | 100 GB         | $20-30
500 GB        | 150-250 GB   | 500 GB         | $100-150
```

## Success Criteria

- ✅ Backup runs daily without failure
- ✅ Backup size monitored
- ✅ Retention policy enforced (delete old backups)
- ✅ Backup verification run daily
- ✅ Offsite backup synced daily
- ✅ Backups tested monthly with restore

## Monitoring

```sql
-- Check last backup timestamp
SELECT
  CURRENT_DATABASE() as db,
  pg_postmaster_start_time() as pg_start,
  now() as current_time,
  (SELECT MAX(ctime) FROM (
    SELECT ctime FROM pg_ls_dir('/pg_wal_archive')
  ) sub) as last_wal_time;

-- Expected: last_wal_time within 5 minutes of current_time
```

## Backup Automation

```bash
#!/bin/bash
# /usr/local/bin/pg-daily-backup.sh

set -euo pipefail

DB="mydb"
BACKUP_DIR="/backups"
DATE=$(date +%Y%m%d)

echo "Starting backup: $DATE"

# Backup
sudo -u postgres pg_dump -Fc "$DB" > "$BACKUP_DIR/mydb-$DATE.dump"

# Compress additional
gzip "$BACKUP_DIR/mydb-$DATE.dump"

# Verify
pg_restore -l "$BACKUP_DIR/mydb-$DATE.dump.gz" > /dev/null && \
  echo "✅ Backup verified" || \
  echo "❌ Backup failed verification"

# Sync to secondary
rsync -av "$BACKUP_DIR/mydb-$DATE.dump.gz" \
  backup-store:/mnt/backup-store/

# Sync to offsite (S3)
aws s3 cp "$BACKUP_DIR/mydb-$DATE.dump.gz" s3://mycompany-backups/

# Clean old backups (30 days)
find "$BACKUP_DIR" -name "mydb-*.dump.gz" -mtime +30 -delete

echo "✅ Backup complete"
```

**In crontab**:
```bash
0 6 * * * /usr/local/bin/pg-daily-backup.sh >> /var/log/backups.log 2>&1
```

---

## Testing Backups

### Monthly Backup Restore Test

**Procedure**:
```bash
#!/bin/bash
# Test restore on non-production server

BACKUP_DATE="20241215"
BACKUP_FILE="/backups/mydb-$BACKUP_DATE.dump.gz"
TEST_DB="mydb_restore_test"

# Clean any existing test DB
psql -c "DROP DATABASE IF EXISTS $TEST_DB;" 2>/dev/null || true

# Create test database
createdb "$TEST_DB"

# Restore from backup
pg_restore -d "$TEST_DB" "$BACKUP_FILE"

# Verify TVIEWs
psql "$TEST_DB" -c "SELECT COUNT(*) FROM pg_tviews_metadata;"

# Verify data integrity
psql "$TEST_DB" -c "SELECT entity_name, COUNT(*) FROM ... GROUP BY entity_name;"

# Clean up
dropdb "$TEST_DB"

echo "✅ Backup restore test passed"
```

## References

- [PostgreSQL Backup Documentation](https://www.postgresql.org/docs/current/backup.html)
- [Backup Schedule](./backup-frequency.md)
- [Testing Backups](./backup-testing.md)
```

### Step 3: Full Database Restore Procedure

**Create**: `docs/operations/disaster-recovery/recovery-procedures/full-database-restore.md`

```markdown
# Full Database Restore Procedure

## Scope
Complete recovery of entire database from backup

## When to Use
- Complete data corruption
- Catastrophic hardware failure
- Need to recover to specific point in time
- Migration to new hardware

## Prerequisites
- Valid backup file (verified with pg_restore -l)
- PostgreSQL running (empty or different database name)
- Sufficient disk space (1.5x backup size)
- Database credentials with superuser access

## Impact
- Full downtime until restore complete
- All open connections dropped
- All data reverted to backup point
- RTO: 30 minutes - 2 hours (depends on size)

## Step-by-Step Procedure

### Phase 1: Preparation

**Step 1: Verify Backup Integrity**
```bash
# Check backup file exists and is readable
ls -lh /backups/mydb-20241215.dump.gz
# Should show size > 0

# Verify backup structure
pg_restore -l /backups/mydb-20241215.dump.gz | head -20
# Should show 20+ lines of objects

# Verify backup can be read
pg_restore -l /backups/mydb-20241215.dump.gz | tail -5
# Should complete without errors
```

**Step 2: Notify Stakeholders**
```bash
# Announce downtime
echo "Database restore starting. Expected downtime: 1 hour"
# Send notifications
```

**Step 3: Identify Backup Point**
```bash
# For point-in-time recovery, check available backups
ls -lt /backups/mydb-*.dump.gz | head -10

# Use most recent backup before corruption time
BACKUP_TIME="20241215-180000"  # Last good backup
BACKUP_FILE="/backups/mydb-$BACKUP_TIME.dump.gz"
```

### Phase 2: Stop Services

**Step 4: Stop Applications**
```bash
# Gracefully stop app
# systemctl stop myapp

# Wait for shutdown
sleep 30

# Verify no connections
psql -tAc "SELECT COUNT(*) FROM pg_stat_activity WHERE usename != 'postgres';"
# Should return 0
```

**Step 5: Create New Database for Restore**
```bash
# Option A: Restore to new database (safe, parallel to old)
createdb mydb_restored

# Option B: Drop old and restore to same name (dangerous)
# dropdb mydb
# createdb mydb
```

### Phase 3: Restore from Backup

**Step 6: Restore Database**
```bash
# Decompress if gzipped
gunzip -c /backups/mydb-$BACKUP_TIME.dump.gz > /tmp/mydb.sql

# Or restore directly (faster)
pg_restore -d mydb_restored \
  --no-password \
  --verbose \
  /backups/mydb-$BACKUP_TIME.dump.gz

# Monitor progress
tail -f /tmp/restore.log

# Expected: Should take 5-30 minutes depending on size
```

**Step 7: Wait for Restore to Complete**
```bash
# Monitor restore progress
watch -n 1 'psql -tAc "SELECT COUNT(*) FROM pg_tviews_metadata;"'

# Should increase from 0 to final count

# Or check restore process
ps aux | grep pg_restore
# When complete, should have no pg_restore processes
```

### Phase 4: Validation

**Step 8: Verify Restored Database**
```sql
-- Connect to restored database
psql mydb_restored

-- Check extension installed
SELECT pg_tviews_version();

-- Check TVIEW count
SELECT COUNT(*) FROM pg_tviews_metadata;
-- Should match pre-backup count

-- Check metadata integrity
SELECT entity_name, backing_table_name
FROM pg_tviews_metadata
ORDER BY entity_name;

-- Spot-check data
SELECT COUNT(*) FROM <TVIEW_NAME> LIMIT 1;
-- Should return row count

-- Check for errors
SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL;
-- Should be 0

-- Check triggers
SELECT COUNT(*) FROM information_schema.triggers
WHERE trigger_name LIKE 'pg_tviews_%';
-- Should match number of TVIEWs
```

**Step 9: Refresh All TVIEWs**
```sql
-- Force refresh all to ensure consistency
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

-- Wait for refreshes to complete
SELECT COUNT(*) FROM pg_tviews_get_queue();
-- Should eventually return 0
```

**Step 10: Run Comprehensive Checks**
```bash
# Run health check script
psql -f docs/operations/runbooks/scripts/health-check.sql

# Should show:
# ✅ All TVIEWs OK
# ✅ Queue empty
# ✅ No errors
```

### Phase 5: Switchover

**Step 11: Switch Applications to Restored Database**

**Option A: Same Database Name**
```bash
# If restored to mydb_restored, can rename/switch:
# 1. Drop corrupted: dropdb mydb
# 2. Rename: psql -c "ALTER DATABASE mydb_restored RENAME TO mydb;"
# 3. Verify: psql mydb -c "SELECT version();"
```

**Option B: New Database Name**
```bash
# Update application connection strings
# Change connection from mydb to mydb_restored
# Restart application

# systemctl start myapp

# Verify connection
curl http://app:8080/health
# Should return 200 OK
```

**Option C: Verify Old Database Still Exists**
```bash
# Can keep both for comparison
psql -tAc "SELECT datname FROM pg_database WHERE datname LIKE 'mydb%';"
# Should show both mydb and mydb_restored
```

### Phase 6: Post-Recovery

**Step 12: Verify Applications Working**
```bash
# Smoke tests
curl http://app:8080/api/tvievs
# Should return data

# Check logs for errors
tail -f /var/log/myapp.log
# Should show successful connections

# Run integration tests if available
./tests/smoke-test.sh
```

**Step 13: Update Monitoring & Backups**
```bash
# Update monitoring to new database
# curl -X POST http://monitoring/config -d "db=mydb_restored"

# Ensure backups resume
/usr/local/bin/pg-daily-backup.sh

# Verify
ls -lt /backups/mydb-*.dump.gz | head -1
# Should show recent backup
```

**Step 14: Document Recovery**
```bash
# Create incident report
cat > /tmp/recovery-report.txt <<'EOF'
Recovery Report
================
Time Started: [timestamp]
Time Completed: [timestamp]
Duration: [time]
Data Loss: [time period lost, if any]
Root Cause: [what caused the need for recovery]
Backup Used: [backup file name]
Backup Age: [how old backup was]
TVIEWs Affected: [count]
Applications Affected: [list]
Post-Recovery Actions: [what to verify]
EOF

# Share report with team
```

## Rollback (if restore fails)

```bash
# If restore fails and old DB still exists
# Can revert to original
# systemctl restart postgresql

# Then troubleshoot why restore failed
# Check error logs:
less /var/log/postgresql/postgresql.log
```

## Success Criteria

- ✅ Restored database accessible
- ✅ pg_tviews extension installed
- ✅ All TVIEWs present
- ✅ Data integrity verified
- ✅ No corrupted metadata
- ✅ Triggers working
- ✅ Applications can connect
- ✅ Refresh working
- ✅ Performance similar to before

## Estimated Time

| Task | Duration |
|------|----------|
| Preparation | 10 min |
| Database backup verification | 5 min |
| Service shutdown | 10 min |
| Restore from backup | 30-120 min (by DB size) |
| Validation | 15 min |
| Application restart | 10 min |
| Verification | 10 min |
| **Total** | **1-3 hours** |

## Testing This Procedure

Test restore monthly:
```bash
# On non-production server
createdb test_restore
pg_restore -d test_restore /backups/mydb-latest.dump.gz
psql test_restore -c "SELECT COUNT(*) FROM pg_tviews_metadata;"
dropdb test_restore
echo "✅ Restore test passed"
```

## References
- [Backup Strategy](../backup-strategy/backup-types.md)
- [PostgreSQL Restore Documentation](https://www.postgresql.org/docs/current/app-pgrestore.html)
- [Health Check Runbook](../../runbooks/01-health-monitoring/tview-health-check.md)
```

### Step 4: Data Corruption Response

**Create**: `docs/operations/disaster-recovery/runbooks/data-corruption-checklist.md`

```markdown
# Data Corruption Response Checklist

## Initial Assessment (First 5 minutes)

- [ ] Identify affected TVIEW(s)
  ```sql
  -- Check TVIEW row count vs backing table
  SELECT 'tv_users' as entity,
    (SELECT COUNT(*) FROM tb_users) as backing_count,
    (SELECT COUNT(*) FROM tv_users) as tview_count,
    CASE WHEN (SELECT COUNT(*) FROM tb_users) = (SELECT COUNT(*) FROM tv_users)
      THEN '✅ MATCH'
      ELSE '❌ MISMATCH'
    END as status;
  ```

- [ ] Check last_error in metadata
  ```sql
  SELECT entity_name, last_error, last_refresh_time
  FROM pg_tviews_metadata
  WHERE last_error IS NOT NULL;
  ```

- [ ] Determine impact scope
  - Single TVIEW or multiple?
  - Data loss or consistency issue?
  - Read-only impact or write impact?

- [ ] Create incident ticket with:
  - Time of discovery
  - Affected TVIEWs
  - Estimated impact
  - Customer-facing systems affected

## Detection: Data Inconsistency (Most Common)

**Symptom**: TVIEW has fewer rows than backing table

```sql
-- Compare counts
SELECT
  COUNT(*) as backing_count,
  (SELECT COUNT(*) FROM tv_name) as tview_count
FROM tb_name;

-- If counts differ:
-- Option 1: Try refresh
SELECT pg_tviews_refresh('tv_name', force => true);

-- Wait 30 seconds, check again
SELECT COUNT(*) FROM tv_name;

-- If still wrong:
-- Option 2: Analyze the diff
SELECT id FROM tb_name
EXCEPT
SELECT id FROM tv_name;
-- Shows IDs in backing table but not TVIEW
```

## Detection: Trigger Disabled

**Symptom**: TVIEW not updating even though backing table changed

```sql
-- Check if triggers exist
SELECT trigger_name, event_object_table, is_enabled
FROM information_schema.triggers
WHERE trigger_name LIKE 'pg_tviews_%'
AND event_object_table = 'tb_name';

-- If is_enabled = false, re-enable:
ALTER TABLE tb_name ENABLE TRIGGER pg_tviews_<id>;

-- Then refresh
SELECT pg_tviews_refresh('tv_name', force => true);
```

## Detection: Metadata Corruption

**Symptom**: Can't find TVIEW metadata

```sql
-- Check if metadata exists
SELECT COUNT(*) FROM pg_tviews_metadata WHERE entity_name = 'tv_name';

-- If 0 rows:
-- Option 1: Recreate metadata
SELECT pg_tviews_convert_existing_table('tv_name');

-- Verify
SELECT * FROM pg_tviews_metadata WHERE entity_name = 'tv_name';
```

## Detection: Circular Dependency

**Symptom**: Error creating new TVIEW or during refresh

```
ERROR: Circular dependency detected
```

```sql
-- Check dependency chain
-- Find all TVIEWs referenced by tv_name
SELECT dependent_on FROM pg_tviews_metadata WHERE entity_name = 'tv_name';

-- Find all TVIEWs that reference tv_name
SELECT entity_name FROM pg_tviews_metadata WHERE dependent_on LIKE '%tv_name%';

-- If circular: break the cycle
-- Option 1: Drop the dependent TVIEW
DROP TABLE tv_dependent CASCADE;

-- Option 2: Recreate without the circular reference
-- Rewrite the query to not reference the other TVIEW
```

## Recovery Actions (by Severity)

### Minor Corruption (Single TVIEW, < 1% data loss)

1. **Try Simple Refresh**
   ```sql
   SELECT pg_tviews_refresh('tv_name', force => true);
   ```

2. **Wait 30 seconds**
   ```sql
   SELECT pg_sleep(30);
   SELECT COUNT(*) FROM tv_name;
   ```

3. **If fixed**: Document and monitor
4. **If not fixed**: Proceed to "Major Corruption"

### Major Corruption (Multiple TVIEWs or > 1% data loss)

1. **Notify stakeholders**
   - "Possible data issue detected, investigating recovery"

2. **Set to read-only if possible**
   ```sql
   -- Pause refresh triggers
   DO $$
   DECLARE
     rec RECORD;
   BEGIN
     FOR rec IN SELECT DISTINCT trigger_name, event_object_table
                 FROM information_schema.triggers
                 WHERE trigger_name LIKE 'pg_tviews_%' LOOP
       EXECUTE format('ALTER TABLE %I DISABLE TRIGGER %I',
                     rec.event_object_table, rec.trigger_name);
     END LOOP;
   END $$;
   ```

3. **Create isolation database for analysis**
   ```bash
   # Logical copy for forensics
   pg_dump mydb -Fc > /tmp/corruption-evidence.dump

   # Restore to separate DB for analysis
   createdb mydb_analysis
   pg_restore -d mydb_analysis /tmp/corruption-evidence.dump
   ```

4. **Investigate root cause**
   - Check PostgreSQL logs
   - Check application logs
   - Review recent changes
   - Check disk health

5. **Proceed to full restore** (if root cause not found)
   - See [Full Database Restore](../recovery-procedures/full-database-restore.md)

### Critical Corruption (Data loss risk)

1. **Immediate escalation**
   - Page database team
   - Call incident commander
   - Notify C-level if customer-impacting

2. **Failover if replica available**
   - See [Failover Procedures](../failover-procedures/planned-failover.md)

3. **Full restore from backup**
   - Follow [Full Database Restore](../recovery-procedures/full-database-restore.md)

4. **Determine data loss window**
   - Backup time to corruption time
   - Report to stakeholders

## Testing This Procedure

### Monthly Corruption Detection Test

```bash
#!/bin/bash
# Simulate data corruption, verify detection

# Create test TVIEW
psql mydb <<'EOF'
CREATE TABLE tb_corrupt (id INT, data TEXT);
CREATE TABLE tv_corrupt AS SELECT * FROM tb_corrupt;
SELECT pg_tviews_convert_existing_table('tv_corrupt');
INSERT INTO tb_corrupt VALUES (1, 'test');
SELECT COUNT(*) FROM tv_corrupt;  -- Should be 1
EOF

# Simulate corruption (delete from TVIEW)
psql mydb -c "DELETE FROM tv_corrupt;"

# Test detection
psql mydb <<'EOF'
SELECT
  (SELECT COUNT(*) FROM tb_corrupt) as backing,
  (SELECT COUNT(*) FROM tv_corrupt) as tview,
  CASE WHEN (SELECT COUNT(*) FROM tb_corrupt) !=
            (SELECT COUNT(*) FROM tv_corrupt)
    THEN '❌ DETECTED'
    ELSE '✅ OK'
  END as status;
EOF

# Verify recovery works
psql mydb -c "SELECT pg_tviews_refresh('tv_corrupt', force => true);"
psql mydb -c "SELECT COUNT(*) FROM tv_corrupt;  -- Should be 1 again"

echo "✅ Corruption detection test passed"
```

## Escalation Matrix

| Severity | Scope | Response Time | Actions |
|----------|-------|---------------|---------|
| Minor | Single TVIEW, < 1% loss | 1 hour | Try refresh |
| Major | Multiple TVIEWs, 1-10% loss | 15 min | Isolate, investigate |
| Critical | > 10% loss, > 1000 rows | 5 min | Failover or restore |

## References

- [Full Database Restore](../recovery-procedures/full-database-restore.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
- [Health Check](../../runbooks/01-health-monitoring/tview-health-check.md)
- [PostgreSQL Logs](https://www.postgresql.org/docs/current/runtime-config-logging.html)
```

### Step 5: Failover Procedure

**Create**: `docs/operations/disaster-recovery/failover-procedures/planned-failover.md`

```markdown
# Planned Failover Procedure

## Scope
Switching from primary to standby PostgreSQL database (with replication)

## Prerequisites
- Replication already configured
- Standby database synchronized with primary
- Monitoring of replication lag
- All applications can connect to new primary via DNS or VIP
- Maintenance window scheduled

## Impact
- Brief downtime (30-120 seconds)
- Applications may need reconnect
- No data loss (if properly replicated)

## Step-by-Step Procedure

### Phase 1: Pre-Failover Checks (30 minutes before)

**Step 1: Verify Replication Status**
```bash
# On PRIMARY
psql -c "SELECT client_addr, write_lsn, flush_lsn, replay_lsn FROM pg_stat_replication;"
# Should show standby connected with matching LSN

# Check replication lag
psql -c "SELECT EXTRACT(EPOCH FROM (now() - pg_last_wal_receive_lsn())) as lag_seconds;"
# Should be < 1 second (for planned failover)
```

**Step 2: Verify All TVIEWs Replicated**
```bash
# On PRIMARY
psql -c "SELECT COUNT(*) FROM pg_tviews_metadata;"

# On STANDBY (via read-only access)
psql -p 5433 -c "SELECT COUNT(*) FROM pg_tviews_metadata;"
# Should match primary
```

**Step 3: Notify Applications & Monitoring**
```bash
# Send notifications
echo "Planned maintenance: Database failover in 30 minutes"
echo "Expected downtime: 2-5 minutes"

# Set maintenance mode
# curl -X POST monitoring/maintenance -d "duration=300&reason=failover"
```

### Phase 2: Stop Replication

**Step 4: Promote Standby to Primary**
```bash
# On STANDBY:
# Method 1: pg_ctl promote (fastest)
sudo -u postgres /usr/lib/postgresql/16/bin/pg_ctl promote \
  -D /var/lib/postgresql/16/main

# Method 2: SQL trigger file (if configured)
# touch /var/lib/postgresql/promote

# Wait for promotion to complete
sleep 10

# Verify promotion
psql -p 5433 -c "SELECT pg_is_in_recovery();"
# Should return false (not in recovery anymore)
```

### Phase 3: Verify New Primary

**Step 5: Check Standby (now Primary) Status**
```bash
# Connect to new primary (on port 5433)
psql -p 5433 <<'EOF'
-- Check version
SELECT version();

-- Check TVIEWs
SELECT COUNT(*) FROM pg_tviews_metadata;

-- Check data integrity
SELECT entity_name FROM pg_tviews_metadata
WHERE last_error IS NOT NULL;

-- Refresh all TVIEWs
DO $$
DECLARE
  rec RECORD;
BEGIN
  FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
    PERFORM pg_tviews_refresh(rec.entity_name);
  END LOOP;
END $$;
EOF
```

### Phase 4: Update Applications

**Step 6: Redirect Traffic to New Primary**
```bash
# Update DNS or VIP to point to new primary
# Option 1: Update DNS
# dig db.internal
# Should resolve to NEW_PRIMARY_IP

# Option 2: Update VIP
# ip addr add 10.0.0.100 dev eth0  (on new primary)
# ip addr del 10.0.0.100 dev eth0  (on old primary)

# Option 3: Update app config
# In app: connection_string="dbname=mydb host=db.internal"
```

**Step 7: Restart Applications**
```bash
# Applications will notice connection loss and reconnect
# OR explicitly restart to force reconnect
# systemctl restart myapp

# Verify connections to new primary
psql -h db.internal -c "SELECT COUNT(*) FROM pg_stat_activity;"
# Should show application connections

# Verify TVIEWs accessible
curl http://app:8080/api/status
# Should return 200 OK with TVIEW data
```

### Phase 5: Set Up New Replication (Optional)

**Step 8: Create New Standby (from old primary)**

If you want to maintain replication:

```bash
# On OLD PRIMARY (now standby):
# Stop it, then make it a standby of new primary

# Option 1: Use pg_rewind (fastest)
sudo -u postgres pg_rewind \
  --target-pgdata=/var/lib/postgresql/16/main \
  --source-server="host=new-primary user=postgres dbname=postgres"

# Option 2: Restore from new primary base backup
pg_basebackup -h new-primary -D /var/lib/postgresql/16/main -R

# Restart PostgreSQL on old primary
sudo systemctl restart postgresql
```

### Phase 6: Verification & Cleanup

**Step 9: Verify Failover Complete**
```bash
# Health check
psql -c "SELECT pg_tviews_version();"
psql -c "SELECT COUNT(*) FROM pg_tviews_get_queue();"

# Monitor logs
tail -f /var/log/postgresql/postgresql.log
# Should show no errors

# Application monitoring
curl http://app:8080/health
```

**Step 10: Resume Replication (if set up in Step 8)**
```bash
# Verify replication
psql -c "SELECT client_addr FROM pg_stat_replication;"
# Should show connection to old primary (now standby)
```

**Step 11: Clear Maintenance Mode**
```bash
# curl -X POST monitoring/maintenance/end
# Notify team that failover complete
```

## Rollback (if new primary fails)

```bash
# Failback to original primary:
# 1. Promote original primary (now in recovery)
# 2. Repeat process in reverse
# 3. Can only do once - second failover requires setup

# To avoid: test failover thoroughly
```

## Success Criteria

- ✅ Applications can connect to new primary
- ✅ All TVIEWs present
- ✅ No data loss
- ✅ Replication resuming (if applicable)
- ✅ Monitoring shows new primary
- ✅ Performance normal

## Estimated Time

- Checks: 5 min
- Promote: 30 sec
- Traffic switchover: 30 sec
- Application reconnect: 30 sec
- Verification: 5 min
- **Total**: 10-15 minutes (brief downtime: 2-5 min)

## Testing Failover

Practice monthly:

```bash
# In test environment or during maintenance window
# Never in production without prior testing

# Practice procedure end-to-end
# Time how long it takes
# Document any issues
```

## References

- [PostgreSQL Replication Documentation](https://www.postgresql.org/docs/current/warm-standby.html)
- [pg_ctl Documentation](https://www.postgresql.org/docs/current/app-pg-ctl.html)
- [Unplanned Failover](./unplanned-failover.md)
```

---

## Verification Commands

```bash
# Verify DR structure exists
test -d docs/operations/disaster-recovery/backup-strategy
test -d docs/operations/disaster-recovery/recovery-procedures
test -d docs/operations/disaster-recovery/failover-procedures
test -d docs/operations/disaster-recovery/runbooks
test -d docs/operations/disaster-recovery/scripts

# Verify key docs exist
test -f docs/operations/disaster-recovery/backup-strategy/backup-types.md
test -f docs/operations/disaster-recovery/recovery-procedures/full-database-restore.md
test -f docs/operations/disaster-recovery/runbooks/data-corruption-checklist.md
test -f docs/operations/disaster-recovery/failover-procedures/planned-failover.md

# Check markdown formatting
for file in docs/operations/disaster-recovery/**/*.md; do
  echo "Checking $file"
  wc -l "$file"
done
```

---

## Acceptance Criteria

- [ ] Backup strategy document created with 4 backup types
- [ ] Backup types include logical (pg_dump), physical (PITR), WAL archiving
- [ ] Backup frequency and retention policy defined
- [ ] Full database restore procedure created with step-by-step instructions
- [ ] Point-in-time recovery procedure documented
- [ ] Partial TVIEW recovery procedure documented
- [ ] Data corruption response checklist created
- [ ] Detection methods for 5+ corruption types documented
- [ ] Severity-based recovery actions defined
- [ ] Failover procedure created with pre-checks and verification
- [ ] All procedures include time estimates
- [ ] All procedures include success criteria
- [ ] All procedures include rollback/undo steps
- [ ] Testing procedures defined for each recovery type
- [ ] No hardcoded server names (use parameterized names)
- [ ] All SQL scripts tested for syntax

---

## DO NOT

- ❌ Create recovery procedures without testing
- ❌ Document untested backup/restore flows
- ❌ Forget success criteria (must verify recovery worked)
- ❌ Skip rollback procedures (always provide undo steps)
- ❌ Include procedures that lose data without warning
- ❌ Use production passwords in examples
- ❌ Document only one recovery method (multiple approaches needed)
- ❌ Leave ambiguous instructions

---

## Rollback Plan

No rollback needed - this phase only adds documentation.

Update procedures if new recovery methods discovered:
```bash
git add docs/operations/disaster-recovery/
git commit -m "docs(dr): Add disaster recovery procedures [PHASE5.3]"
```

---

## Next Steps

After completion:
- Commit with message: `docs(dr): Add comprehensive disaster recovery procedures [PHASE5.3]`
- Test all procedures with sample database
- Have ops team review for accuracy and completeness
- Conduct disaster recovery drill (quarterly)
- Update procedures based on lessons learned
- Mark Quality Initiative Phase 5 as complete

---

## Post-Phase-5 Checklist

After all Phase 5 (5.1, 5.2, 5.3) completion:

- [ ] All runbooks tested with sample database
- [ ] All upgrade guides tested on non-prod
- [ ] All DR procedures tested (restore, failover)
- [ ] Operations team trained on all procedures
- [ ] Runbooks accessible to on-call engineers 24/7
- [ ] Emergency contact list created
- [ ] Escalation procedures documented
- [ ] Monthly testing schedule established
- [ ] Incident response playbook updated
- [ ] Insurance/SLA verified for disaster scenarios

**Expected Outcome**: pg_tviews ready for enterprise production use with documented operational procedures for all scenarios.
