# pg_tviews Disaster Recovery Procedures

This directory contains comprehensive disaster recovery procedures for pg_tviews deployments, including backup strategies, recovery procedures, and incident response runbooks.

## Recovery Objectives

### RTO (Recovery Time Objective)
- **Critical Systems**: < 15 minutes
- **Standard Systems**: < 1 hour
- **Extended Recovery**: < 4 hours

### RPO (Recovery Point Objective)
- **Critical Data**: < 5 minutes data loss
- **Standard Data**: < 15 minutes data loss
- **Archival Data**: < 1 hour data loss

## Quick Reference

| Scenario | Primary Procedure | RTO | RPO | Difficulty |
|----------|-------------------|-----|-----|------------|
| **Data Corruption** | [Incident Checklist](../runbooks/04-incident-response/incident-checklist.md) | 30-60 min | 0-15 min | Medium |
| **Hardware Failure** | [Emergency Procedures](../runbooks/04-incident-response/emergency-procedures.md) | 15-60 min | 0-5 min | High |
| **Primary Lost** | [Replication and Failover](../replication.md) | 5-30 min | 0 min | Medium |
| **Full Database Loss** | [Full Database Restore](recovery-procedures/full-database-restore.md) | 30-120 min | 0-15 min | High |
| **Point-in-Time Recovery** | [PostgreSQL PITR](https://www.postgresql.org/docs/current/continuous-archiving.html), then [Full Database Restore](recovery-procedures/full-database-restore.md) from step 4 | 60-180 min | Custom | High |
| **TVIEW Corruption** | [Manual Refresh](../runbooks/02-refresh-operations/manual-refresh.md) | 10-30 min | 0 min | Medium |

## Backup Strategy Overview

### Backup Types
- **[Logical Backups](backup-strategy/backup-types.md)**: pg_dump for portability and flexibility
- **[Physical Backups](backup-strategy/backup-types.md)**: File-level for speed and PITR capability
- **[WAL Archiving](backup-strategy/backup-types.md)**: Continuous archiving for point-in-time recovery
- **[TVIEW Metadata](backup-strategy/backup-types.md)**: Specialized backups for TVIEW configurations

### Backup Schedule
- **Hourly**: WAL archiving (continuous)
- **Daily**: Incremental physical backups
- **Weekly**: Full logical backups
- **Monthly**: Full physical backups + offsite storage

### Retention Policy
- **Daily Backups**: 7 days
- **Weekly Backups**: 4 weeks
- **Monthly Backups**: 12 months
- **Yearly Backups**: 7 years (compliance)

## Recovery Procedures

### Database Recovery
- **[Full Database Restore](recovery-procedures/full-database-restore.md)**: Complete cluster recovery
- **Point-in-time recovery** is PostgreSQL's
  ([continuous archiving](https://www.postgresql.org/docs/current/continuous-archiving.html));
  pg_tviews needs nothing more than its catalog restored with the rest.

### TVIEW-Specific Recovery
- **[Manual Refresh](../runbooks/02-refresh-operations/manual-refresh.md)**: Rebuild a stale or
  corrupted TVIEW and the TVIEWs that embed it

### High Availability
- **[Replication and Failover](../replication.md)**: Standbys, promotion and the TVIEW rebuild after it

## Incident Response Runbooks

- **[Incident Checklist](../runbooks/04-incident-response/incident-checklist.md)**: Identify and assess the issue
- **[Emergency Procedures](../runbooks/04-incident-response/emergency-procedures.md)**: Contain and recover
- **[Post-Incident Review](../runbooks/04-incident-response/post-incident-review.md)**: Lessons learned

### Response Framework
Each runbook follows a structured approach:
1. **Detection**: How to identify the issue
2. **Assessment**: Evaluate impact and severity
3. **Containment**: Prevent further damage
4. **Recovery**: Restore service
5. **Lessons Learned**: Post-incident analysis

## Supporting Scripts

All procedures reference executable scripts in the `scripts/` directory:

- `create-backup.sh` - Automated backup creation
- `restore-backup.sh` - Backup restoration procedures
- `verify-backup.sh` - Backup integrity validation
- `test-recovery.sh` - Recovery procedure testing
- `cleanup-after-recovery.sh` - Post-recovery cleanup

## Prerequisites

### Infrastructure Requirements
- [ ] Backup storage with sufficient capacity (3x database size)
- [ ] Offsite backup storage for disaster scenarios
- [ ] Backup verification processes
- [ ] Monitoring and alerting for backup failures
- [ ] Recovery testing environment

### Team Preparation
- [ ] DR coordinator identified and trained
- [ ] Recovery team roles and responsibilities defined
- [ ] Contact lists current and accessible
- [ ] Communication templates prepared
- [ ] Regular DR training and testing

### Documentation Requirements
- [ ] Recovery procedures reviewed quarterly
- [ ] Contact information updated monthly
- [ ] Backup procedures tested weekly
- [ ] Recovery time objectives validated annually

## Recovery Testing

### Regular Testing Schedule
- **Monthly**: Backup verification and restore testing
- **Quarterly**: Full disaster recovery simulation
- **Annually**: Complete failover and failback testing

### Testing Scenarios
1. **Backup Verification**: Restore from backup to test environment
2. **Partial Recovery**: Test table/schema level recovery
3. **Failover Testing**: Planned and unplanned failover procedures
4. **Performance Validation**: Ensure recovered system meets performance requirements

## Success Criteria

### Technical Success
- [ ] Systems restored within RTO
- [ ] Data recovered within RPO
- [ ] Application functionality verified
- [ ] Performance requirements met
- [ ] Monitoring systems operational

### Business Success
- [ ] User impact minimized
- [ ] Stakeholder communication effective
- [ ] Business operations resumed
- [ ] Lessons learned documented
- [ ] Process improvements identified

## Emergency Contacts

**During Incident:**
- DR Coordinator: [primary contact]
- Database Administrator: [DBA contact]
- Infrastructure Team: [infra contact]
- Application Support: [app team contact]

**After Hours:**
- On-call Engineer: [24/7 contact]
- Executive Escalation: [business impact decisions]

## Risk Assessment

### High-Risk Scenarios
- **Data Corruption**: May require complex forensic analysis
- **Ransomware**: Security and legal implications
- **Multi-Site Failure**: Requires offsite backups
- **Extended Downtime**: Business continuity impact

### Mitigation Strategies
- **Regular Testing**: Validate procedures work
- **Multiple Backup Methods**: Defense in depth
- **Offsite Storage**: Protect against site-wide disasters
- **Monitoring**: Early detection of issues
- **Documentation**: Clear, tested procedures

## Continuous Improvement

### Metrics to Track
- **MTTR (Mean Time To Recovery)**: Average recovery time
- **Backup Success Rate**: Percentage of successful backups
- **Recovery Test Results**: Pass/fail rates for recovery testing
- **RTO/RPO Compliance**: Percentage of recoveries within objectives

### Regular Reviews
- **Monthly**: Backup and recovery metrics
- **Quarterly**: DR procedure updates and testing
- **Annually**: Complete DR plan review and validation

Remember: Disaster recovery is not just about technology - it's about minimizing business impact and maintaining customer trust. Regular testing and preparation are essential for success.
