# Incident Checklist Runbook

## Purpose
A step-by-step process for pg_tviews incidents, so each one is handled the same
way and fully resolved.

## When to Use
- **Writes failing** with TVIEW refresh errors
- **Stale or empty TVIEWs** reported by users or checks
- **Slow writes** attributed to TVIEW refresh
- **Health check warnings/errors** from monitoring

## Prerequisites
- Access to the incident tracker, the PostgreSQL log and `psql`
- Contact list for stakeholders

## Stage 1: Detection & Assessment (5 minutes)

### Step 1: Confirm the incident
- [ ] Reproduce or confirm the symptom (error text, stale row, latency)
- [ ] Identify the affected TVIEWs and base tables
- [ ] Classify severity (table below) and open a ticket

### Step 2: Collect initial data
```sql
SELECT now() AS assessed_at,
       tviews.pg_tviews_version() AS pg_tviews,
       current_setting('server_version') AS postgresql,
       (SELECT count(*) FROM tviews.registry) AS tviews,
       (SELECT count(*) FROM tviews.registry WHERE needs_reregister) AS needs_reregister,
       (SELECT count(*) FROM tviews.pg_tviews_health_check() WHERE severity = 'error') AS health_errors,
       (SELECT count(*) FROM tviews.pg_tviews_health_check() WHERE severity = 'warning') AS health_warnings,
       (SELECT count(*) FROM pg_stat_activity
        WHERE datname = current_database() AND wait_event_type = 'Lock') AS sessions_waiting_on_locks;
```

Save the output of [health-check.sql](../scripts/health-check.sql) and
[refresh-status.sql](../scripts/refresh-status.sql) to the ticket.

### Step 3: Severity classification

| Severity | Criteria | Response Time | Communication |
|----------|----------|---------------|---------------|
| **SEV 1** | Writes failing for the application, or TVIEWs empty in production | 15 min | Management notification |
| **SEV 2** | Stale data visible to users, or writes much slower | 1 hour | Team lead notification |
| **SEV 3** | Stale data in a non-critical TVIEW, health warnings | 4 hours | Team notification |
| **SEV 4** | No user impact | 24 hours | Document only |

## Stage 2: Investigation (15-30 minutes)

### Step 4: Analyse
- [ ] PostgreSQL log: refresh errors carry the failing statement in `CONTEXT`
- [ ] Recent changes: deployments, extension upgrade, TVIEW definition changes, bulk loads
- [ ] Resource use: CPU, I/O, locks

### Step 5: Diagnostic queries
```sql
-- Health problems
SELECT component, severity, message
FROM tviews.pg_tviews_health_check() WHERE severity <> 'info';

-- Tables whose writes cannot reach a TVIEW
SELECT name, uncascaded_tables, options->>'uncascaded_policy' AS uncascaded_policy
FROM tviews.registry WHERE cardinality(uncascaded_tables) > 0;

-- Empty TVIEWs (UNLOGGED after a crash, or on a standby)
SELECT * FROM tviews.pg_tviews_replication_status() WHERE is_empty OR needs_rebuild;
```

For a suspect TVIEW, compare it with its view (step 4 of
[post-upgrade-validation.sql](../../upgrade/scripts/post-upgrade-validation.sql))
and see [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md).

### Step 6: Root cause
- [ ] Failing view expression on new data
- [ ] Write not mapped to TVIEW keys (uncascaded table, missing triggers)
- [ ] Missing index on a join or mapping query, or high fan-out
- [ ] Lock contention between writers refreshing the same TVIEW rows

## Stage 3: Containment (30-60 minutes)

### Step 7: Mitigate
- [ ] Unblock writers if needed: defer refresh to the end of the writing
      transaction ([Emergency Procedures](emergency-procedures.md), Action 1)
- [ ] Clear blocking sessions
- [ ] Tell stakeholders which TVIEWs may be stale meanwhile

### Step 8: Restore service
- [ ] Roll back the change that caused it, if any
- [ ] Re-register after an upgrade: `SELECT * FROM tviews.pg_tviews_reregister_all();`
- [ ] Undo any suspension
- [ ] Refresh affected TVIEWs: `SELECT tviews.pg_tviews_refresh_all();`

## Stage 4: Resolution (1-4 hours)

### Step 9: Permanent fix
- [ ] Fix the definition (`pg_tviews_create_or_replace`), data, indexes or configuration
- [ ] Test in staging, then apply

### Step 10: Validate
```sql
SELECT component, severity, message
FROM tviews.pg_tviews_health_check() WHERE severity <> 'info';   -- expect no rows
```
- [ ] Every TVIEW matches its view (post-upgrade-validation.sql step 4: 0 rows differing)
- [ ] Writes succeed at normal latency
- [ ] Monitor for 30-60 minutes; confirm with the users who reported it

## Stage 5: Closure (30 minutes)
- [ ] Document what happened, the root cause and the fix
- [ ] Notify stakeholders of resolution and impact
- [ ] Record follow-up actions and schedule a [Post-Incident Review](post-incident-review.md) for SEV 1-2
- [ ] Close the ticket

## Common Incident Patterns

### Pattern 1: Refresh errors fail writes
**Quick diagnosis**: the client error and the PostgreSQL log (`CONTEXT` names
`tviews.<schema>__tv_<entity>` / `tv_<entity>`).
**Resolution**: [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md), Issue 1.

### Pattern 2: Stale TVIEW rows
**Quick diagnosis**: TVIEW-vs-view comparison; `tviews.registry.uncascaded_tables`;
`needs_reregister`.
**Resolution**: [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md), Issue 2.

### Pattern 3: Slow writes
**Quick diagnosis**: `tviews.pg_tviews_profile()` warnings, `pg_stat_statements`.
**Resolution**: [Performance Monitoring](../01-health-monitoring/performance-monitoring.md).

### Pattern 4: Empty TVIEWs after crash or failover
**Quick diagnosis**: `tviews.pg_tviews_replication_status()`.
**Resolution**: `SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true);`, or keep
the TVIEW LOGGED (the default; `ALTER TABLE tv_<entity> SET LOGGED` switches an UNLOGGED one).

### Pattern 5: Connection issues
**Quick diagnosis**: `pg_stat_activity` counts.
**Resolution**: [Connection Management](../03-maintenance/connection-management.md).

## Escalation
- **Time**: not resolved within the severity's response time
- **Scope**: more TVIEWs or systems affected than assessed
- **Product defect**: report at
  [github.com/fraiseql/pg_tviews/issues](https://github.com/fraiseql/pg_tviews/issues) with the
  version, the error text and the TVIEW definition (`SELECT name, query FROM tviews.registry`)

## Related Runbooks
- [Emergency Procedures](emergency-procedures.md) - Critical incidents
- [Post-Incident Review](post-incident-review.md) - After resolution
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Initial assessment
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Technical issues
