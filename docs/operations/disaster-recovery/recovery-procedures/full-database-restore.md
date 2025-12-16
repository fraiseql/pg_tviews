# Full Database Restore Procedure

## Purpose
Restore an entire PostgreSQL database from backup, including all TVIEWs and associated data, following data loss or corruption incidents.

## When to Use
- **Complete Database Loss**: Server failure, storage corruption, or disaster
- **Major Data Corruption**: Widespread data integrity issues
- **Testing**: Validating backup integrity and recovery procedures
- **Migration**: Moving database to new infrastructure

## Prerequisites
- **Valid Backup**: Tested backup file available and accessible
- **Clean Environment**: Target PostgreSQL instance ready for restore
- **Permissions**: Database superuser access
- **Storage**: Sufficient disk space (3x backup size minimum)
- **Time Window**: Scheduled maintenance window for restore duration

## Impact Assessment

### Downtime
- **Logical Restore (pg_restore)**: 30-120 minutes depending on database size
- **Physical Restore (pg_basebackup)**: 15-60 minutes
- **Testing**: Additional 30-60 minutes for validation

### Data Loss
- **RPO Dependent**: Based on backup frequency and WAL archiving
- **Target**: < 15 minutes for critical systems
- **Recovery**: Point-in-time recovery available if WAL archived

### Resource Requirements
- **CPU**: High during restore operations
- **Memory**: 2-4x normal database memory
- **Storage**: 3x database size for restore operations
- **Network**: Fast access to backup files

## Pre-Restore Preparation

### Step 1: Environment Assessment
```bash
# Check available resources
echo "=== Environment Assessment ==="
echo "CPU Cores: $(nproc)"
echo "Memory: $(free -h | grep '^Mem:' | awk '{print $2}')"
echo "Disk Space: $(df -h /var/lib/postgresql | tail -1 | awk '{print $4}')"

# Check PostgreSQL status
sudo systemctl status postgresql
psql -c "SELECT version();"
```

### Step 2: Backup Verification
```bash
# Verify backup file integrity
BACKUP_FILE="/backups/mydb-recent.dump"

echo "=== Backup Verification ==="
ls -lh $BACKUP_FILE

# Test backup readability
sudo -u postgres pg_restore --list $BACKUP_FILE | head -10

# Check backup age
echo "Backup created: $(stat -c %y $BACKUP_FILE)"
echo "Backup age: $(($(date +%s) - $(stat -c %Y $BACKUP_FILE))) seconds"
```

### Step 3: Target Database Preparation
```bash
# Stop applications
echo "Stopping dependent applications..."
# sudo systemctl stop your-app your-api

# Create restore database (if needed)
TARGET_DB="mydb_restored"
sudo -u postgres createdb $TARGET_DB

# Verify database creation
psql -l | grep $TARGET_DB
```

## Logical Restore Procedure (pg_restore)

### Step 1: Initial Restore Setup
```bash
# Set restore parameters
export PGHOST=localhost
export PGUSER=postgres
export PGDATABASE=$TARGET_DB
export BACKUP_FILE=/backups/mydb-recent.dump

# Create restore log
RESTORE_LOG="/var/log/pg_restore_$(date +%Y%m%d_%H%M%S).log"
echo "Starting restore at $(date)" > $RESTORE_LOG
```

### Step 2: Schema-Only Restore
```bash
echo "=== Phase 1: Schema Restore ==="

# Restore schema only first
sudo -u postgres pg_restore \
    --verbose \
    --schema-only \
    --no-owner \
    --no-privileges \
    --dbname=$TARGET_DB \
    $BACKUP_FILE \
    2>&1 | tee -a $RESTORE_LOG

# Verify schema creation
psql -d $TARGET_DB -c "
SELECT schemaname, COUNT(*) as objects
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
GROUP BY schemaname
ORDER BY schemaname;
"
```

### Step 3: Data Restore
```bash
echo "=== Phase 2: Data Restore ==="

# Restore data with parallel processing
sudo -u postgres pg_restore \
    --verbose \
    --data-only \
    --no-owner \
    --no-privileges \
    --disable-triggers \
    --jobs=4 \
    --dbname=$TARGET_DB \
    $BACKUP_FILE \
    2>&1 | tee -a $RESTORE_LOG

# Check for restore errors
if grep -i "error\|failed\|fatal" $RESTORE_LOG; then
    echo "⚠️  Errors detected in restore log - review manually"
fi
```

### Step 4: Index and Constraint Restore
```bash
echo "=== Phase 3: Indexes and Constraints ==="

# Create indexes (if not included in data restore)
# Note: pg_restore typically handles this, but verify
psql -d $TARGET_DB -c "
SELECT schemaname, tablename,
       COUNT(*) as indexes_expected
FROM pg_indexes
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
GROUP BY schemaname, tablename
ORDER BY schemaname, tablename;
"

# Re-enable triggers
psql -d $TARGET_DB -c "ALTER TABLE your_table ENABLE TRIGGER ALL;"  # Repeat for each table
```

### Step 5: Permission and Ownership Restore
```bash
echo "=== Phase 4: Permissions and Ownership ==="

# Restore ownership (if using custom roles)
# Note: This depends on your backup method
psql -d $TARGET_DB -c "
-- Example ownership restoration
-- ALTER TABLE your_table OWNER TO your_owner;
-- GRANT SELECT ON your_table TO your_user;
"

# Verify permissions
psql -d $TARGET_DB -c "
SELECT schemaname, tablename, tableowner
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY schemaname, tablename;
"
```

## Physical Restore Procedure (pg_basebackup)

### Step 1: Base Backup Restore
```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Backup current data directory
sudo mv /var/lib/postgresql/16/main /var/lib/postgresql/16/main.backup

# Extract base backup
sudo tar -xzf /backups/base-recent.tar.gz -C /var/lib/postgresql/16/

# Fix permissions
sudo chown -R postgres:postgres /var/lib/postgresql/16/main
```

### Step 2: WAL Recovery (if applicable)
```bash
# Copy required WAL files
sudo mkdir -p /var/lib/postgresql/16/main/pg_wal
sudo cp /backups/wal/* /var/lib/postgresql/16/main/pg_wal/

# Create recovery.conf for PITR if needed
sudo vi /var/lib/postgresql/16/main/recovery.conf
# restore_command = 'cp /backups/wal/%f %p'
# recovery_target_time = '2025-12-13 10:00:00'
```

### Step 3: Start PostgreSQL
```bash
# Start PostgreSQL (will perform recovery)
sudo systemctl start postgresql

# Monitor recovery progress
tail -f /var/log/postgresql/postgresql-16-main.log

# Verify recovery completion
psql -c "SELECT pg_is_in_recovery();"
```

## TVIEW-Specific Restore Steps

### Step 1: TVIEW Extension Installation
```sql
-- Install pg_tviews extension
CREATE EXTENSION pg_tviews;

-- Verify extension
SELECT pg_tviews_version();
```

### Step 2: TVIEW Recreation
```sql
-- Recreate TVIEWs from backup metadata
-- This assumes TVIEW definitions were backed up separately

-- Example TVIEW recreation (adjust for your schema)
SELECT pg_tviews_convert_existing_table('public.sales');
SELECT pg_tviews_convert_existing_table('public.inventory');
-- Add more TVIEWs as needed

-- Verify TVIEWs are functional
SELECT
    COUNT(*) as tviews_created,
    COUNT(*) FILTER (WHERE last_error IS NULL) as healthy_tviews
FROM pg_tviews_metadata;
```

### Step 3: TVIEW Data Validation
```sql
-- Test TVIEW functionality
SELECT pg_tviews_health_check();

-- Test refresh operations
SELECT pg_tviews_refresh('your_test_tview');

-- Verify data consistency
SELECT
    'Data validation' as check,
    (SELECT COUNT(*) FROM your_source_table) as source_count,
    (SELECT COUNT(*) FROM your_tview) as tview_count,
    CASE
        WHEN (SELECT COUNT(*) FROM your_source_table) = (SELECT COUNT(*) FROM your_tview)
        THEN 'CONSISTENT'
        ELSE 'INCONSISTENT'
    END as status;
```

## Post-Restore Validation

### Step 1: Database Integrity Checks
```sql
-- Run comprehensive integrity checks
VACUUM VERBOSE;  -- Check for corruption

-- Verify foreign key constraints
SELECT
    conname,
    conrelid::regclass,
    confrelid::regclass
FROM pg_constraint
WHERE contype = 'f'
LIMIT 5;

-- Check for orphaned records
-- Add custom checks based on your schema
```

### Step 2: Performance Validation
```sql
-- Test query performance
EXPLAIN ANALYZE SELECT COUNT(*) FROM your_largest_table;

-- Check index usage
SELECT
    schemaname,
    tablename,
    idx_scan,
    seq_scan
FROM pg_stat_user_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY seq_scan DESC;

-- Update statistics
ANALYZE;
```

### Step 3: Application Testing
```bash
# Test application connectivity
curl -f http://your-app/health

# Test database operations
psql -c "SELECT 1;"

# Run application integration tests
# npm test  # or your test command
```

## Success Criteria

### Technical Success
- [ ] PostgreSQL starts successfully
- [ ] All databases accessible
- [ ] pg_tviews extension functional
- [ ] TVIEWs present and operational
- [ ] Data integrity verified
- [ ] Performance within acceptable ranges

### Application Success
- [ ] Applications connect successfully
- [ ] Core functionality working
- [ ] User operations functional
- [ ] Error rates normal
- [ ] Response times acceptable

### Business Success
- [ ] Systems operational within RTO
- [ ] Data recovered within RPO
- [ ] Business processes resumed
- [ ] Stakeholder communication completed
- [ ] Incident documented

## Rollback Procedures

### Immediate Rollback (< 15 minutes)
If restore introduces new issues:

```bash
# Stop applications
sudo systemctl stop your-applications

# Drop restored database
sudo -u postgres dropdb $TARGET_DB

# Restore from original backup if needed
# (Keep original database running during testing)

# Restart applications
sudo systemctl start your-applications
```

### Complete Environment Rollback (< 60 minutes)
If full rollback required:

```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Restore original data directory
sudo rm -rf /var/lib/postgresql/16/main
sudo mv /var/lib/postgresql/16/main.backup /var/lib/postgresql/16/main

# Start PostgreSQL
sudo systemctl start postgresql

# Verify rollback
psql -c "SELECT current_database();"
```

## Monitoring During Restore

### Progress Monitoring
```bash
# Monitor restore progress
watch -n 30 "psql -c 'SELECT phase, n_tup_ins, n_tup_upd, n_tup_del FROM pg_stat_progress_copy;'"

# Monitor system resources
watch -n 10 "free -h && df -h /var/lib/postgresql"
```

### Alert Thresholds
- **Duration**: Alert if restore takes > 2x expected time
- **Errors**: Alert on any restore errors
- **Resources**: Alert if disk space < 10% or memory < 20%
- **Connections**: Monitor application connection attempts

## Troubleshooting

### Common Restore Issues

#### Permission Errors
```sql
# Fix ownership
sudo chown -R postgres:postgres /var/lib/postgresql/16/main

# Check file permissions
ls -la /var/lib/postgresql/16/main/
```

#### Out of Disk Space
```sql
# Check space usage
df -h /var/lib/postgresql

# Clean up space if needed
sudo find /var/lib/postgresql -name "*.log" -mtime +7 -delete

# Or add more disk space
```

#### Extension Installation Failures
```sql
# Check extension files
ls -la /usr/share/postgresql/16/extension/pg_tviews*

# Reinstall extension
cd /path/to/pg_tviews && make install

# Try extension creation again
CREATE EXTENSION pg_tviews;
```

#### TVIEW Recreation Issues
```sql
-- Check source table exists
SELECT schemaname, tablename FROM pg_tables WHERE tablename = 'source_table';

-- Verify table structure
SELECT column_name, data_type FROM information_schema.columns
WHERE table_name = 'source_table'
ORDER BY ordinal_position;
```

## Performance Optimization

### Restore Performance Tuning
```bash
# Use parallel restore
pg_restore --jobs=8 --dbname=$TARGET_DB $BACKUP_FILE

# Disable synchronous commit during restore
psql -c "ALTER SYSTEM SET synchronous_commit = off;"

# Re-enable after restore
psql -c "ALTER SYSTEM SET synchronous_commit = on;"
```

### Post-Restore Optimization
```sql
-- Update statistics
ANALYZE;

-- Rebuild indexes if needed
REINDEX DATABASE CONCURRENTLY $TARGET_DB;

-- Vacuum for space optimization
VACUUM FULL;
```

## Documentation Requirements

### Restore Record
- [ ] Date and time of restore
- [ ] Backup file used
- [ ] Duration and issues encountered
- [ ] Success verification results
- [ ] Performance impact assessment

### Incident Documentation
- [ ] Root cause of data loss
- [ ] Restore procedure followed
- [ ] Issues encountered and resolutions
- [ ] Lessons learned and improvements

## Related Documentation

- [Backup Types](../backup-strategy/backup-types.md) - Backup creation procedures
- [Backup Testing](../backup-strategy/backup-testing.md) - Backup validation
- [Point-in-Time Recovery](point-in-time-recovery.md) - Advanced recovery options
- [TVIEW Recovery](tview-recovery.md) - TVIEW-specific recovery</content>
<parameter name="filePath">docs/operations/disaster-recovery/recovery-procedures/full-database-restore.md