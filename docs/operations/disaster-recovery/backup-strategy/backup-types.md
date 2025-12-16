# Backup Types for pg_tviews

## Overview

pg_tviews requires a multi-layered backup strategy to protect both PostgreSQL data and TVIEW-specific metadata. This document outlines four backup types optimized for different recovery scenarios.

## 1. Logical Backups (pg_dump)

### Purpose
Complete database export in SQL format, ideal for portability and selective recovery.

### Advantages
- ✅ **Version Independent**: Can restore to different PostgreSQL versions
- ✅ **Human Readable**: Can inspect and modify backup contents
- ✅ **Selective Restore**: Restore specific tables, schemas, or objects
- ✅ **Compression**: Built-in compression reduces storage needs
- ✅ **TVIEW Compatible**: Preserves all TVIEW metadata and configurations

### Disadvantages
- ❌ **Slow on Large Databases**: 100GB+ databases take significant time
- ❌ **No Point-in-Time Recovery**: Can only restore to backup time
- ❌ **Resource Intensive**: High CPU and memory usage during backup

### When to Use
- Regular nightly backups for most environments
- Before major schema changes or upgrades
- Cross-version migrations (PostgreSQL 15→16→17)
- Development and testing environment backups
- Databases under 100GB

### Implementation

#### Full Database Backup
```bash
# Standard full backup with compression
sudo -u postgres pg_dump \
    --compress=9 \
    --format=custom \
    --verbose \
    --file=/backups/mydb-$(date +%Y%m%d_%H%M%S).dump \
    mydb

# Verify backup integrity
sudo -u postgres pg_restore \
    --list /backups/mydb-*.dump | tail -5
```

#### Parallel Backup (Faster)
```bash
# Use multiple jobs for large databases
sudo -u postgres pg_dump \
    --compress=9 \
    --format=directory \
    --jobs=4 \
    --verbose \
    --file=/backups/mydb-$(date +%Y%m%d_%H%M%S) \
    mydb

# Directory format allows parallel processing
ls -la /backups/mydb-20251213_020000/
```

#### Schema-Only Backup
```bash
# Backup only TVIEW-related schemas
sudo -u postgres pg_dump \
    --schema-only \
    --compress=9 \
    --format=custom \
    --verbose \
    --file=/backups/tview-schemas-$(date +%Y%m%d).dump \
    mydb
```

### Restore Procedures

#### Full Restore
```bash
# Create fresh database
sudo -u postgres createdb mydb_restored

# Restore from backup
sudo -u postgres pg_restore \
    --verbose \
    --dbname=mydb_restored \
    /backups/mydb-20251213.dump

# Verify TVIEWs are present
psql -d mydb_restored -c "SELECT COUNT(*) FROM pg_tviews_metadata;"
```

#### Selective Restore
```bash
# Restore only specific TVIEW
sudo -u postgres pg_restore \
    --table=my_tview \
    --verbose \
    --dbname=mydb \
    /backups/mydb-20251213.dump
```

## 2. Physical Backups (pg_basebackup)

### Purpose
File-level copy of PostgreSQL data directory, optimized for speed and point-in-time recovery capability.

### Advantages
- ✅ **Fast**: Minimal impact on running database
- ✅ **Complete**: Includes all database objects and configurations
- ✅ **PITR Ready**: Foundation for point-in-time recovery
- ✅ **Consistent**: Atomic snapshot of database state
- ✅ **TVIEW Safe**: Preserves all TVIEW internal structures

### Disadvantages
- ❌ **Version Dependent**: Must restore to same PostgreSQL major version
- ❌ **Large**: Full data directory size (no compression)
- ❌ **Storage Intensive**: Requires significant backup storage

### When to Use
- High-availability environments with streaming replication
- Large databases (100GB+) where speed is critical
- Point-in-time recovery requirements
- Disaster recovery scenarios
- Production systems requiring minimal backup impact

### Implementation

#### Base Backup Creation
```bash
# Create base backup with WAL inclusion
sudo -u postgres pg_basebackup \
    --pgdata=/backups/base-$(date +%Y%m%d_%H%M%S) \
    --format=tar \
    --compress=9 \
    --verbose \
    --checkpoint=fast \
    --wal-method=stream

# Verify backup
tar -tzf /backups/base-20251213_020000.tar.gz | head -10
```

#### Incremental Backup (with rsync)
```bash
# For large databases, consider incremental backups
rsync -av --delete \
    --exclude=pg_wal \
    --exclude=pg_log \
    /var/lib/postgresql/16/main/ \
    /backups/incremental-$(date +%Y%m%d_%H%M%S)/
```

### Restore Procedures

#### Full Physical Restore
```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Clear old data directory
sudo rm -rf /var/lib/postgresql/16/main/*

# Extract backup
sudo tar -xzf /backups/base-20251213.tar.gz -C /var/lib/postgresql/16/main/

# Fix permissions
sudo chown -R postgres:postgres /var/lib/postgresql/16/main/

# Start PostgreSQL
sudo systemctl start postgresql

# Verify
psql -c "SELECT pg_tviews_version();"
```

## 3. WAL Archiving (Continuous Archiving)

### Purpose
Continuous archiving of Write-Ahead Log files to enable point-in-time recovery.

### Advantages
- ✅ **Minimal Data Loss**: Recovery to any point in time
- ✅ **Continuous Protection**: Near real-time backup
- ✅ **Efficient**: Only changed data is archived
- ✅ **TVIEW Transaction Safe**: Captures all TVIEW operations

### Disadvantages
- ❌ **Complex Setup**: Requires WAL archiving configuration
- ❌ **Storage Intensive**: Continuous WAL file generation
- ❌ **Recovery Complex**: Requires base backup + WAL files

### When to Use
- Critical systems requiring minimal data loss (RPO < 5 minutes)
- Financial or regulatory environments
- High-transaction-volume systems
- Zero-downtime requirements

### Implementation

#### WAL Archiving Setup
```bash
# Edit postgresql.conf
sudo vi /etc/postgresql/16/main/postgresql.conf

# Add these settings:
# wal_level = replica
# archive_mode = on
# archive_command = 'cp %p /backups/wal/%f'
# archive_timeout = 60

# Create WAL archive directory
sudo mkdir -p /backups/wal
sudo chown postgres:postgres /backups/wal

# Restart PostgreSQL
sudo systemctl restart postgresql
```

#### WAL Archive Maintenance
```bash
# Monitor WAL archive growth
du -sh /backups/wal/

# Clean old WAL files (keep last 7 days)
find /backups/wal -name "*.gz" -mtime +7 -delete

# Compress WAL files to save space
find /backups/wal -name "000000*" -exec gzip {} \;
```

### Restore Procedures

#### Point-in-Time Recovery
```bash
# Stop PostgreSQL
sudo systemctl stop postgresql

# Restore base backup
sudo rm -rf /var/lib/postgresql/16/main/*
sudo tar -xzf /backups/base-20251213.tar.gz -C /var/lib/postgresql/16/main/

# Create recovery.conf
sudo vi /var/lib/postgresql/16/main/recovery.conf
# restore_command = 'cp /backups/wal/%f %p'
# recovery_target_time = '2025-12-13 14:30:00'

# Start PostgreSQL (will enter recovery mode)
sudo systemctl start postgresql

# Monitor recovery progress
tail -f /var/log/postgresql/postgresql-16-main.log
```

## 4. TVIEW Metadata Backups

### Purpose
Specialized backups of TVIEW configurations and metadata, separate from main database backups.

### Advantages
- ✅ **Fast Recovery**: Quick restoration of TVIEW configurations
- ✅ **Selective**: Can restore individual TVIEW metadata
- ✅ **Version Independent**: Metadata format is stable
- ✅ **Small Size**: Minimal storage requirements

### Disadvantages
- ❌ **Incomplete**: Only metadata, not TVIEW data
- ❌ **Dependency**: Requires source tables to exist
- ❌ **Manual Process**: Requires recreation of TVIEWs

### When to Use
- Frequent TVIEW configuration changes
- Development environments with rapid iteration
- Backup of TVIEW definitions before major changes
- Documentation of TVIEW configurations

### Implementation

#### Metadata Export
```sql
-- Export TVIEW metadata
COPY (
    SELECT
        entity_name,
        primary_key_column,
        created_at,
        last_refreshed,
        last_refresh_duration_ms,
        last_error
    FROM pg_tviews_metadata
) TO '/backups/tview-metadata-$(date +%Y%m%d).csv' WITH CSV HEADER;
```

#### TVIEW Definition Export
```sql
-- Export TVIEW creation scripts
DO $$
DECLARE
    tview_record RECORD;
    script_content TEXT := '';
BEGIN
    FOR tview_record IN SELECT entity_name FROM pg_tviews_metadata LOOP
        -- This is conceptual - actual implementation depends on how TVIEWs are created
        script_content := script_content || 'SELECT pg_tviews_convert_existing_table(''' || tview_record.entity_name || ''');' || E'\n';
    END LOOP;

    -- Write to file (requires superuser or file access)
    -- Note: This is simplified - actual implementation would use external tools
    RAISE NOTICE 'TVIEW recreation script: %', script_content;
END $$;
```

### Restore Procedures

#### Metadata Restore
```sql
-- Restore TVIEW metadata (after database restore)
-- Note: This assumes the TVIEWs themselves need recreation
COPY pg_tviews_metadata (
    entity_name,
    primary_key_column,
    created_at,
    last_refreshed,
    last_refresh_duration_ms,
    last_error
) FROM '/backups/tview-metadata-20251213.csv' WITH CSV HEADER;
```

## Backup Strategy Recommendations

### Small Databases (< 10GB)
- **Primary**: Daily logical backups
- **Secondary**: Weekly physical backups
- **WAL**: Optional for critical systems

### Medium Databases (10GB - 100GB)
- **Primary**: Daily logical backups
- **Secondary**: Weekly physical backups + WAL archiving
- **Retention**: 30 days daily, 1 year weekly

### Large Databases (> 100GB)
- **Primary**: Daily physical backups + WAL archiving
- **Secondary**: Weekly logical backups for portability
- **Retention**: 14 days daily, 6 months weekly

### TVIEW-Specific Considerations
- **Metadata**: Include in all backup types
- **Configurations**: Backup TVIEW definitions separately
- **Testing**: Regularly test TVIEW recovery from backups
- **Documentation**: Document custom TVIEW configurations

## Monitoring and Alerting

### Backup Success Monitoring
```sql
-- Create backup monitoring function
CREATE OR REPLACE FUNCTION monitor_backups()
RETURNS TABLE (
    backup_type TEXT,
    last_success TIMESTAMP,
    status TEXT,
    recommendation TEXT
) AS $$
BEGIN
    -- Logical backup check
    RETURN QUERY
    SELECT
        'logical'::TEXT,
        (SELECT MAX(backup_date) FROM backup_log WHERE backup_type = 'logical'),
        CASE
            WHEN (SELECT MAX(backup_date) FROM backup_log WHERE backup_type = 'logical') > NOW() - INTERVAL '25 hours'
            THEN 'HEALTHY'
            ELSE 'OVERDUE'
        END,
        'Ensure daily logical backups are running'::TEXT;

    -- Physical backup check
    RETURN QUERY
    SELECT
        'physical'::TEXT,
        (SELECT MAX(backup_date) FROM backup_log WHERE backup_type = 'physical'),
        CASE
            WHEN (SELECT MAX(backup_date) FROM backup_log WHERE backup_type = 'physical') > NOW() - INTERVAL '7 days'
            THEN 'HEALTHY'
            ELSE 'OVERDUE'
        END,
        'Ensure weekly physical backups are running'::TEXT;
END;
$$ LANGUAGE plpgsql;
```

### Automated Verification
```bash
# Daily backup verification script
#!/bin/bash
# Check backup files exist and are recent
find /backups -name "*.dump" -mtime -1 | wc -l
find /backups -name "*base*" -mtime -1 | wc -l

# Alert if backups are missing
if [ $(find /backups -name "*.dump" -mtime -1 | wc -l) -eq 0 ]; then
    echo "CRITICAL: No recent logical backups found"
    # Send alert
fi
```

## Testing Backup Integrity

### Regular Testing Schedule
- **Daily**: Automated backup existence checks
- **Weekly**: Backup restoration to test environment
- **Monthly**: Full disaster recovery simulation
- **Quarterly**: TVIEW-specific recovery testing

### Backup Testing Procedure
```bash
# Create test environment
createdb test_restore

# Restore backup
pg_restore -d test_restore /backups/mydb-recent.dump

# Verify TVIEWs
psql -d test_restore -c "
SELECT COUNT(*) as tviews_restored FROM pg_tviews_metadata;
SELECT pg_tviews_health_check();
"

# Clean up
dropdb test_restore
```

## Security Considerations

### Backup Encryption
```bash
# Encrypt backups at rest
openssl enc -aes-256-cbc -salt -in backup.dump -out backup.dump.enc -k $ENCRYPTION_KEY

# Decrypt for restore
openssl enc -d -aes-256-cbc -in backup.dump.enc -out backup.dump -k $ENCRYPTION_KEY
```

### Access Control
- Limit backup file access to authorized personnel
- Use separate credentials for backup operations
- Audit backup access and modifications
- Store encryption keys securely

## Cost Optimization

### Storage Tiering
- **Hot Storage**: Recent backups (last 7 days)
- **Warm Storage**: Weekly backups (last 30 days)
- **Cold Storage**: Monthly backups (last 12 months)
- **Archive Storage**: Yearly backups (compliance retention)

### Compression Strategies
- Use maximum compression for logical backups
- Consider deduplication for physical backups
- Compress WAL files after archiving
- Balance compression ratio vs. restore speed

## Related Documentation

- [Backup Frequency](backup-frequency.md) - When to perform different backup types
- [Backup Retention](backup-retention.md) - How long to keep different backups
- [Backup Testing](backup-testing.md) - Procedures for testing backup integrity
- [Full Database Restore](../recovery-procedures/full-database-restore.md) - Complete recovery procedures</content>
<parameter name="filePath">docs/operations/disaster-recovery/backup-strategy/backup-types.md