# Disaster Recovery Procedures

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

## Overview

This document outlines backup strategies, recovery procedures, and disaster recovery testing for pg_tviews deployments.

## Backup Strategy

### What to Backup

**Critical (Must Backup)**:
- `pg_tview_meta` table (TVIEW definitions and metadata)
- `pg_tview_helpers` table (helper view relationships)
- `pg_tview_audit_log` table (audit trail)
- Base tables (`tb_*`) containing your data

**Optional (Can Recreate)**:
- TVIEW tables (`tv_*`) - can be rebuilt from metadata + base tables
- Backing views (`v_*`) - auto-created from metadata

### Backup Commands

```bash
# Metadata-only backup (fast, small)
pg_dump -t pg_tview_meta -t pg_tview_helpers -t pg_tview_audit_log \
    -d your_db > tview_metadata_backup.sql

# Full database backup
pg_dump -Fc your_db > full_backup_$(date +%Y%m%d_%H%M%S).dump

# Incremental WAL backup (for PITR)
# Configure pg_basebackup or WAL archiving
pg_basebackup -D /backup/location -Ft -z -P
```

### Automated Backup Script

```bash
#!/bin/bash
# backup_tviews.sh

BACKUP_DIR="/backups/pg_tviews"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
DB_NAME="${1:-postgres}"

mkdir -p $BACKUP_DIR

# Backup metadata
pg_dump -t pg_tview_meta -t pg_tview_helpers -t pg_tview_audit_log \
    -d $DB_NAME > $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Compress
gzip $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql

# Cleanup old backups (keep last 30 days)
find $BACKUP_DIR -name "tview_metadata_*.sql.gz" -mtime +30 -delete

echo "Backup completed: $BACKUP_DIR/tview_metadata_$TIMESTAMP.sql.gz"
```

## Recovery Scenarios

### Scenario 1: Corrupted TVIEW Data

**Problem**: tv_* table data is inconsistent with base tables

**Symptoms**:
- Queries return stale or incorrect data
- Health check shows "ERROR" status for metadata consistency

**Recovery**:

```sql
-- Option A: Drop and recreate single TVIEW
DROP TABLE tv_your_entity CASCADE;
CREATE TABLE tv_your_entity AS
SELECT
    tb_your_entity.pk_your_entity,
    tb_your_entity.id,
    jsonb_build_object('id', tb_your_entity.id, 'field', tb_your_entity.field) as data
FROM tb_your_entity;

-- Option B: Full system rebuild (for multiple corrupted TVIEWs)
-- See "Complete System Reset" below
```

### Scenario 2: Lost pg_tview_meta Table

**Problem**: Metadata table deleted or corrupted

**Symptoms**:
- TVIEWs exist but are not recognized by pg_tviews
- Health check shows "ERROR" for metadata
- DDL operations fail

**Recovery**:

```sql
-- Restore from backup
psql -d your_db < tview_metadata_backup.sql

-- Verify restoration
SELECT COUNT(*) FROM pg_tview_meta;

-- Reinstall triggers (they may be missing)
SELECT pg_tviews_install_stmt_triggers();
```

### Scenario 3: Extension Corruption

**Problem**: Extension files corrupted or deleted

**Symptoms**:
- Extension functions return "function does not exist"
- TVIEW creation fails with "extension not available"

**Recovery**:

```bash
# Reinstall extension files
cd /path/to/pg_tviews
cargo pgrx install --release

# In PostgreSQL
DROP EXTENSION pg_tviews CASCADE;
CREATE EXTENSION pg_tviews;

# Restore metadata
psql -d your_db < tview_metadata_backup.sql

# Recreate TVIEWs from metadata
SELECT pg_tviews_create(
    pg_tview_meta.entity,
    pg_tview_meta.definition
)
FROM pg_tview_meta;
```

### Scenario 4: Point-in-Time Recovery (PITR)

**Problem**: Need to restore to specific point in time

**Requirements**:
- WAL archiving enabled
- Base backup available
- Target recovery timestamp known

**Recovery**:

```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Restore base backup
sudo -u postgres rm -rf /var/lib/postgresql/data
sudo -u postgres tar -xzf /backup/base.tar.gz -C /var/lib/postgresql/data

# Configure recovery
sudo tee /var/lib/postgresql/data/recovery.conf > /dev/null <<EOF
restore_command = 'cp /backup/wal/%f %p'
recovery_target_time = '2025-12-10 14:30:00'
EOF

# Start PostgreSQL (will replay WAL to target time)
sudo systemctl start postgresql

# Verify TVIEWs
SELECT COUNT(*) FROM pg_tview_meta;
SELECT * FROM pg_tviews_health_check();
```

### Scenario 5: Complete Data Loss

**Problem**: Entire database lost or corrupted

**Recovery**:

```bash
# Restore from full backup
pg_restore -d postgres full_backup.dump

# Or from compressed dump
gunzip -c full_backup.dump.gz | pg_restore -d postgres

# Verify TVIEWs
SELECT * FROM pg_tviews_health_check();
```

## Emergency Procedures

### Complete System Reset

**Use only as last resort - will lose all TVIEW data**

```sql
-- 1. Stop all application connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = current_database()
  AND pid != pg_backend_pid();

-- 2. Drop all TVIEWs
DO $$
DECLARE
    rec record;
BEGIN
    FOR rec IN SELECT entity FROM pg_tview_meta LOOP
        EXECUTE 'DROP TABLE tv_' || rec.entity || ' CASCADE';
    END LOOP;
END $$;

-- 3. Clean metadata
TRUNCATE pg_tview_meta;
TRUNCATE pg_tview_helpers;
TRUNCATE pg_tview_audit_log;

-- 4. Recreate TVIEWs from application scripts
-- (Run your TVIEW creation scripts)

-- 5. Verify system health
SELECT * FROM pg_tviews_health_check();
```

### Force TVIEW Recreation

When TVIEWs exist but metadata is lost:

```sql
-- 1. Find all TVIEW tables
SELECT schemaname, tablename
FROM pg_tables
WHERE tablename LIKE 'tv_%'
  AND schemaname = 'public';

-- 2. For each TVIEW, recreate metadata
-- This is manual - inspect each TVIEW to determine its definition
DO $$
DECLARE
    tv_name text;
    entity_name text;
BEGIN
    FOR tv_name IN
        SELECT tablename FROM pg_tables
        WHERE tablename LIKE 'tv_%' AND schemaname = 'public'
    LOOP
        entity_name := substring(tv_name from 4); -- Remove 'tv_' prefix

        -- Insert metadata (you need to know the original definition)
        -- This is tricky without the original definition
        RAISE NOTICE 'TVIEW % needs metadata recreation', tv_name;
    END LOOP;
END $$;
```

## Recovery Time Objectives

### RTO (Recovery Time Objective)
- **Metadata-only recovery**: < 5 minutes
- **Single TVIEW recovery**: < 15 minutes
- **Full system recovery**: < 2 hours
- **Complete rebuild**: < 4 hours

### RPO (Recovery Point Objective)
- **With WAL archiving**: < 5 minutes data loss
- **With streaming replication**: < 1 minute data loss
- **Without replication**: Up to last backup

## Testing Recovery Procedures

### Quarterly Recovery Drills

```bash
#!/bin/bash
# recovery_test.sh

DB_NAME="pg_tviews_recovery_test"
BACKUP_FILE="/tmp/recovery_test_backup.sql"

# 1. Create test database
createdb $DB_NAME

# 2. Create sample TVIEWs
psql -d $DB_NAME << 'EOF'
CREATE EXTENSION pg_tviews;

CREATE TABLE tb_test (pk_test SERIAL PRIMARY KEY, id UUID DEFAULT gen_random_uuid(), name TEXT);
INSERT INTO tb_test (name) VALUES ('test1'), ('test2');

CREATE TABLE tv_test AS
SELECT pk_test, id, jsonb_build_object('id', id, 'name', name) as data FROM tb_test;
EOF

# 3. Backup metadata
pg_dump -t pg_tview_meta -d $DB_NAME > $BACKUP_FILE

# 4. Simulate disaster
psql -d $DB_NAME -c "DROP EXTENSION pg_tviews CASCADE;"

# 5. Recover
psql -d $DB_NAME -c "CREATE EXTENSION pg_tviews;"
psql -d $DB_NAME < $BACKUP_FILE

# 6. Verify
psql -d $DB_NAME -c "SELECT * FROM pg_tviews_health_check();"

# 7. Cleanup
dropdb $DB_NAME
rm $BACKUP_FILE

echo "Recovery test completed successfully"
```

### Automated Health Monitoring

```bash
#!/bin/bash
# health_monitor.sh

DB_NAME="${1:-postgres}"
ALERT_EMAIL="admin@yourcompany.com"

# Run health check
HEALTH_OUTPUT=$(psql -d $DB_NAME -t -c "SELECT * FROM pg_tviews_health_check();")

# Check for issues
if echo "$HEALTH_OUTPUT" | grep -q "ERROR\|WARNING"; then
    echo "$HEALTH_OUTPUT" | mail -s "pg_tviews Health Alert" $ALERT_EMAIL
    echo "Health issues detected - alert sent"
else
    echo "All systems healthy"
fi
```

## Prevention Best Practices

### Regular Backups
- **Metadata**: Hourly automated backups
- **Full database**: Daily backups
- **WAL archiving**: Continuous for PITR capability

### Monitoring
- **Health checks**: Every 5 minutes
- **Backup verification**: Daily restore tests
- **Performance monitoring**: Continuous

### Configuration
```sql
-- Enable WAL archiving for PITR
ALTER SYSTEM SET wal_level = 'replica';
ALTER SYSTEM SET archive_mode = 'on';
ALTER SYSTEM SET archive_command = 'cp %p /backup/wal/%f';

-- Configure backup retention
ALTER SYSTEM SET wal_keep_size = '1GB';
```

## See Also

- [Backup and Restore](https://www.postgresql.org/docs/current/backup.html) - PostgreSQL official documentation
- [Monitoring Guide](../MONITORING.md) - Health check details
- [Operations Guide](operations.md) - Production procedures