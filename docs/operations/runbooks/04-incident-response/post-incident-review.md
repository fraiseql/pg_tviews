# Post-Incident Review Runbook

## Purpose
Analyse a pg_tviews incident to find its root cause, improve the response, and
prevent it from recurring.

## When to Use
- **SEV 1 and SEV 2 incidents**: always
- **Recurring issues**: the same symptom more than once
- **Response problems**: when a runbook or tool did not help

## Prerequisites
- The incident ticket, with the diagnostics saved during the incident
- The participants
- PostgreSQL logs for the incident window

## Stage 1: Preparation (30 minutes)

### Step 1: Reconstruct the timeline
Record when the incident started, was detected, was contained and was resolved,
and every action taken. Sources:
- the PostgreSQL log (refresh errors, slow statements, restarts);
- the ticket and chat history;
- deployments, migrations, extension upgrades and bulk loads in the window.

pg_tviews keeps no history of refresh errors. What the database can still show:

```sql
-- When each TVIEW's rows last changed (per TVIEW)
SELECT max(updated_at) AS last_change FROM public.tv_user;

-- Installed versions (the time of an upgrade is not recorded)
SELECT tviews.pg_tviews_version() AS library,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;
```

If `pg_tviews.audit_enabled` was on, `tviews.pg_tview_audit_log` holds the TVIEW
creations, drops and committed refreshes of the window:

```sql
SELECT performed_at, operation, entity, rows_affected, performed_by
FROM tviews.pg_tview_audit_log
WHERE performed_at BETWEEN now() - interval '1 day' AND now()
ORDER BY performed_at;
```

### Step 2: Assess impact
- Duration of failing writes, stale data or slow writes
- Affected TVIEWs, applications and users
- Data impact: TVIEWs are derived; base-table data is affected only if writes were
  lost by the application during failures

### Step 3: Collect data
- [ ] Logs and error texts
- [ ] Diagnostics saved during the incident (health check, refresh status)
- [ ] TVIEW definitions involved (`SELECT name, query FROM tviews.registry`)
- [ ] Actions taken and their effect

## Stage 2: Root Cause Analysis (1 hour)

### Step 4: 5-Why analysis
Ask "why" from the symptom until you reach a cause you can act on:

1. **Why did the incident occur?** *[Immediate cause, e.g. writes failed on a refresh error]*
2. **Why did that happen?** *[e.g. the view divided by a column that became zero]*
3. **Why did that happen?** *[e.g. new data not covered by the definition]*
4. **Why was it not caught?** *[e.g. no test with that data]*
5. **Why?** *[Root cause, e.g. definition changes are not tested against production-like data]*

### Step 5: Contributing factors
Classify factors as people, process, technology or environment. pg_tviews-specific
ones to check:
- [ ] A base table no cascade reaches (`tviews.registry.uncascaded_tables`) under the `warn` policy
- [ ] Re-registration skipped after an upgrade (`needs_reregister`)
- [ ] Refresh suspended (`pg_tviews.suspend_triggers`) and not followed by a refresh
- [ ] UNLOGGED TVIEWs read on a standby or after a crash
- [ ] Missing indexes, or high fan-out (`tviews.pg_tviews_profile()`)
- [ ] A health check warning ignored before the incident

### Step 6: Root cause
- [ ] Primary cause agreed
- [ ] Contributing causes listed
- [ ] Preventable causes identified

## Stage 3: Response Review (45 minutes)

### Step 7: Timeline analysis
Compare detection, containment and resolution times with the severity's response
time in the [Incident Checklist](incident-checklist.md).

### Step 8: Process adherence
- [ ] Detected promptly?
- [ ] Severity correct?
- [ ] Stakeholders informed?
- [ ] Escalated at the right time?
- [ ] Documented?

### Step 9: Runbooks and tools
- [ ] Did the runbooks match the system? Fix any step that failed.
- [ ] Were the needed tools and access available?
- [ ] Could a check have detected it earlier (health check, TVIEW-vs-view comparison)?

## Stage 4: Improvements (45 minutes)

### Step 10: Corrective actions
Fix this incident's cause: definition, data, index, configuration, missing
re-registration.

### Step 11: Preventive measures
Examples:
- schedule [health-check.sql](../scripts/health-check.sql) and alert on warnings;
- run the TVIEW-vs-view comparison (step 4 of
  [post-upgrade-validation.sql](../../upgrade/scripts/post-upgrade-validation.sql))
  after deployments;
- create TVIEWs with `pg_tviews.uncascaded_policy = 'error'` so unmappable
  definitions are rejected;
- make TVIEWs read on standbys logged (`pg_tviews_set_logged`).

### Step 12: Process improvements
- [ ] Detection: monitoring and alerts
- [ ] Response: escalation and communication
- [ ] Resolution: runbook fixes
- [ ] Prevention: tests and reviews

## Stage 5: Action Planning (30 minutes)

### Step 13: Assign actions
Track each action in the team's tracker with an owner, priority and target date.

### Step 14: Timeline
- [ ] Immediate actions: within 1 week
- [ ] Short-term: within 1 month
- [ ] Longer-term: within 3-6 months

### Step 15: Success measures
- [ ] The same incident does not recur
- [ ] Time to detect and resolve decreases

## Post-Incident Review Template

### Incident Summary
- **Incident ID**:
- **Date/Time**:
- **Duration**:
- **Severity**:
- **Affected TVIEWs / systems**:
- **Business impact**:

### What Happened
- **Trigger**:
- **Symptoms**:
- **Scope**:
- **Detection**:

### Root Cause
- **Primary cause**:
- **Contributing factors**:
- **Prevention gaps**:

### Response Analysis
- **What went well**:
- **What did not**:
- **Timeline**:

### Lessons Learned
- **Technical**:
- **Process**:

### Action Items
| Action | Owner | Priority | Target Date | Status |
|--------|-------|----------|-------------|--------|
| | | | | |

## Guidelines
- Blameless: focus on systems and processes, not people
- Evidence-based: conclusions from logs and data
- Specific actions with owners, not broad intentions
- Complete the review while details are fresh

## Related Runbooks
- [Incident Checklist](incident-checklist.md) - Incident response process
- [Emergency Procedures](emergency-procedures.md) - Crisis response
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Ongoing monitoring
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Proactive monitoring
