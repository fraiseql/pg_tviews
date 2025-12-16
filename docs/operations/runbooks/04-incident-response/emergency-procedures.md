# Emergency Procedures Runbook

## Purpose
Handle critical pg_tviews incidents that require immediate action to restore service availability and prevent data loss.

## When to Use
- **CRITICAL**: TVIEW system completely unavailable
- **CRITICAL**: Data corruption detected in TVIEWs
- **CRITICAL**: Refresh operations failing catastrophically
- **CRITICAL**: System performance impacting business operations
- **CRITICAL**: Security incidents affecting TVIEW data

## Prerequisites
- **EMERGENCY ACCESS**: Root/database admin privileges
- **BACKUP ACCESS**: Recent database backups available
- **COMMUNICATION**: Incident response team contact information
- **AUTHORIZATION**: Executive approval for emergency actions
- **DOCUMENTATION**: Runbook access and incident logging

## Emergency Assessment (2 minutes)

### Step 1: Confirm Emergency Status
```sql
-- Quick system status check
SELECT
    'EMERGENCY ASSESSMENT' as status,
    NOW() as assessment_time,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as tviews_with_errors,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections
FROM pg_stat_bgwriter;
```

### Step 2: Determine Emergency Level
```sql
-- Evaluate emergency severity
SELECT
    CASE
        WHEN (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) > (SELECT COUNT(*) FROM pg_tviews_metadata) * 0.5 THEN 'CRITICAL: Majority of TVIEWs failing'
        WHEN (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) > 10000 THEN 'CRITICAL: Massive refresh backlog'
        WHEN (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'waiting') > (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.8 THEN 'CRITICAL: Connection exhaustion'
        WHEN (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_refreshed < NOW() - INTERVAL '4 hours') > 0 THEN 'HIGH: TVIEWs stale'
        ELSE 'MONITOR: Issues detected but not critical'
    END as emergency_level,
    CASE
        WHEN (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) > (SELECT COUNT(*) FROM pg_tviews_metadata) * 0.5 THEN 'EXECUTE_EMERGENCY_PROTOCOL'
        WHEN (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) > 10000 THEN 'EXECUTE_EMERGENCY_PROTOCOL'
        WHEN (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'waiting') > (SELECT setting FROM pg_settings WHERE name = 'max_connections')::integer * 0.8 THEN 'EXECUTE_EMERGENCY_PROTOCOL'
        ELSE 'FOLLOW_STANDARD_INCIDENTS'
    END as recommended_action
FROM pg_stat_bgwriter;
```

## Critical Emergency Protocol (15 minutes)

### WARNING: Emergency Actions Have Risks
**These procedures may cause:**
- Temporary service unavailability
- Data loss (if backups are inadequate)
- Performance degradation
- Need for system restart

**Requirements for proceeding:**
- [ ] Executive approval obtained
- [ ] Incident documented with timestamp
- [ ] Backup verification completed
- [ ] Rollback plan prepared

### Step 1: Emergency TVIEW Disable
```sql
-- CRITICAL: Disable all TVIEW operations immediately
-- This stops refreshes and prevents further issues

-- Option 1: Quick disable via emergency script
\i docs/operations/runbooks/scripts/emergency-disable.sql

-- Option 2: Manual disable (if script unavailable)
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE query LIKE '%tview%' OR query LIKE '%refresh%';

-- Clear all pending refreshes
DELETE FROM pg_tviews_queue WHERE processed_at IS NULL;

-- Mark TVIEWs as emergency disabled
UPDATE pg_tviews_metadata
SET emergency_disabled = true,
    emergency_disable_time = NOW(),
    emergency_disable_reason = 'Critical incident response';
```

### Step 2: System Stabilization
```sql
-- Terminate problematic connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE state IN ('idle in transaction', 'waiting')
  AND query_start < NOW() - INTERVAL '30 minutes';

-- Reset connection pool if using pgbouncer
-- (External command, adjust for your setup)
-- sudo systemctl restart pgbouncer

-- Check system resources
SELECT * FROM pg_stat_bgwriter;
SELECT * FROM pg_stat_database WHERE datname = current_database();
```

### Step 3: Data Integrity Verification
```sql
-- Verify TVIEW data integrity (sample check)
DO $$
DECLARE
    tview_record RECORD;
    integrity_issues INTEGER := 0;
BEGIN
    FOR tview_record IN SELECT entity_name FROM pg_tviews_metadata LIMIT 5 LOOP
        BEGIN
            EXECUTE 'SELECT COUNT(*) FROM ' || tview_record.entity_name || ' LIMIT 1';
        EXCEPTION WHEN OTHERS THEN
            integrity_issues := integrity_issues + 1;
            RAISE NOTICE 'Integrity issue with TVIEW: % - %', tview_record.entity_name, SQLERRM;
        END;
    END LOOP;

    IF integrity_issues > 0 THEN
        RAISE EXCEPTION 'Data integrity issues detected in % TVIEWs. Do not proceed with restart.', integrity_issues;
    END IF;
END $$;
```

## Service Restoration Procedures

### Option 1: Controlled Restart (30 minutes)
```sql
-- For issues that require PostgreSQL restart

-- Step 1: Verify backup integrity
-- (External command - adjust for your backup system)
-- pg_restore --list /path/to/backup | head -20

-- Step 2: Graceful shutdown
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE pid != pg_backend_pid();

-- Step 3: Shutdown PostgreSQL
-- sudo systemctl stop postgresql

-- Step 4: Start PostgreSQL
-- sudo systemctl start postgresql

-- Step 5: Verify startup
SELECT version();
SELECT COUNT(*) FROM pg_tviews_metadata;
```

### Option 2: TVIEW System Reset (20 minutes)
```sql
-- For TVIEW-specific issues without full restart

-- Step 1: Clear all TVIEW state
TRUNCATE pg_tviews_queue;

-- Step 2: Reset TVIEW metadata
UPDATE pg_tviews_metadata
SET last_error = NULL,
    emergency_disabled = false,
    emergency_disable_time = NULL,
    emergency_disable_reason = NULL;

-- Step 3: Re-enable TVIEW system
-- (If you have a system enable function)
-- SELECT pg_tviews_system_enable();

-- Step 4: Test basic functionality
SELECT pg_tviews_health_check();
```

### Option 3: Backup Restoration (2-4 hours)
```sql
-- For data corruption or severe issues

-- Step 1: Identify good backup
-- List available backups and timestamps
-- ls -la /backup/path/*.backup

-- Step 2: Restore to temporary instance
-- createdb tview_restore
-- pg_restore -d tview_restore /path/to/good/backup

-- Step 3: Verify restore integrity
-- psql -d tview_restore -c "SELECT COUNT(*) FROM pg_tviews_metadata;"

-- Step 4: Plan production restoration
-- Coordinate with application teams for downtime window
```

## Communication During Emergency

### Immediate Notifications
- **Incident Response Team**: Alert via phone/pager
- **Application Teams**: Notify of TVIEW unavailability
- **Business Stakeholders**: Alert if impacting operations
- **Customer Support**: Prepare for user inquiries

### Status Updates
- **Every 15 minutes**: Critical incidents
- **Every 30 minutes**: High-priority incidents
- **Hourly**: Medium-priority incidents

### Communication Template
```
EMERGENCY TVIEW INCIDENT - [TIMESTAMP]

Status: [ACTIVE/MITIGATED/RESOLVED]
Impact: [Description of user/business impact]
ETA: [Estimated resolution time]
Workaround: [Any available alternatives]
Contact: [Incident coordinator]
```

## Post-Emergency Verification

### Step 1: System Health Check
```sql
-- Comprehensive post-emergency verification
SELECT
    'POST-EMERGENCY VERIFICATION' as check_type,
    NOW() as verification_time,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NULL) as healthy_tviews,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as tviews_with_errors,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as avg_refresh_time_ms
FROM pg_stat_bgwriter;
```

### Step 2: Application Testing
```sql
-- Test critical application queries
-- Replace with your actual critical queries

-- Test 1: Basic TVIEW access
SELECT COUNT(*) FROM your_critical_tview LIMIT 1;

-- Test 2: Recent data availability
SELECT COUNT(*) FROM your_critical_tview
WHERE updated_at > NOW() - INTERVAL '1 hour';

-- Test 3: Refresh functionality
SELECT pg_tviews_refresh('your_critical_tview');
```

### Step 3: Performance Validation
```sql
-- Ensure performance is acceptable post-emergency
SELECT
    query,
    mean_time / 1000 as mean_time_seconds,
    calls
FROM pg_stat_statements
WHERE query LIKE '%tview%'
  AND mean_time > 5000  -- Flag queries > 5 seconds
ORDER BY mean_time DESC
LIMIT 5;
```

## Emergency Decision Framework

### When to Declare Emergency
- [ ] TVIEW system completely unavailable (> 50% TVIEWs failing)
- [ ] Critical business operations impacted
- [ ] Data loss or corruption detected
- [ ] Security breach affecting TVIEW data
- [ ] System performance < 10% of normal

### Emergency Action Checklist
- [ ] Confirm emergency status with leadership
- [ ] Document incident with timestamp and symptoms
- [ ] Alert incident response team
- [ ] Execute appropriate emergency procedure
- [ ] Communicate status to stakeholders
- [ ] Begin post-incident analysis

### Emergency Exit Criteria
- [ ] TVIEW system operational (> 95% TVIEWs healthy)
- [ ] Critical application functions working
- [ ] Performance within acceptable ranges
- [ ] No active security threats
- [ ] Incident response team approval

## Related Runbooks

- [Incident Checklist](incident-checklist.md) - Systematic incident response
- [Post-Incident Review](post-incident-review.md) - After-action analysis
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - System health verification
- [Emergency Disable Script](../scripts/emergency-disable.sql) - Automated emergency actions

## Emergency Contacts

**Primary Escalation:**
- Incident Response Coordinator: [phone/email]
- Database Administration: [phone/email]
- Application Development: [phone/email]

**Secondary Escalation:**
- IT Operations Manager: [phone/email]
- Business Continuity Officer: [phone/email]
- Executive Leadership: [phone/email]

**Vendor Support:**
- pg_tviews Support: [contact information]
- PostgreSQL Community: [forums/mailing lists]

## Best Practices

1. **Prepare in Advance**: Test emergency procedures regularly
2. **Document Everything**: Log all actions and communications
3. **Communicate Frequently**: Keep stakeholders informed
4. **Test Restorations**: Regularly verify backup integrity
5. **Learn from Incidents**: Conduct thorough post-mortems
6. **Automate Where Possible**: Use scripts to reduce human error
7. **Have Multiple Options**: Prepare fallback procedures</content>
<parameter name="filePath">docs/operations/runbooks/04-incident-response/emergency-procedures.md