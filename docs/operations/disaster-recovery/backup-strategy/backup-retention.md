# Backup Retention Policy

## Overview

Backup retention balances data protection requirements with storage costs and compliance obligations. This policy defines how long different types of backups are retained based on business needs, regulatory requirements, and technical constraints.

## Retention Categories

### Operational Retention (Short-term)

#### Daily Backups
- **Retention Period**: 7-14 days
- **Purpose**: Recovery from recent data loss or corruption
- **Storage**: High-performance storage (local/NAS)
- **Access**: Immediate (minutes)

#### Weekly Backups
- **Retention Period**: 4-8 weeks
- **Purpose**: Recovery from extended outages or complex issues
- **Storage**: Medium-performance storage
- **Access**: Fast (hours)

### Compliance Retention (Medium-term)

#### Monthly Backups
- **Retention Period**: 12 months
- **Purpose**: Regulatory compliance and audit requirements
- **Storage**: Cost-effective storage (cloud/object storage)
- **Access**: Standard (days)

#### Quarterly Backups
- **Retention Period**: 7 years (typical compliance requirement)
- **Purpose**: Long-term regulatory and legal requirements
- **Storage**: Archive storage (glacier/deep archive)
- **Access**: Slow (weeks)

## Backup Type-Specific Retention

### Logical Backups (pg_dump)
- **Daily**: 14 days (frequent schema changes)
- **Weekly**: 8 weeks (monthly equivalent)
- **Monthly**: 12 months
- **Total Retention**: 14 months

### Physical Backups (pg_basebackup)
- **Daily**: 7 days (storage intensive)
- **Weekly**: 4 weeks
- **Monthly**: 12 months
- **Total Retention**: 13 months

### WAL Archives
- **Retention**: Until corresponding base backup expires
- **Minimum**: 30 days (safety buffer)
- **Maximum**: 90 days (storage optimization)
- **Pruning**: Automatic based on base backup lifecycle

### TVIEW Metadata Backups
- **Daily**: 30 days (configuration changes)
- **Weekly**: 12 weeks (6 months)
- **Monthly**: 12 months
- **Total Retention**: 18 months

## Retention Policy by Environment

### Production Environment
- **Critical Systems**: Maximum retention (7 years)
- **Standard Systems**: 3 years minimum
- **Development**: 6 months (cost optimization)

### Staging/Test Environments
- **Retention**: 30-90 days
- **Purpose**: Testing and validation
- **Storage**: Minimal cost storage

### Development Environments
- **Retention**: 7-30 days
- **Purpose**: Code changes and debugging
- **Storage**: Local storage only

## Storage Tier Strategy

### Hot Storage (Immediate Access)
- **Retention**: 0-30 days
- **Storage Type**: Local disks, high-performance NAS
- **Cost**: High
- **Access Time**: < 5 minutes
- **Use Case**: Daily operational recovery

### Warm Storage (Fast Access)
- **Retention**: 30-365 days
- **Storage Type**: Cloud storage (S3, Azure Blob)
- **Cost**: Medium
- **Access Time**: < 1 hour
- **Use Case**: Monthly compliance recovery

### Cold Storage (Archive)
- **Retention**: 1-7 years
- **Storage Type**: Glacier, Deep Archive
- **Cost**: Low
- **Access Time**: 1-24 hours
- **Use Case**: Long-term compliance

## Automated Retention Management

### Retention Policy Implementation
```bash
# Example retention script
#!/bin/bash

# Configuration
BACKUP_DIR="/backups"
RETENTION_DAILY=14
RETENTION_WEEKLY=8
RETENTION_MONTHLY=12

# Clean daily backups (older than 14 days)
find $BACKUP_DIR/daily -name "*.dump" -mtime +$RETENTION_DAILY -delete

# Clean weekly backups (older than 8 weeks)
find $BACKUP_DIR/weekly -name "*.dump" -mtime +$(($RETENTION_WEEKLY * 7)) -delete

# Clean monthly backups (older than 12 months)
find $BACKUP_DIR/monthly -name "*.dump" -mtime +$(($RETENTION_MONTHLY * 30)) -delete

# Log cleanup actions
echo "$(date): Cleaned old backups" >> /var/log/backup-cleanup.log
```

### Monitoring Retention Compliance
```sql
-- Create retention monitoring function
CREATE OR REPLACE FUNCTION check_backup_retention()
RETURNS TABLE (
    backup_type TEXT,
    retention_days INTEGER,
    oldest_backup DATE,
    days_overdue INTEGER,
    status TEXT
) AS $$
BEGIN
    -- Daily backup retention
    RETURN QUERY
    SELECT
        'daily'::TEXT,
        14,
        MIN(backup_date)::DATE,
        GREATEST(0, EXTRACT(DAY FROM NOW() - MIN(backup_date))::INTEGER - 14),
        CASE
            WHEN MIN(backup_date) < NOW() - INTERVAL '21 days' THEN 'CRITICAL'
            WHEN MIN(backup_date) < NOW() - INTERVAL '17 days' THEN 'WARNING'
            ELSE 'COMPLIANT'
        END
    FROM backup_log WHERE backup_type = 'daily' AND backup_date > NOW() - INTERVAL '30 days';

    -- Weekly backup retention
    RETURN QUERY
    SELECT
        'weekly'::TEXT,
        56,  -- 8 weeks
        MIN(backup_date)::DATE,
        GREATEST(0, EXTRACT(DAY FROM NOW() - MIN(backup_date))::INTEGER - 56),
        CASE
            WHEN MIN(backup_date) < NOW() - INTERVAL '70 days' THEN 'CRITICAL'
            WHEN MIN(backup_date) < NOW() - INTERVAL '63 days' THEN 'WARNING'
            ELSE 'COMPLIANT'
        END
    FROM backup_log WHERE backup_type = 'weekly' AND backup_date > NOW() - INTERVAL '100 days';
END;
$$ LANGUAGE plpgsql;
```

## Compliance Considerations

### Regulatory Requirements
- **GDPR**: 7 years for personal data
- **SOX**: 7 years for financial records
- **HIPAA**: 7 years for healthcare data
- **PCI DSS**: 1 year minimum for cardholder data

### Business Requirements
- **Contractual Obligations**: Check client contracts
- **Industry Standards**: Follow industry-specific requirements
- **Internal Policies**: Adhere to company data retention policies

## Cost Optimization

### Storage Cost Analysis
```sql
-- Calculate backup storage costs
SELECT
    'backup_storage_costs' as metric,
    SUM(pg_size_pretty(size_bytes)) as total_size,
    COUNT(*) as backup_count,
    ROUND(AVG(size_bytes) / 1024 / 1024 / 1024, 2) as avg_gb_per_backup,
    '$' || ROUND(SUM(size_bytes) * 0.02 / 1024 / 1024 / 1024 / 1024, 2) as monthly_cost_estimate
FROM (
    SELECT
        (SELECT size FROM pg_stat_file((settings.setting || '/' || name)) LIMIT 1) as size_bytes
    FROM pg_ls_dir('/backups') AS files(name)
    CROSS JOIN (SELECT setting FROM pg_settings WHERE name = 'data_directory') as settings
    WHERE name LIKE '%.dump' OR name LIKE '%.backup'
) as backup_sizes;
```

### Retention Optimization
- **Compression**: Use maximum compression to reduce storage
- **Deduplication**: Implement deduplication where possible
- **Tiering**: Move older backups to cheaper storage
- **Archiving**: Compress and archive old backups

## Exception Handling

### Extended Retention Requests
1. **Legal Hold**: Preserve data for legal proceedings
2. **Audit Requests**: Maintain data for compliance audits
3. **Business Requirements**: Extended retention for specific business needs

### Retention Policy Exceptions
- **Approval Required**: Exceptions require management approval
- **Documentation**: All exceptions must be documented
- **Review**: Exceptions reviewed annually
- **Cost Impact**: Exception costs tracked separately

## Testing Retention Procedures

### Retention Testing
```bash
# Test retention script in dry-run mode
./cleanup-backups.sh --dry-run

# Verify retention calculations
find /backups -name "*.dump" -printf "%T@ %Tc %p\n" | sort -n

# Test restore from oldest retained backup
./restore-backup.sh /path/to/oldest/backup
```

### Compliance Auditing
- **Quarterly Reviews**: Audit retention compliance
- **Annual Assessments**: Full retention policy review
- **Incident Response**: Verify retention during data loss events

## Documentation Requirements

### Retention Policy Documentation
- [ ] Clear retention periods for each backup type
- [ ] Storage tier definitions and costs
- [ ] Exception handling procedures
- [ ] Compliance requirements mapping

### Change Management
- [ ] Retention policy changes require approval
- [ ] Impact assessment for retention changes
- [ ] Communication plan for policy changes
- [ ] Training for operations team

## Related Documentation

- [Backup Types](backup-types.md) - Different backup methods
- [Backup Frequency](backup-frequency.md) - When backups are created
- [Backup Testing](backup-testing.md) - Validation procedures
- [Full Database Restore](../recovery-procedures/full-database-restore.md) - Recovery procedures</content>
<parameter name="filePath">docs/operations/disaster-recovery/backup-strategy/backup-retention.md