# pg_tviews Operations Runbooks

Procedures for running pg_tviews in production.

## How pg_tviews refreshes (read first)

A TVIEW `tv_<entity>` is a table kept up to date from its backing view `tviews.<schema>__tv_<entity>`. Triggers
on the base tables queue the affected keys in memory, inside the writing transaction,
and the TVIEW rows are refreshed at the end of each statement and on COMMIT. There is
no queue table, no background worker and no refresh schedule. A failed refresh fails
the writing statement and rolls back its transaction.

## Quick Reference

| Category | Runbook | Purpose |
|----------|---------|---------|
| **Health Monitoring** | [TVIEW Health Check](01-health-monitoring/tview-health-check.md) | Installation, registration, triggers, content |
| | [Refresh Queue](01-health-monitoring/queue-management.md) | The in-memory queue: observing it, size limit, blocked writes |
| | [Performance Monitoring](01-health-monitoring/performance-monitoring.md) | Slow writes and slow TVIEW reads |
| **Refresh Operations** | [Manual Refresh](02-refresh-operations/manual-refresh.md) | Rebuild one TVIEW |
| | [Batch Refresh](02-refresh-operations/batch-refresh.md) | Rebuild several TVIEWs, bulk loads |
| | [Refresh Troubleshooting](02-refresh-operations/refresh-troubleshooting.md) | TVIEW not reflecting writes |
| **Maintenance** | [Regular Maintenance](03-maintenance/regular-maintenance.md) | Routine tasks |
| | [Connection Management](03-maintenance/connection-management.md) | Connections and poolers |
| | [Table Analysis](03-maintenance/table-analysis.md) | Table statistics and storage |
| **Incident Response** | [Emergency Procedures](04-incident-response/emergency-procedures.md) | Critical incidents |
| | [Incident Checklist](04-incident-response/incident-checklist.md) | Step-by-step incident response |
| | [Post-Incident Review](04-incident-response/post-incident-review.md) | After incidents |

## Getting Started

### For on-call engineers

1. Run the [TVIEW Health Check](01-health-monitoring/tview-health-check.md)
2. During an outage, follow the [Incident Checklist](04-incident-response/incident-checklist.md)
3. For stale TVIEW data, see [Refresh Troubleshooting](02-refresh-operations/refresh-troubleshooting.md)

### For operations teams

1. Routine: [TVIEW Health Check](01-health-monitoring/tview-health-check.md) and
   [Regular Maintenance](03-maintenance/regular-maintenance.md)
2. Before an incident happens: read [Emergency Procedures](04-incident-response/emergency-procedures.md)

## Supporting Scripts

All read-only. Run the SQL scripts with `psql -X -v ON_ERROR_STOP=1 -d <database> -f <script>`.

- `scripts/health-check.sql` - versions, health check, registry, freshness, physical health, replication
- `scripts/refresh-status.sql` - how writes reach each TVIEW, last content change, suspension state
- `scripts/emergency-disable.sql` - suspension state, and the commands to suspend and resume refresh
- `../upgrade/scripts/pre-upgrade-checks.sh`, `../upgrade/scripts/post-upgrade-validation.sql` - upgrades

## Conventions

- SQL uses schema-qualified names: pg_tviews objects live in schema `tviews`.
- Examples use the entity `user` (`tb_user`, `v_user`, `tv_user`); substitute your own.

## Prerequisites

- PostgreSQL client tools (`psql`)
- A role with SELECT on the `tviews` schema and the TVIEW tables; repair operations
  (refresh, re-register) need the TVIEW owner or a superuser
- Access to the PostgreSQL server log

## Contributing

When updating runbooks:
1. Run every SQL snippet against a database with pg_tviews installed
2. Name only objects that exist (`\df tviews.*`, `\dv tviews.*`, `\dt tviews.*`)
3. Update this README when adding or removing a runbook
