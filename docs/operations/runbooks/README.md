# pg_tviews Operations Runbooks

This directory contains comprehensive operational procedures for managing pg_tviews in production environments.

## Quick Reference

| Category | Runbook | Purpose | Frequency |
|----------|---------|---------|-----------|
| **Health Monitoring** | [TVIEW Health Check](01-health-monitoring/tview-health-check.md) | Verify TVIEW synchronization | Every 4 hours |
| | [Queue Management](01-health-monitoring/queue-management.md) | Monitor and manage refresh queues | Daily |
| | [Performance Monitoring](01-health-monitoring/performance-monitoring.md) | Track refresh performance | Hourly |
| **Refresh Operations** | [Manual Refresh](02-refresh-operations/manual-refresh.md) | Refresh individual TVIEWs | As needed |
| | [Batch Refresh](02-refresh-operations/batch-refresh.md) | Refresh multiple TVIEWs | Scheduled |
| | [Refresh Troubleshooting](02-refresh-operations/refresh-troubleshooting.md) | Debug refresh issues | When needed |
| **Maintenance** | [Regular Maintenance](03-maintenance/regular-maintenance.md) | Routine maintenance tasks | Weekly |
| | [Connection Management](03-maintenance/connection-management.md) | Monitor database connections | Daily |
| | [Table Analysis](03-maintenance/table-analysis.md) | Analyze table statistics | Monthly |
| **Incident Response** | [Emergency Procedures](04-incident-response/emergency-procedures.md) | Handle critical incidents | As needed |
| | [Incident Checklist](04-incident-response/incident-checklist.md) | Systematic incident response | During incidents |
| | [Post-Incident Review](04-incident-response/post-incident-review.md) | Learn from incidents | After incidents |

## Getting Started

### For On-Call Engineers

1. **Health Check**: Start with [TVIEW Health Check](01-health-monitoring/tview-health-check.md) for routine monitoring
2. **Incident Response**: Use [Incident Checklist](04-incident-response/incident-checklist.md) during outages
3. **Common Issues**: Check [Refresh Troubleshooting](02-refresh-operations/refresh-troubleshooting.md) for refresh problems

### For Operations Teams

1. **Daily Tasks**: Review [Queue Management](01-health-monitoring/queue-management.md) and [Connection Management](03-maintenance/connection-management.md)
2. **Weekly Tasks**: Follow [Regular Maintenance](03-maintenance/regular-maintenance.md)
3. **Emergency Prep**: Familiarize with [Emergency Procedures](04-incident-response/emergency-procedures.md)

## Supporting Scripts

All runbooks reference executable SQL scripts in the `scripts/` directory:

- `health-check.sql` - Comprehensive health verification
- `refresh-status.sql` - Current refresh status
- `queue-cleanup.sql` - Safe queue maintenance
- `emergency-disable.sql` - Emergency TVIEW disable

## Conventions

### Command Format
- **SQL commands** are shown in code blocks with syntax highlighting
- **Shell commands** use `$` prefix for local commands
- **Database commands** use `psql>` prefix for interactive sessions

### Parameterization
- All scripts use parameterized queries (no hardcoded database names)
- Environment variables used for configuration
- Examples show both parameterized and concrete usage

### Error Handling
- Each procedure includes expected errors and solutions
- Rollback procedures provided for reversible operations
- Escalation paths defined for complex issues

## Prerequisites

### Database Access
- PostgreSQL client tools (`psql`, `pg_isready`)
- Database connection credentials
- Appropriate permissions (SELECT on system tables, TVIEW operations)

### Monitoring Tools
- Access to PostgreSQL logs
- System monitoring (CPU, memory, disk I/O)
- Alerting system integration

### Knowledge Requirements
- Basic PostgreSQL administration
- Understanding of TVIEW concepts
- Familiarity with your specific database schema

## Emergency Contacts

When procedures don't resolve issues:

1. **Database Team**: For PostgreSQL-specific issues
2. **Application Team**: For TVIEW schema changes
3. **Infrastructure Team**: For system-level problems
4. **Vendor Support**: For pg_tviews extension issues

## Contributing

When updating runbooks:
1. Test procedures in staging environment
2. Update supporting scripts if needed
3. Include rollback procedures for new operations
4. Update this README if adding new runbooks

## Version History

- **v1.0**: Initial comprehensive runbook set
- Covers all major operational scenarios
- Tested procedures with error handling
- Supporting automation scripts included</content>
<parameter name="filePath">docs/operations/runbooks/README.md