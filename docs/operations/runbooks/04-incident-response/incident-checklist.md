# Incident Checklist Runbook

## Purpose
Provide a systematic, step-by-step process for responding to pg_tviews incidents, ensuring consistent handling and complete resolution.

## When to Use
- **Any TVIEW Issue**: From minor performance degradation to major outages
- **User Reports**: When users report TVIEW-related problems
- **Monitoring Alerts**: When automated monitoring detects issues
- **Scheduled Maintenance Issues**: When maintenance activities uncover problems
- **Pre-Production**: Before deploying changes that affect TVIEWs

## Prerequisites
- Access to incident tracking system
- Contact information for all stakeholders
- Access to monitoring dashboards
- Runbook access and documentation
- Authorization to perform incident response actions

## Phase 1: Incident Detection & Assessment (5 minutes)

### Step 1: Confirm Incident
- [ ] **Symptom Verification**: Confirm the reported issue exists
- [ ] **Scope Assessment**: Determine which TVIEWs/systems are affected
- [ ] **Severity Evaluation**: Assess business impact (see severity matrix below)
- [ ] **Incident Logging**: Create incident ticket with initial assessment

### Step 2: Initial Data Collection
```sql
-- Gather initial diagnostic information
SELECT
    'INCIDENT ASSESSMENT' as phase,
    NOW() as assessment_time,
    (SELECT COUNT(*) FROM pg_tviews_metadata) as total_tviews,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as tviews_with_errors,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections
FROM pg_stat_bgwriter;
```

### Step 3: Severity Classification

| Severity | Criteria | Response Time | Communication |
|----------|----------|---------------|---------------|
| **SEV 1** | System completely unavailable, data loss, security breach | Immediate (15 min) | Executive notification |
| **SEV 2** | Major functionality broken, significant user impact | 1 hour | Management notification |
| **SEV 3** | Minor functionality issues, limited user impact | 4 hours | Team notification |
| **SEV 4** | Cosmetic issues, no functional impact | 24 hours | Document only |

## Phase 2: Incident Investigation (15-30 minutes)

### Step 4: Detailed Problem Analysis
- [ ] **Error Log Review**: Check PostgreSQL and application logs
- [ ] **System Metrics**: Review CPU, memory, disk, and network usage
- [ ] **Query Analysis**: Identify slow or failing queries
- [ ] **Configuration Check**: Verify TVIEW and PostgreSQL settings
- [ ] **Recent Changes**: Review recent deployments or configuration changes

### Step 5: Diagnostic Queries
```sql
-- Comprehensive incident diagnostics
SELECT
    'INVESTIGATION' as phase,
    NOW() as investigation_time,

    -- TVIEW health
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as error_count,
    (SELECT MAX(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as max_refresh_time_ms,

    -- Queue status
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_queue,
    (SELECT MAX(EXTRACT(EPOCH FROM (NOW() - created_at))) FROM pg_tviews_queue WHERE processed_at IS NULL) as oldest_pending_seconds,

    -- System status
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'waiting') as waiting_connections,
    (SELECT sum(blks_read) + sum(blks_hit) FROM pg_stat_database WHERE datname = current_database()) as recent_io
FROM pg_stat_bgwriter;
```

### Step 6: Root Cause Identification
- [ ] **Pattern Recognition**: Look for common failure patterns
- [ ] **Timeline Analysis**: Correlate with system events
- [ ] **Dependency Check**: Verify upstream/downstream system health
- [ ] **Resource Analysis**: Check for resource exhaustion
- [ ] **Code Review**: Examine recent changes if applicable

## Phase 3: Incident Containment (30-60 minutes)

### Step 7: Immediate Mitigation
- [ ] **Stop the Bleeding**: Prevent further damage or impact
- [ ] **Service Isolation**: Contain issue to affected systems
- [ ] **Resource Allocation**: Ensure adequate resources for resolution
- [ ] **Communication**: Update stakeholders on containment status

### Step 8: Temporary Workarounds
```sql
-- Implement temporary fixes based on root cause

-- Example: For refresh failures
-- Pause problematic refreshes
UPDATE pg_tviews_metadata
SET emergency_disabled = true
WHERE last_error LIKE '%specific_error_pattern%';

-- Example: For performance issues
-- Reduce refresh frequency temporarily
-- (Adjust based on your system's configuration options)

-- Example: For connection issues
-- Terminate problematic connections
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE state = 'idle in transaction'
  AND query_start < NOW() - INTERVAL '30 minutes';
```

### Step 9: Service Restoration
- [ ] **Rollback Changes**: If recent changes caused the issue
- [ ] **Configuration Reset**: Restore known-good configurations
- [ ] **Service Restart**: Restart affected services if safe
- [ ] **Verification**: Confirm services are restored

## Phase 4: Incident Resolution (1-4 hours)

### Step 10: Permanent Fix Implementation
- [ ] **Root Cause Fix**: Implement the actual solution
- [ ] **Testing**: Verify fix works in staging environment
- [ ] **Deployment**: Apply fix to production
- [ ] **Validation**: Confirm issue is resolved

### Step 11: Comprehensive Testing
```sql
-- Post-fix validation
SELECT
    'RESOLUTION VALIDATION' as phase,
    NOW() as validation_time,

    -- Verify TVIEW health
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NULL) as healthy_tviews,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as remaining_errors,

    -- Verify queue processing
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL AND created_at > NOW() - INTERVAL '1 hour') as recent_pending,

    -- Verify performance
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '30 minutes') as avg_refresh_time_ms
FROM pg_stat_bgwriter;
```

### Step 12: Monitoring Period
- [ ] **Observation**: Monitor system for 30-60 minutes post-fix
- [ ] **Performance Check**: Ensure no performance degradation
- [ ] **Error Monitoring**: Watch for new issues
- [ ] **User Verification**: Confirm user-reported issues are resolved

## Phase 5: Incident Closure (30 minutes)

### Step 13: Documentation Update
- [ ] **Incident Summary**: Document what happened, when, and impact
- [ ] **Root Cause**: Clearly identify the cause
- [ ] **Resolution Steps**: Detail how the issue was fixed
- [ ] **Prevention Measures**: Note any preventive actions taken
- [ ] **Lessons Learned**: Document insights for future incidents

### Step 14: Stakeholder Communication
- [ ] **Resolution Notification**: Inform stakeholders of resolution
- [ ] **Impact Summary**: Provide summary of user/business impact
- [ ] **Prevention Plans**: Outline measures to prevent recurrence
- [ ] **Follow-up Actions**: Note any required follow-up tasks

### Step 15: Incident Closure
- [ ] **Ticket Resolution**: Close incident ticket
- [ ] **Knowledge Base Update**: Add to known issues if applicable
- [ ] **Metrics Update**: Update incident metrics and reporting
- [ ] **Team Debrief**: Conduct brief retrospective if needed

## Incident Response Checklist

### Pre-Incident Preparation
- [ ] Incident response plan reviewed quarterly
- [ ] Contact lists current and accessible
- [ ] Runbooks tested and up-to-date
- [ ] Monitoring alerts configured and tested
- [ ] Backup procedures verified monthly

### During Incident
- [ ] Incident logged with unique identifier
- [ ] Severity assessed and appropriate response initiated
- [ ] Stakeholders notified based on severity
- [ ] Investigation completed systematically
- [ ] Root cause identified before implementing fixes
- [ ] Temporary workarounds implemented to reduce impact
- [ ] Permanent fix tested before production deployment
- [ ] Resolution validated and monitored

### Post-Incident
- [ ] Incident documented completely
- [ ] Root cause analysis completed
- [ ] Preventive measures implemented
- [ ] Lessons learned shared with team
- [ ] Process improvements identified and tracked

## Common Incident Patterns

### Pattern 1: Refresh Failures
**Symptoms**: TVIEWs show errors, refresh operations fail
**Common Causes**: Permission issues, source table changes, connection problems
**Quick Diagnosis**: Check `pg_tviews_metadata.last_error`
**Resolution**: See [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md)

### Pattern 2: Performance Degradation
**Symptoms**: Slow queries, high CPU/memory usage
**Common Causes**: Table bloat, missing indexes, resource contention
**Quick Diagnosis**: Check `pg_stat_user_tables` and `pg_stat_activity`
**Resolution**: See [Performance Monitoring](../01-health-monitoring/performance-monitoring.md)

### Pattern 3: Queue Backlog
**Symptoms**: Growing refresh queue, stale TVIEW data
**Common Causes**: Refresh failures, system overload, configuration issues
**Quick Diagnosis**: Check `pg_tviews_queue` counts and ages
**Resolution**: See [Queue Management](../01-health-monitoring/queue-management.md)

### Pattern 4: Connection Issues
**Symptoms**: Connection timeouts, pool exhaustion
**Common Causes**: Application connection leaks, pool misconfiguration
**Quick Diagnosis**: Check `pg_stat_activity` connection counts
**Resolution**: See [Connection Management](../03-maintenance/connection-management.md)

## Escalation Guidelines

### When to Escalate
- **Time Thresholds**: Issue not resolved within severity-based timeframes
- **Scope Expansion**: Issue affects more systems than initially assessed
- **Business Impact**: Issue causes significant business disruption
- **Resource Needs**: Incident requires resources beyond team capabilities

### Escalation Contacts
- **Level 1**: Team Lead (immediate supervisor)
- **Level 2**: Department Manager (business impact)
- **Level 3**: Executive Leadership (major incidents)
- **External**: Vendor Support (product-specific issues)

## Related Runbooks

- [Emergency Procedures](emergency-procedures.md) - For critical incidents
- [Post-Incident Review](post-incident-review.md) - After incident resolution
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Initial assessment
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Technical issues

## Metrics and Reporting

### Incident Metrics to Track
- **MTTR (Mean Time To Resolution)**: Average time to resolve incidents
- **MTTD (Mean Time To Detection)**: Average time to detect incidents
- **Incident Count by Severity**: Distribution of incident types
- **Recurring Issues**: Incidents that happen repeatedly
- **Business Impact**: Financial and operational impact of incidents

### Monthly Reporting
- [ ] Incident summary with trends
- [ ] Top incident types and causes
- [ ] Process improvements implemented
- [ ] Upcoming preventive measures

## Best Practices

1. **Stay Calm**: Systematic process prevents panic decisions
2. **Document Everything**: Complete documentation aids resolution and prevention
3. **Communicate Frequently**: Keep stakeholders informed throughout
4. **Follow the Process**: Don't skip steps even under pressure
5. **Learn Continuously**: Each incident improves future response
6. **Automate Where Possible**: Use scripts and tools to reduce human error
7. **Review Regularly**: Conduct post-mortems and implement improvements</content>
<parameter name="filePath">docs/operations/runbooks/04-incident-response/incident-checklist.md