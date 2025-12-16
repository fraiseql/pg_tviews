# Backup Testing Procedures

## Overview

Regular backup testing ensures that backups are valid, complete, and can be restored successfully. This document provides procedures for testing different backup types and validating recovery capabilities.

## Testing Frequency

### Automated Testing
- **Daily**: Backup existence and basic integrity checks
- **Weekly**: Full restore testing to staging environment
- **Monthly**: Complete disaster recovery simulation
- **Quarterly**: TVIEW-specific recovery testing

### Manual Testing
- **After Major Changes**: Database schema changes, TVIEW modifications
- **Before Upgrades**: PostgreSQL or pg_tviews upgrades
- **After Incidents**: Following backup or recovery issues

## Automated Backup Validation

### Daily Integrity Checks
```bash
#!/bin/bash
# Daily backup validation script

BACKUP_DIR="/backups"
LOG_FILE="/var/log/backup-validation.log"

echo "$(date): Starting backup validation" >> $LOG_FILE

# Check backup file existence
DAILY_COUNT=$(find $BACKUP_DIR -name "*.dump" -mtime -1 | wc -l)
if [ $DAILY_COUNT -eq 0 ]; then
    echo "$(date): CRITICAL - No recent daily backups found" >> $LOG_FILE
    exit 1
fi

# Check backup file sizes (should be reasonable)
find $BACKUP_DIR -name "*.dump" -mtime -1 -exec ls -lh {} \; | while read line; do
    size=$(echo $line | awk '{print $5}')
    file=$(echo $line | awk '{print $9}')
    # Check if file is at least 1MB (basic sanity check)
    if [[ $size < 1000000 ]]; then
        echo "$(date): WARNING - Backup file $file seems too small: $size" >> $LOG_FILE
    fi
done

echo "$(date): Backup validation completed successfully" >> $LOG_FILE
```

### Backup Metadata Validation
```sql
-- Create backup validation tracking
CREATE TABLE IF NOT EXISTS backup_validation_log (
    validation_id SERIAL PRIMARY KEY,
    backup_file TEXT,
    validation_date TIMESTAMP DEFAULT NOW(),
    file_size_bytes BIGINT,
    object_count INTEGER,
    validation_status TEXT,
    notes TEXT
);

-- Log validation results
INSERT INTO backup_validation_log (backup_file, file_size_bytes, object_count, validation_status, notes)
SELECT
    '/backups/mydb-20251213.dump',
    (SELECT size FROM pg_stat_file('/backups/mydb-20251213.dump')),
    (SELECT count(*) FROM pg_restore --list /backups/mydb-20251213.dump),
    'SUCCESS',
    'Daily validation completed'
WHERE EXISTS (
    SELECT 1 FROM pg_stat_file('/backups/mydb-20251213.dump')
);
```

## Full Restore Testing

### Weekly Restore Testing Procedure

#### Step 1: Prepare Test Environment
```bash
# Create isolated test environment
TEST_DB="test_restore_$(date +%Y%m%d_%H%M%S)"
TEST_DIR="/tmp/test_restore"

mkdir -p $TEST_DIR
cd $TEST_DIR

# Initialize test database
sudo -u postgres createdb $TEST_DB
```

#### Step 2: Execute Restore
```bash
# Restore from recent backup
BACKUP_FILE=$(ls -t /backups/*.dump | head -1)

echo "Restoring from: $BACKUP_FILE"
time sudo -u postgres pg_restore \
    --verbose \
    --dbname=$TEST_DB \
    --jobs=4 \
    $BACKUP_FILE
```

#### Step 3: Validate Restore
```sql
-- Connect to test database
\c $TEST_DB

-- Check database structure
SELECT
    schemaname,
    tablename,
    pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename)) as size
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname || '.' || tablename) DESC
LIMIT 10;

-- Verify TVIEWs are present and functional
SELECT
    COUNT(*) as tviews_restored,
    COUNT(*) FILTER (WHERE last_error IS NULL) as healthy_tviews
FROM pg_tviews_metadata;

-- Test TVIEW functionality
SELECT pg_tviews_health_check();

-- Test a sample TVIEW
SELECT COUNT(*) FROM (SELECT * FROM your_test_tview LIMIT 100) as sample;
```

#### Step 4: Performance Validation
```sql
-- Compare performance with production
SELECT
    'Restore validation' as test_type,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as avg_refresh_time,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    pg_size_pretty(pg_database_size(current_database())) as database_size
FROM pg_stat_bgwriter;
```

#### Step 5: Cleanup
```bash
# Drop test database
sudo -u postgres dropdb $TEST_DB

# Clean up test directory
rm -rf $TEST_DIR

# Log completion
echo "$(date): Weekly restore test completed successfully" >> /var/log/backup-testing.log
```

## TVIEW-Specific Testing

### TVIEW Metadata Validation
```sql
-- Test TVIEW metadata integrity
SELECT
    entity_name,
    primary_key_column,
    created_at,
    CASE
        WHEN primary_key_column IS NULL THEN 'ERROR: Missing primary key'
        WHEN created_at > NOW() THEN 'ERROR: Future creation date'
        ELSE 'VALID'
    END as validation_status
FROM pg_tviews_metadata;
```

### TVIEW Data Consistency Testing
```sql
-- Test TVIEW data consistency (sample check)
DO $$
DECLARE
    tview_record RECORD;
    source_count INTEGER;
    tview_count INTEGER;
BEGIN
    FOR tview_record IN SELECT entity_name FROM pg_tviews_metadata LIMIT 5 LOOP
        -- This is conceptual - adjust based on your TVIEW definitions
        EXECUTE 'SELECT COUNT(*) FROM ' || tview_record.entity_name INTO tview_count;

        -- Compare with source table (adjust table name logic)
        EXECUTE 'SELECT COUNT(*) FROM ' || replace(tview_record.entity_name, 'tview_', 'table_') INTO source_count;

        IF source_count != tview_count THEN
            RAISE NOTICE 'Data inconsistency in TVIEW %: source=% tview=%', tview_record.entity_name, source_count, tview_count;
        END IF;
    END LOOP;
END $$;
```

### TVIEW Refresh Testing
```sql
-- Test TVIEW refresh functionality
SELECT
    entity_name,
    pg_tviews_refresh(entity_name) as refresh_result,
    last_refresh_duration_ms as refresh_time_ms
FROM pg_tviews_metadata
WHERE last_refreshed < NOW() - INTERVAL '1 hour'
LIMIT 3;
```

## Point-in-Time Recovery Testing

### PITR Testing Procedure
```bash
# Step 1: Create test scenario
TEST_TIME=$(date -d '1 hour ago' +%Y-%m-%d\ %H:%M:%S)
echo "Testing PITR to: $TEST_TIME"

# Step 2: Setup recovery environment
sudo systemctl stop postgresql
sudo mv /var/lib/postgresql/16/main /var/lib/postgresql/16/main.backup

# Step 3: Restore base backup
sudo tar -xzf /backups/base-recent.tar.gz -C /var/lib/postgresql/16/

# Step 4: Configure PITR
sudo vi /var/lib/postgresql/16/main/recovery.conf
# recovery_target_time = '$TEST_TIME'
# restore_command = 'cp /backups/wal/%f %p'

# Step 5: Start recovery
sudo systemctl start postgresql

# Monitor recovery
tail -f /var/log/postgresql/postgresql-16-main.log

# Step 6: Validate recovery point
psql -c "SELECT NOW(), 'Recovery completed to target time' as status;"
```

## Backup Performance Testing

### Backup Duration Monitoring
```sql
-- Monitor backup performance
CREATE TABLE backup_performance_log (
    backup_id SERIAL PRIMARY KEY,
    backup_type TEXT,
    start_time TIMESTAMP,
    end_time TIMESTAMP,
    duration_seconds INTEGER,
    data_size_bytes BIGINT,
    compression_ratio NUMERIC
);

-- Log backup completion
INSERT INTO backup_performance_log (backup_type, start_time, end_time, duration_seconds)
VALUES ('daily_logical', '2025-12-13 02:00:00', NOW(), EXTRACT(EPOCH FROM (NOW() - '2025-12-13 02:00:00')));
```

### Performance Trend Analysis
```sql
-- Analyze backup performance trends
SELECT
    DATE_TRUNC('week', start_time) as week,
    backup_type,
    AVG(duration_seconds) as avg_duration,
    AVG(data_size_bytes / 1024 / 1024 / 1024) as avg_size_gb,
    AVG(compression_ratio) as avg_compression
FROM backup_performance_log
WHERE start_time > NOW() - INTERVAL '3 months'
GROUP BY DATE_TRUNC('week', start_time), backup_type
ORDER BY week DESC;
```

## Compliance Testing

### Retention Policy Validation
```sql
-- Verify retention compliance
SELECT
    'retention_check' as test_type,
    (SELECT COUNT(*) FROM backup_log WHERE backup_date > NOW() - INTERVAL '7 days') as daily_backups_7days,
    (SELECT COUNT(*) FROM backup_log WHERE backup_date > NOW() - INTERVAL '30 days') as daily_backups_30days,
    (SELECT COUNT(*) FROM backup_log WHERE backup_type = 'weekly' AND backup_date > NOW() - INTERVAL '30 days') as weekly_backups_recent,
    CASE
        WHEN (SELECT COUNT(*) FROM backup_log WHERE backup_date > NOW() - INTERVAL '7 days') >= 7 THEN 'COMPLIANT'
        ELSE 'NON_COMPLIANT'
    END as retention_status
FROM backup_log LIMIT 1;
```

### Recovery Time Validation
```sql
-- Track recovery time objectives
CREATE TABLE recovery_time_log (
    recovery_id SERIAL PRIMARY KEY,
    recovery_type TEXT,
    start_time TIMESTAMP,
    end_time TIMESTAMP,
    rto_minutes_target INTEGER,
    rto_minutes_actual INTEGER,
    rpo_minutes_actual INTEGER,
    success BOOLEAN
);

-- Log recovery completion
INSERT INTO recovery_time_log (recovery_type, start_time, end_time, rto_minutes_target, rto_minutes_actual, success)
VALUES (
    'full_restore_test',
    '2025-12-13 10:00:00',
    NOW(),
    60,  -- 1 hour target
    EXTRACT(EPOCH FROM (NOW() - '2025-12-13 10:00:00')) / 60,
    true
);
```

## Automated Testing Framework

### Daily Test Suite
```bash
#!/bin/bash
# Comprehensive daily backup testing

echo "=== Daily Backup Test Suite ==="

# Test 1: Backup existence
echo "1. Backup Existence Test:"
find /backups -name "*.dump" -mtime -1 | wc -l
if [ $(find /backups -name "*.dump" -mtime -1 | wc -l) -eq 0 ]; then
    echo "❌ FAIL: No recent backups found"
    exit 1
fi
echo "✅ PASS"

# Test 2: Backup integrity
echo "2. Backup Integrity Test:"
for backup in $(find /backups -name "*.dump" -mtime -1); do
    if ! pg_restore --list "$backup" >/dev/null 2>&1; then
        echo "❌ FAIL: Corrupt backup $backup"
        exit 1
    fi
done
echo "✅ PASS"

# Test 3: TVIEW presence
echo "3. TVIEW Presence Test:"
TVIEW_COUNT=$(pg_restore --list /backups/mydb-recent.dump | grep -c "pg_tviews_metadata")
if [ $TVIEW_COUNT -eq 0 ]; then
    echo "❌ FAIL: No TVIEW metadata in backup"
    exit 1
fi
echo "✅ PASS: $TVIEW_COUNT TVIEW objects found"

echo "=== All Daily Tests Passed ==="
```

### Monthly Comprehensive Test
```bash
#!/bin/bash
# Monthly full recovery testing

TEST_DB="monthly_recovery_test_$(date +%Y%m%d)"
BACKUP_FILE=$(ls -t /backups/*.dump | head -1)

echo "=== Monthly Recovery Test ==="
echo "Test Database: $TEST_DB"
echo "Backup File: $BACKUP_FILE"

# Create test database
createdb $TEST_DB

# Restore backup
time pg_restore --jobs=4 --dbname=$TEST_DB $BACKUP_FILE

# Run validation tests
psql -d $TEST_DB -f docs/operations/disaster-recovery/scripts/test-recovery.sh

# Cleanup
dropdb $TEST_DB

echo "=== Monthly Recovery Test Completed ==="
```

## Reporting and Documentation

### Test Results Reporting
```sql
-- Generate test results report
SELECT
    test_date,
    test_type,
    success,
    duration_minutes,
    notes
FROM backup_test_results
WHERE test_date > NOW() - INTERVAL '30 days'
ORDER BY test_date DESC;
```

### Failure Analysis
- **Document all test failures**
- **Identify root causes**
- **Implement corrective actions**
- **Update procedures based on lessons learned**

## Continuous Improvement

### Test Enhancement
- **Add new test scenarios** based on incidents
- **Improve test automation** to reduce manual effort
- **Enhance monitoring** to detect issues early
- **Update procedures** based on technology changes

### Performance Optimization
- **Optimize test environments** for faster testing
- **Parallelize tests** where possible
- **Automate repetitive tasks** to reduce human error
- **Monitor test performance** and optimize slow tests

## Related Documentation

- [Backup Types](../backup-strategy/backup-types.md) - Different backup methods
- [Backup Frequency](../backup-strategy/backup-frequency.md) - When backups are created
- [Backup Retention](../backup-strategy/backup-retention.md) - How long backups are kept
- [Full Database Restore](../recovery-procedures/full-database-restore.md) - Recovery procedures</content>
<parameter name="filePath">docs/operations/disaster-recovery/backup-strategy/backup-testing.md