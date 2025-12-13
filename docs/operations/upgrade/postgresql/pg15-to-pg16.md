# PostgreSQL 15 to 16 Major Version Upgrade

## Scope
Upgrading from PostgreSQL 15.x to 16.x using both pg_upgrade (in-place) and logical migration methods.

## Risk Level
**HIGH** - Major version changes require extensive testing and careful planning

## Prerequisites
- PostgreSQL 15.x currently running and stable
- At least 2x database size in free disk space
- Full database backup completed and tested
- Maintenance window scheduled (2-4 hours)
- Staging environment with identical data for testing
- pg_tviews extension compatible with PostgreSQL 16
- Application compatibility verified with PostgreSQL 16

## Impact Assessment

### Downtime
- **pg_upgrade method**: 30-90 minutes
- **Logical migration**: 60-180 minutes (longer but safer)
- **Testing time**: 4-8 hours in staging

### TVIEW Impact
- **Data**: Schema compatible, no data migration needed
- **Functionality**: Extension must be reinstalled
- **Performance**: May improve with PostgreSQL 16 optimizations
- **Compatibility**: Full compatibility maintained

### Compatibility Considerations
- **Applications**: May need driver updates for PostgreSQL 16
- **Extensions**: All extensions need PostgreSQL 16 versions
- **System Catalogs**: Major changes in internal structure
- **Configuration**: Some parameters may have different defaults

## Pre-Upgrade Planning

### Environment Assessment
```bash
# Check current PostgreSQL version and configuration
psql -h $DB_HOST -U $DB_USER -c "SELECT version();"
psql -h $DB_HOST -U $DB_USER -c "SHOW ALL;" > $BACKUP_DIR/postgres-config-before.txt

# Check database size and objects
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "
SELECT
    current_database() as database,
    pg_size_pretty(pg_database_size(current_database())) as size,
    (SELECT count(*) FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema')) as user_tables,
    (SELECT count(*) FROM pg_stat_user_indexes) as indexes,
    (SELECT count(*) FROM pg_extension) as extensions
;"

# Check for deprecated features
psql -h $DB_HOST -U $DB_USER -c "
SELECT name, current_setting(name) as value
FROM pg_settings
WHERE name LIKE '%deprecated%' OR name LIKE '%obsolete%'
    AND current_setting(name) != '';
"
```

### TVIEW-Specific Assessment
```sql
-- Assess TVIEW upgrade impact
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refresh_duration_ms > 30000) as slow_tviews,
    pg_size_pretty(SUM(pg_total_relation_size(entity_name))) as total_tview_size,
    MAX(last_refreshed) as last_refresh_time
FROM pg_tviews_metadata;

-- Check for any TVIEW-specific configurations
SELECT name, setting
FROM pg_settings
WHERE name LIKE '%tview%' OR name LIKE '%refresh%';
```

### Compatibility Testing Plan
- [ ] Install PostgreSQL 16 in staging environment
- [ ] Restore production backup to staging
- [ ] Test all application functionality
- [ ] Run full TVIEW test suite
- [ ] Performance benchmarking
- [ ] Failover and recovery testing

## Method 1: pg_upgrade (In-Place Upgrade)

### Advantages
- Faster upgrade process
- Less disk space required
- Preserves all database objects exactly

### Disadvantages
- Higher risk if issues occur
- Requires PostgreSQL downtime during upgrade
- More complex rollback if needed

### Step-by-Step pg_upgrade Procedure

#### Phase 1: Preparation (60 minutes)
```bash
# Install PostgreSQL 16 alongside 15
sudo apt update
sudo apt install postgresql-16 postgresql-client-16

# Stop PostgreSQL 15
sudo systemctl stop postgresql

# Create new data directory for PostgreSQL 16
sudo mkdir -p /var/lib/postgresql/16/main
sudo chown postgres:postgres /var/lib/postgresql/16/main

# Initialize PostgreSQL 16 cluster
sudo -u postgres /usr/lib/postgresql/16/bin/initdb -D /var/lib/postgresql/16/main

# Copy configuration from PostgreSQL 15
sudo cp /etc/postgresql/15/main/*.conf /etc/postgresql/16/main/
sudo cp /etc/postgresql/15/main/pg_hba.conf /etc/postgresql/16/main/
sudo cp /etc/postgresql/15/main/pg_ident.conf /etc/postgresql/16/main/

# Adjust configuration for PostgreSQL 16
sudo -u postgres vi /etc/postgresql/16/main/postgresql.conf
# - Update data_directory
# - Update port if needed (default 5433 for 16)
# - Adjust other settings as needed
```

#### Phase 2: Upgrade Execution (30-60 minutes)
```bash
# Run pg_upgrade in check mode first
sudo -u postgres /usr/lib/postgresql/16/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/15/main \
    --new-datadir=/var/lib/postgresql/16/main \
    --old-bindir=/usr/lib/postgresql/15/bin \
    --new-bindir=/usr/lib/postgresql/16/bin \
    --old-port=5432 \
    --new-port=5433 \
    --check

# If check passes, run actual upgrade
sudo -u postgres /usr/lib/postgresql/16/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/15/main \
    --new-datadir=/var/lib/postgresql/16/main \
    --old-bindir=/usr/lib/postgresql/15/bin \
    --new-bindir=/usr/lib/postgresql/16/bin \
    --old-port=5432 \
    --new-port=5433

# Check upgrade log for errors
cat /var/lib/postgresql/16/main/pg_upgrade_internal.log
```

#### Phase 3: Service Migration (15 minutes)
```bash
# Stop both PostgreSQL instances
sudo systemctl stop postgresql

# Update configuration to use PostgreSQL 16
sudo ln -sf /etc/postgresql/16/main /etc/postgresql/main
sudo ln -sf /var/lib/postgresql/16/main /var/lib/postgresql/main

# Start PostgreSQL 16
sudo systemctl start postgresql

# Verify upgrade success
psql -c "SELECT version();"
psql -d $DB_NAME -c "SELECT pg_tviews_version();"
```

#### Phase 4: Extension Reinstallation
```sql
-- Drop and recreate pg_tviews extension
DROP EXTENSION IF EXISTS pg_tviews;
CREATE EXTENSION pg_tviews;

-- Verify TVIEWs are accessible
SELECT COUNT(*) FROM pg_tviews_metadata;
SELECT pg_tviews_health_check();
```

## Method 2: Logical Migration (pg_dump + pg_restore)

### Advantages
- Lower risk approach
- Can be tested thoroughly in advance
- Easier rollback if issues occur
- Can migrate to different architecture

### Disadvantages
- Longer downtime
- More disk space required
- Statistics need rebuilding
- Sequences may need adjustment

### Step-by-Step Logical Migration

#### Phase 1: Schema Migration (30 minutes)
```bash
# Create new PostgreSQL 16 instance
sudo systemctl stop postgresql
sudo apt install postgresql-16
sudo systemctl start postgresql

# Create target database
createdb -O $DB_OWNER $DB_NAME

# Dump schema only first
pg_dump -h $OLD_HOST -U $DB_USER --schema-only --no-owner --no-privileges $DB_NAME > schema.sql

# Restore schema to PostgreSQL 16
psql -h $NEW_HOST -U $DB_USER -d $DB_NAME -f schema.sql
```

#### Phase 2: Data Migration (60-120 minutes)
```bash
# Dump data with parallel processing
pg_dump -h $OLD_HOST -U $DB_USER \
    --data-only \
    --compress=9 \
    --format=directory \
    --jobs=4 \
    --no-owner \
    --exclude-schema=pg_toast \
    --file=data_dump \
    $DB_NAME

# Restore data to PostgreSQL 16
pg_restore -h $NEW_HOST -U $DB_USER \
    --jobs=4 \
    --verbose \
    --dbname=$DB_NAME \
    data_dump
```

#### Phase 3: Extension and Configuration
```sql
-- Install pg_tviews extension
CREATE EXTENSION pg_tviews;

-- Recreate TVIEWs (they won't be in the dump)
-- This requires running the original TVIEW creation scripts
-- Adjust paths and scripts as needed
psql -f /path/to/tview_creation_scripts.sql

-- Update sequences if needed
SELECT 'SELECT setval(''' || schemaname || '.' || sequencename || ''', (SELECT max(' || attname || ') FROM ' || schemaname || '.' || tablename || '));'
FROM pg_sequences s
JOIN information_schema.columns c ON c.column_default LIKE '%' || s.sequencename || '%'
WHERE c.table_schema = s.schemaname;
```

## Post-Upgrade Validation

### Comprehensive Testing
```bash
# Run post-upgrade validation script
psql -d $DB_NAME -f docs/operations/upgrade/scripts/post-upgrade-validation.sql

# Test TVIEW functionality
psql -d $DB_NAME -c "
SELECT
    COUNT(*) as tviews_present,
    COUNT(*) FILTER (WHERE last_error IS NULL) as tviews_healthy,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as recently_refreshed
FROM pg_tviews_metadata;
"

# Test application connectivity
# Replace with your application test commands
curl -f http://app-server/health
```

### Performance Validation
```sql
-- Compare performance metrics
SELECT
    'Performance validation' as check,
    (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata WHERE last_refreshed > NOW() - INTERVAL '1 hour') as current_avg_refresh,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as block_access
FROM pg_stat_bgwriter;
```

## Rollback Procedures

### pg_upgrade Rollback (< 30 minutes)
```bash
# Stop PostgreSQL 16
sudo systemctl stop postgresql

# Restore configuration to PostgreSQL 15
sudo ln -sf /etc/postgresql/15/main /etc/postgresql/main
sudo ln -sf /var/lib/postgresql/15/main /var/lib/postgresql/main

# Start PostgreSQL 15
sudo systemctl start postgresql

# Verify rollback
psql -c "SELECT version();"
```

### Logical Migration Rollback (< 60 minutes)
```bash
# Stop applications
sudo systemctl stop application-services

# Drop and recreate database from backup
dropdb $DB_NAME
createdb -O $DB_OWNER $DB_NAME
pg_restore -d $DB_NAME /path/to/pre-upgrade/backup

# Restart applications
sudo systemctl start application-services
```

## Troubleshooting

### pg_upgrade Issues
```bash
# Check upgrade logs
cat /var/lib/postgresql/16/main/pg_upgrade_internal.log
cat /var/lib/postgresql/16/main/pg_upgrade_server.log

# Common issues:
# - Incompatible extensions
# - Custom data types
# - Large objects issues

# Fix extension issues
psql -d $DB_NAME -c "DROP EXTENSION problematic_extension;"
# Then rerun pg_upgrade
```

### Logical Migration Issues
```bash
# Check for data consistency issues
psql -d $DB_NAME -c "
SELECT schemaname, tablename,
       pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC
LIMIT 5;
"

# Fix sequence issues
SELECT 'SELECT setval(''' || schemaname || '.' || sequencename || ''', (SELECT COALESCE(max(' || attname || '), 1) FROM ' || schemaname || '.' || tablename || '));'
FROM pg_sequences s
JOIN information_schema.columns c ON c.column_default LIKE '%' || s.sequencename || '%';
```

### TVIEW-Specific Issues
```sql
-- Recreate TVIEWs if needed
SELECT 'SELECT pg_tviews_convert_existing_table(''' || table_schema || '.' || table_name || ''');'
FROM information_schema.tables
WHERE table_type = 'BASE TABLE'
  AND table_schema NOT IN ('pg_catalog', 'information_schema')
  AND table_name LIKE '%tview%';
```

## Success Criteria

### Technical Success
- [ ] PostgreSQL 16 running and accessible
- [ ] All databases restored successfully
- [ ] pg_tviews extension installed and functional
- [ ] All TVIEWs present and operational
- [ ] Application connections working
- [ ] Performance within acceptable ranges

### Data Integrity Success
- [ ] Row counts match between versions
- [ ] Data consistency verified
- [ ] Foreign key constraints satisfied
- [ ] Sequences at correct values

### Application Success
- [ ] All application features working
- [ ] User acceptance testing passed
- [ ] Error rates within normal bounds
- [ ] Response times acceptable

## Performance Expectations

### PostgreSQL 16 Improvements
- Better query optimization
- Improved parallel processing
- Enhanced memory management
- Faster index operations
- Better vacuum performance

### TVIEW Performance Impact
- May see 10-30% performance improvement
- Reduced memory usage
- Faster refresh operations
- Better concurrency handling

## Documentation Requirements

### Upgrade Record
- [ ] Method used (pg_upgrade vs logical)
- [ ] Versions before and after
- [ ] Duration and issues encountered
- [ ] Performance impact assessment
- [ ] Rollback procedures tested

### Change Management
- [ ] Change request approved
- [ ] Risk assessment completed
- [ ] Communication plan executed
- [ ] Success criteria met

## Related Guides

- [Minor Version Upgrade](minor-version-upgrade.md) - For patch-level upgrades
- [Extension Major Update](../extension/0.1-to-0.2-migration.md) - For pg_tviews upgrades
- [Troubleshooting Upgrades](troubleshooting-upgrades.md) - For upgrade issue resolution
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md) - For upgrade failures</content>
<parameter name="filePath">docs/operations/upgrade/postgresql/pg15-to-pg16.md