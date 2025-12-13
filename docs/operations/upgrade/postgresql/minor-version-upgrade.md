# PostgreSQL Minor Version Upgrade

## Scope
Upgrading within the same major version (e.g., PostgreSQL 15.1 → 15.5, 16.2 → 16.4)

## Risk Level
**LOW** - Usually safe with proper testing and backups

## Prerequisites
- PostgreSQL 15.x or 16.x currently running
- At least 20GB free disk space for upgrades
- Full database backup completed and verified
- Maintenance window scheduled (30-60 minutes total)
- Application read-only mode capability (recommended)
- pg_tviews extension installed and functioning

## Impact Assessment

### Downtime
- **Planned**: 5-15 minutes for PostgreSQL restart
- **Unplanned**: 30-60 minutes if issues occur

### TVIEW Impact
- **Data**: No changes required
- **Functionality**: Fully compatible
- **Performance**: May improve with bug fixes
- **Extension**: May need reinstall if binary incompatible

### Compatibility
- **Applications**: Fully compatible
- **Extensions**: Usually compatible, may need reinstall
- **System Catalogs**: No changes

## Pre-Upgrade Checklist

### Environment Verification
- [ ] PostgreSQL version confirmed (`SELECT version();`)
- [ ] pg_tviews version noted (`SELECT pg_tviews_version();`)
- [ ] Database size checked (`SELECT pg_size_pretty(pg_database_size(current_database()));`)
- [ ] Free disk space verified (2x database size minimum)
- [ ] Backup completed and integrity verified

### TVIEW Health Check
- [ ] All TVIEWs functioning normally (no errors in metadata)
- [ ] Queue processing working (no stuck items)
- [ ] Recent refreshes completed successfully
- [ ] Performance within normal ranges

### Application Readiness
- [ ] Application teams notified of maintenance window
- [ ] Read-only mode tested and available
- [ ] Connection pooling configured for graceful shutdown
- [ ] Monitoring alerts configured for upgrade window

## Step-by-Step Upgrade Procedure

### Phase 1: Pre-Upgrade Preparation (30 minutes)

#### Step 1: Create Backup
```bash
# Create full database backup
BACKUP_DIR="/backups/$(date +%Y%m%d_%H%M%S)"
mkdir -p $BACKUP_DIR

# Logical backup (recommended for safety)
sudo -u postgres pg_dump --compress=9 --format=directory \
    --jobs=4 --verbose --file=$BACKUP_DIR \
    --exclude-schema=pg_toast $DB_NAME

# Verify backup integrity
sudo -u postgres pg_restore --list $BACKUP_DIR | head -10
echo "✅ Backup created successfully in $BACKUP_DIR"
```

#### Step 2: Run Pre-Upgrade Health Checks
```bash
# Execute comprehensive health check
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f docs/operations/runbooks/scripts/health-check.sql > $BACKUP_DIR/pre-upgrade-health.txt

# Check for any critical issues
grep -i "CRITICAL\|error\|failed" $BACKUP_DIR/pre-upgrade-health.txt || echo "✅ No critical issues found"

# Document current state
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -tAc "SELECT version();" > $BACKUP_DIR/postgres-version-before.txt
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -tAc "SELECT pg_tviews_version();" > $BACKUP_DIR/extension-version-before.txt
```

#### Step 3: Prepare Applications
```bash
# Notify application teams
echo "PostgreSQL minor upgrade starting in 30 minutes. Brief downtime expected."

# Enable read-only mode if available
# curl -X POST http://app-server:8080/admin/maintenance-mode \
#   -H "Content-Type: application/json" \
#   -d '{"mode": "readonly", "reason": "postgresql_upgrade"}'

# Verify read-only mode working
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "INSERT INTO test_table VALUES (1);" 2>&1 | grep -q "read-only" && echo "✅ Read-only mode active" || echo "⚠️ Read-only mode not confirmed"
```

### Phase 2: Upgrade Execution (10 minutes)

#### Step 4: Stop Application Services
```bash
# Graceful application shutdown
echo "Stopping application services..."

# Stop web services
sudo systemctl stop nginx
sudo systemctl stop application-service

# Verify connections draining
watch -n 5 "psql -h $DB_HOST -U $DB_USER -d $DB_NAME -tAc \"SELECT count(*) FROM pg_stat_activity WHERE state != 'idle';\""

# Wait for active connections to drop below threshold
ACTIVE_CONNS=$(psql -h $DB_HOST -U $DB_USER -d $DB_NAME -tAc "SELECT count(*) FROM pg_stat_activity WHERE state != 'idle';")
while [ "$ACTIVE_CONNS" -gt 5 ]; do
    echo "Waiting for $ACTIVE_CONNS connections to drain..."
    sleep 10
    ACTIVE_CONNS=$(psql -h $DB_HOST -U $DB_USER -d $DB_NAME -tAc "SELECT count(*) FROM pg_stat_activity WHERE state != 'idle';")
done
```

#### Step 5: Upgrade PostgreSQL
```bash
# Check current version
sudo -u postgres psql -c "SELECT version();"

# Stop PostgreSQL service
sudo systemctl stop postgresql

# Install new PostgreSQL version
# Note: Adjust package manager and version for your system
sudo apt update
sudo apt install --only-upgrade postgresql-15 postgresql-client-15

# Verify new version installed
/usr/lib/postgresql/15/bin/postgres --version

# Start PostgreSQL with new version
sudo systemctl start postgresql

# Verify startup
sudo systemctl status postgresql
```

#### Step 6: Verify PostgreSQL Upgrade
```bash
# Check PostgreSQL is running and accessible
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT version();"

# Verify database is accessible
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT current_database(), current_user;"

# Check for any startup errors
sudo journalctl -u postgresql -n 50 --no-pager | grep -i error || echo "✅ No startup errors"
```

#### Step 7: Reinstall pg_tviews Extension (if needed)
```bash
# Check if extension needs reinstall
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT * FROM pg_extension WHERE extname = 'pg_tviews';"

# If extension is missing or incompatible, reinstall
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "DROP EXTENSION IF EXISTS pg_tviews;"
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "CREATE EXTENSION pg_tviews;"

# Verify extension version
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT pg_tviews_version();"
```

### Phase 3: Post-Upgrade Validation (15 minutes)

#### Step 8: Run Post-Upgrade Checks
```bash
# Execute post-upgrade validation
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -f docs/operations/upgrade/scripts/post-upgrade-validation.sql > $BACKUP_DIR/post-upgrade-validation.txt

# Check for any issues
grep -i "FAILED\|ERROR\|CRITICAL" $BACKUP_DIR/post-upgrade-validation.txt || echo "✅ Post-upgrade checks passed"
```

#### Step 9: Test TVIEW Functionality
```sql
-- Quick TVIEW functionality test
SELECT
    COUNT(*) as tview_count,
    COUNT(*) FILTER (WHERE last_error IS NULL) as healthy_tviews,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as recently_refreshed
FROM pg_tviews_metadata;

-- Test a simple refresh
SELECT pg_tviews_refresh('test_tview_name') as refresh_result;
```

#### Step 10: Restart Application Services
```bash
# Disable read-only mode
# curl -X DELETE http://app-server:8080/admin/maintenance-mode

# Start application services
sudo systemctl start application-service
sudo systemctl start nginx

# Verify application health
curl -f http://localhost/health || echo "⚠️ Application health check failed"
```

## Success Criteria

### Technical Success
- [ ] PostgreSQL started successfully with new version
- [ ] All databases accessible
- [ ] pg_tviews extension loaded and functional
- [ ] TVIEW metadata intact
- [ ] Basic queries working
- [ ] No critical errors in logs

### Application Success
- [ ] Application services started successfully
- [ ] Basic functionality verified
- [ ] User-facing features working
- [ ] Performance acceptable
- [ ] Error rates normal

### TVIEW Success
- [ ] All TVIEWs present in metadata
- [ ] No new errors introduced
- [ ] Refresh operations working
- [ ] Performance maintained or improved

## Rollback Procedure

### Immediate Rollback (< 15 minutes)
If critical issues discovered immediately after upgrade:

```bash
# Stop application services
sudo systemctl stop application-service nginx

# Stop PostgreSQL
sudo systemctl stop postgresql

# Downgrade PostgreSQL packages
sudo apt install --reinstall postgresql-15=15.x.x postgresql-client-15=15.x.x

# Restore from backup
sudo -u postgres dropdb $DB_NAME
sudo -u postgres createdb $DB_NAME
sudo -u postgres pg_restore --jobs=4 --verbose --dbname=$DB_NAME $BACKUP_DIR

# Start services
sudo systemctl start postgresql
sudo systemctl start application-service nginx
```

### Verification After Rollback
```sql
-- Verify rollback success
SELECT version();
SELECT pg_tviews_version();
SELECT COUNT(*) FROM pg_tviews_metadata;
```

## Monitoring During Upgrade

### Key Metrics to Monitor
- PostgreSQL startup time and errors
- Application startup time and errors
- TVIEW refresh performance
- Database connection counts
- System resource usage (CPU, memory, disk)

### Alert Thresholds
- PostgreSQL startup > 5 minutes
- Application startup > 10 minutes
- TVIEW errors > 0
- Connection failures > 5%

## Troubleshooting

### PostgreSQL Won't Start
```bash
# Check PostgreSQL logs
sudo journalctl -u postgresql -n 100 --no-pager

# Common issues:
# - Configuration file syntax errors
# - Missing libraries for new version
# - Permission issues on data directory

# Fix configuration issues
sudo -u postgres vi /etc/postgresql/15/main/postgresql.conf
sudo systemctl restart postgresql
```

### Extension Won't Load
```bash
# Check extension installation
ls -la /usr/lib/postgresql/15/lib/ | grep tviews

# Reinstall extension
sudo make install  # From pg_tviews source directory
psql -c "CREATE EXTENSION pg_tviews;"
```

### Application Won't Connect
```bash
# Check connection settings
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT 1;"

# Common issues:
# - Connection string not updated
# - Authentication method changed
# - SSL configuration issues
```

## Performance Expectations

### Expected Improvements
- Bug fixes in new version may improve performance
- Better query optimization
- Improved memory management
- Enhanced indexing capabilities

### Monitoring After Upgrade
```sql
-- Compare performance before/after
SELECT
    'Performance comparison' as metric,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as current_avg,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed BETWEEN NOW() - INTERVAL '25 hours' AND NOW() - INTERVAL '24 hours') as previous_avg
FROM pg_stat_bgwriter;
```

## Documentation Requirements

### Upgrade Record
- [ ] Date and time of upgrade
- [ ] Versions before and after
- [ ] Duration of downtime
- [ ] Issues encountered and resolutions
- [ ] Performance impact assessment
- [ ] Backup location and verification

### Communication Log
- [ ] Teams notified and when
- [ ] Status updates provided
- [ ] Issues communicated promptly
- [ ] Success confirmation sent

## Related Guides

- [Major Version Upgrade](pg15-to-pg16.md) - For major PostgreSQL upgrades
- [Extension Updates](extension-minor-update.md) - For pg_tviews extension updates
- [Troubleshooting Upgrades](troubleshooting-upgrades.md) - For upgrade issue resolution
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md) - For upgrade failures</content>
<parameter name="filePath">docs/operations/upgrade/postgresql/minor-version-upgrade.md