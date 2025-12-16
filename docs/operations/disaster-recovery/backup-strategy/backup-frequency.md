# Backup Frequency Guidelines

## Overview

Backup frequency must balance data protection needs with system performance impact and storage costs. This document provides guidelines for different backup types based on data criticality and change frequency.

## RPO Considerations

### Recovery Point Objective (RPO) by Data Type

| Data Type | Maximum Data Loss | Backup Frequency |
|-----------|-------------------|------------------|
| **Critical Financial** | < 5 minutes | Continuous (WAL) + Hourly |
| **Customer Data** | < 15 minutes | Continuous (WAL) + 15 min |
| **Transactional Data** | < 1 hour | Hourly + WAL |
| **Configuration Data** | < 4 hours | 4-hourly |
| **Historical Data** | < 24 hours | Daily |
| **Static Reference** | < 1 week | Weekly |

## Recommended Backup Schedules

### Small Databases (< 10GB)

#### Primary Schedule
- **WAL Archiving**: Continuous (if high availability needed)
- **Logical Backups**: Daily at 02:00
- **Physical Backups**: Weekly (Sunday 03:00)
- **TVIEW Metadata**: Daily with logical backup

#### Resource Impact
- **Peak Load**: < 10% CPU during backup window
- **Duration**: 5-15 minutes for logical backups
- **Storage**: 2x database size for retention period

### Medium Databases (10GB - 100GB)

#### Primary Schedule
- **WAL Archiving**: Continuous
- **Logical Backups**: Daily at 01:00 (parallel jobs)
- **Physical Backups**: Weekly (Sunday 02:00)
- **Incremental**: Daily at 03:00 (if supported)
- **TVIEW Metadata**: Hourly with configuration changes

#### Resource Impact
- **Peak Load**: < 20% CPU during backup window
- **Duration**: 15-60 minutes for logical backups
- **Storage**: 3x database size for retention period

### Large Databases (> 100GB)

#### Primary Schedule
- **WAL Archiving**: Continuous (mandatory)
- **Physical Backups**: Daily at 22:00 (off-peak)
- **Logical Backups**: Weekly (Sunday 23:00)
- **Incremental**: 4-hourly during business hours
- **TVIEW Metadata**: Real-time with configuration monitoring

#### Resource Impact
- **Peak Load**: < 30% CPU during backup window
- **Duration**: 30-120 minutes for physical backups
- **Storage**: 4x database size for retention period

## TVIEW-Specific Backup Frequency

### TVIEW Metadata
- **Configuration Changes**: Immediate backup after changes
- **Schema Modifications**: Backup before and after changes
- **Regular Schedule**: Daily with main database backup

### TVIEW Data Considerations
- **High-Churn TVIEWs**: More frequent backups if data changes rapidly
- **Critical TVIEWs**: Align with business RPO requirements
- **Large TVIEWs**: Consider incremental strategies

## Backup Window Optimization

### Peak vs. Off-Peak Scheduling

#### Business Hours Considerations
- **User Impact**: Avoid during peak business hours
- **System Load**: Schedule during natural low-usage periods
- **Monitoring**: Ensure alerting covers backup windows

#### Optimal Timing
- **Start Time**: 10 PM - 2 AM local time (avoid international peaks)
- **Duration Buffer**: Add 50% to estimated time for safety
- **Monitoring**: 24/7 coverage during backup windows

### Parallel Backup Strategies

#### Multiple Backup Types
```bash
# Schedule different backup types to minimize impact
# 22:00 - Physical backup (high I/O)
# 23:00 - Logical backup (high CPU)
# 00:00 - WAL maintenance (low impact)
```

#### Resource Balancing
- **I/O Intensive**: Physical backups during off-peak
- **CPU Intensive**: Logical backups during moderate load
- **Network**: Offsite copies during low-usage windows

## Monitoring Backup Frequency

### Automated Monitoring
```sql
-- Create backup frequency monitoring
CREATE OR REPLACE FUNCTION check_backup_frequency()
RETURNS TABLE (
    backup_type TEXT,
    last_backup TIMESTAMP,
    expected_frequency INTERVAL,
    status TEXT,
    next_expected TIMESTAMP
) AS $$
BEGIN
    -- Logical backup check
    RETURN QUERY
    SELECT
        'logical'::TEXT,
        MAX(backup_date)::TIMESTAMP,
        INTERVAL '1 day',
        CASE
            WHEN MAX(backup_date) > NOW() - INTERVAL '25 hours' THEN 'ON_SCHEDULE'
            WHEN MAX(backup_date) > NOW() - INTERVAL '49 hours' THEN 'DELAYED'
            ELSE 'OVERDUE'
        END,
        (MAX(backup_date) + INTERVAL '1 day')::TIMESTAMP
    FROM backup_log WHERE backup_type = 'logical';

    -- Physical backup check
    RETURN QUERY
    SELECT
        'physical'::TEXT,
        MAX(backup_date)::TIMESTAMP,
        INTERVAL '7 days',
        CASE
            WHEN MAX(backup_date) > NOW() - INTERVAL '8 days' THEN 'ON_SCHEDULE'
            WHEN MAX(backup_date) > NOW() - INTERVAL '14 days' THEN 'DELAYED'
            ELSE 'OVERDUE'
        END,
        (MAX(backup_date) + INTERVAL '7 days')::TIMESTAMP
    FROM backup_log WHERE backup_type = 'physical';
END;
$$ LANGUAGE plpgsql;
```

### Alert Thresholds
- **Warning**: Backup overdue by 25% of frequency
- **Critical**: Backup overdue by 50% of frequency
- **Emergency**: Backup overdue by 100% of frequency

## Adjusting Frequency Based on Risk

### Increased Frequency Triggers
- **High Data Volatility**: More frequent backups during peak change periods
- **Recent Incidents**: Increase frequency after data loss events
- **Business Criticality**: Higher frequency for critical systems
- **Regulatory Requirements**: Compliance-driven frequency increases

### Decreased Frequency Considerations
- **Stable Data**: Reduce frequency for rarely changing data
- **Storage Constraints**: Balance frequency with storage costs
- **Performance Impact**: Reduce frequency if backups impact production

## Backup Frequency Testing

### Frequency Validation
```bash
# Test backup scheduling
crontab -l | grep backup

# Verify backup scripts run
ls -la /backups/ | tail -10

# Check backup timestamps
find /backups -name "*.dump" -printf "%T@ %Tc %p\n" | sort -n | tail -5
```

### Performance Impact Assessment
```sql
-- Monitor backup performance impact
SELECT
    query_start,
    query,
    EXTRACT(EPOCH FROM (NOW() - query_start)) as duration_seconds
FROM pg_stat_activity
WHERE query LIKE '%pg_dump%' OR query LIKE '%pg_basebackup%'
ORDER BY query_start DESC
LIMIT 5;
```

## Documentation Requirements

### Backup Schedule Documentation
- [ ] Backup types and frequencies clearly documented
- [ ] Contact information for backup issues
- [ ] Escalation procedures for backup failures
- [ ] Recovery time objectives aligned with backup frequency

### Change Management
- [ ] Backup frequency changes require approval
- [ ] Impact assessment for frequency modifications
- [ ] Testing requirements for frequency changes
- [ ] Rollback procedures for frequency changes

## Related Documentation

- [Backup Types](backup-types.md) - Detailed backup method descriptions
- [Backup Retention](backup-retention.md) - How long to keep backups
- [Backup Testing](backup-testing.md) - Validation procedures
- [Full Database Restore](../recovery-procedures/full-database-restore.md) - Recovery procedures</content>
<parameter name="filePath">docs/operations/disaster-recovery/backup-strategy/backup-frequency.md