# Troubleshooting Upgrade Issues

## Overview
Common issues encountered during PostgreSQL and pg_tviews upgrades, with diagnosis and resolution steps.

## Issue Categories

### 1. PostgreSQL Upgrade Issues

#### pg_upgrade Fails with "Incompatible Data Types"
**Symptoms**: pg_upgrade fails with data type compatibility errors

**Diagnosis**:
```bash
# Check pg_upgrade logs
cat /var/lib/postgresql/16/main/pg_upgrade_internal.log | grep -i "error\|fail"

# Check for problematic data types
psql -d $OLD_DB -c "
SELECT
    schemaname,
    tablename,
    attname,
    atttypid::regtype as data_type
FROM pg_attribute a
JOIN pg_class c ON a.attrelid = c.oid
JOIN pg_namespace n ON c.relnamespace = n.oid
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
  AND atttypid NOT IN (
      SELECT oid FROM pg_type WHERE typname IN (
          'int2', 'int4', 'int8', 'float4', 'float8',
          'varchar', 'text', 'bool', 'date', 'timestamp',
          'timestamptz', 'bytea'
      )
  )
LIMIT 10;
"
```

**Solutions**:
1. **Custom Types**: Convert to standard PostgreSQL types before upgrade
2. **Extensions**: Ensure all extensions are compatible with target version
3. **User-Defined Types**: May need to be recreated in target version

#### Post-Upgrade: Database Won't Start
**Symptoms**: PostgreSQL fails to start after upgrade

**Diagnosis**:
```bash
# Check PostgreSQL logs
sudo journalctl -u postgresql -n 50 --no-pager

# Common log locations
tail -f /var/log/postgresql/postgresql-16-main.log
tail -f /var/lib/postgresql/16/main/log/postgresql.log
```

**Solutions**:
1. **Configuration Errors**: Check postgresql.conf syntax
   ```bash
   sudo -u postgres /usr/lib/postgresql/16/bin/postgres -C config_file
   ```

2. **Missing Libraries**: Install missing dependencies
   ```bash
   sudo apt install postgresql-16 postgresql-contrib-16
   ```

3. **Permission Issues**: Fix data directory permissions
   ```bash
   sudo chown -R postgres:postgres /var/lib/postgresql/16/main
   ```

#### Performance Degradation After Upgrade
**Symptoms**: Queries slower after PostgreSQL upgrade

**Diagnosis**:
```sql
-- Compare query performance
SELECT
    query,
    calls,
    total_time / calls as avg_time,
    rows
FROM pg_stat_statements
WHERE query LIKE '%tview%' OR query LIKE '%SELECT%'
ORDER BY total_time DESC
LIMIT 5;

-- Check if statistics are up to date
SELECT schemaname, tablename, last_analyze
FROM pg_stat_user_tables
WHERE last_analyze < NOW() - INTERVAL '1 day';
```

**Solutions**:
1. **Update Statistics**: Run ANALYZE on all tables
   ```sql
   ANALYZE;
   ```

2. **Reindex**: Rebuild indexes for better performance
   ```sql
   REINDEX DATABASE CONCURRENTLY current_database();
   ```

3. **Query Plan Changes**: Review and optimize queries if needed

### 2. pg_tviews Extension Issues

#### Extension Won't Install
**Symptoms**: `CREATE EXTENSION pg_tviews;` fails

**Diagnosis**:
```sql
-- Check extension files
ls -la /usr/share/postgresql/16/extension/pg_tviews*

-- Check PostgreSQL version compatibility
SELECT version();

-- Check for conflicting extensions
SELECT * FROM pg_extension WHERE extname LIKE '%tview%';
```

**Solutions**:
1. **Missing Files**: Reinstall pg_tviews
   ```bash
   cd /path/to/pg_tviews/source
   make clean && make && sudo make install
   ```

2. **Version Mismatch**: Ensure extension matches PostgreSQL version
3. **Dependencies**: Install required dependencies

#### TVIEWs Not Accessible After Upgrade
**Symptoms**: TVIEWs exist in metadata but queries fail

**Diagnosis**:
```sql
-- Check TVIEW existence
SELECT entity_name FROM pg_tviews_metadata;

-- Try to access a TVIEW
SELECT * FROM your_tview_name LIMIT 1;  -- This might fail

-- Check for schema issues
SELECT schemaname, tablename
FROM pg_tables
WHERE tablename LIKE '%tview%';
```

**Solutions**:
1. **Recreate TVIEWs**: Drop and recreate TVIEWs
   ```sql
   -- Drop existing TVIEW
   DROP VIEW your_tview_name;

   -- Recreate using pg_tviews_convert_existing_table
   SELECT pg_tviews_convert_existing_table('source_table_name');
   ```

2. **Schema Mismatch**: Ensure source tables exist and are accessible

#### Refresh Operations Fail
**Symptoms**: TVIEW refresh operations return errors

**Diagnosis**:
```sql
-- Check recent errors
SELECT entity_name, last_error, last_refreshed
FROM pg_tviews_metadata
WHERE last_error IS NOT NULL;

-- Test refresh operation
SELECT pg_tviews_refresh('problematic_tview');
```

**Solutions**:
1. **Permission Issues**: Grant necessary permissions
   ```sql
   GRANT SELECT, UPDATE ON source_table TO pg_tviews_user;
   ```

2. **Source Table Changes**: Verify source table structure hasn't changed
3. **Extension Version**: Ensure extension is properly updated

### 3. Logical Migration Issues

#### pg_dump Fails with Large Tables
**Symptoms**: pg_dump fails on large tables

**Diagnosis**:
```bash
# Check table sizes
psql -d $DB_NAME -c "
SELECT
    schemaname || '.' || tablename as table_name,
    pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename)) as size
FROM pg_tables
WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(schemaname || '.' || tablename) DESC
LIMIT 5;
"

# Check available disk space
df -h /tmp  # pg_dump uses /tmp by default
```

**Solutions**:
1. **Increase Temp Space**: Use custom temp directory
   ```bash
   pg_dump --temp-directory=/path/to/large/temp/space ...
   ```

2. **Parallel Dump**: Use parallel processing
   ```bash
   pg_dump --jobs=4 --compress=9 ...
   ```

3. **Table-by-Table**: Dump large tables separately

#### pg_restore Fails with Dependencies
**Symptoms**: pg_restore fails due to object dependencies

**Diagnosis**:
```bash
# Check restore errors
# pg_restore will show specific dependency errors

# Check for circular dependencies
psql -d $TARGET_DB -c "
SELECT conname, conrelid::regclass, confrelid::regclass
FROM pg_constraint
WHERE contype = 'f'
LIMIT 10;
"
```

**Solutions**:
1. **Dependency Order**: Restore in correct order
   ```bash
   # Restore schema first
   pg_restore --schema-only backup.dump | psql -d $TARGET_DB

   # Then data
   pg_restore --data-only --disable-triggers backup.dump | psql -d $TARGET_DB
   ```

2. **Disable Triggers**: Use --disable-triggers during data restore

#### Data Consistency Issues
**Symptoms**: Data differs between source and target

**Diagnosis**:
```sql
-- Compare row counts
SELECT
    'source' as db,
    schemaname,
    tablename,
    n_tup_ins as rows
FROM pg_stat_user_tables
WHERE schemaname = 'public'
  AND tablename NOT LIKE 'pg_%';

-- In target database, compare with source counts
```

**Solutions**:
1. **Sequence Reset**: Reset sequences to correct values
   ```sql
   SELECT 'SELECT setval(''' || schemaname || '.' || sequencename || ''', (SELECT max(' || attname || ') FROM ' || schemaname || '.' || tablename || '));'
   FROM pg_sequences s
   JOIN information_schema.columns c ON c.column_default LIKE '%' || s.sequencename || '%';
   ```

2. **Constraint Validation**: Re-enable and validate constraints
   ```sql
   ALTER TABLE table_name VALIDATE CONSTRAINT constraint_name;
   ```

### 4. Application Integration Issues

#### Connection String Changes
**Symptoms**: Applications can't connect after upgrade

**Diagnosis**:
```bash
# Test connection
psql -h $NEW_HOST -p $NEW_PORT -U $USER -d $DB_NAME -c "SELECT 1;"

# Check application logs for connection errors
grep -i "connection\|connect" /var/log/application/*.log
```

**Solutions**:
1. **Update Connection Strings**: Change host/port in application config
2. **SSL Configuration**: Update SSL settings if changed
3. **Authentication**: Verify authentication methods

#### Query Performance Issues
**Symptoms**: Application queries slower after upgrade

**Diagnosis**:
```sql
-- Check slow queries
SELECT query, mean_time, calls
FROM pg_stat_statements
ORDER BY mean_time DESC
LIMIT 10;

-- Compare with pre-upgrade baseline
```

**Solutions**:
1. **Statistics Update**: Run ANALYZE on affected tables
2. **Query Optimization**: Review and optimize slow queries
3. **Index Recreation**: Rebuild indexes if needed

### 5. Rollback Complications

#### Can't Rollback Due to Data Changes
**Symptoms**: Rollback fails because data was modified

**Diagnosis**:
```bash
# Check if target database has been modified
psql -d $TARGET_DB -c "
SELECT
    schemaname,
    tablename,
    n_tup_ins,
    n_tup_upd,
    n_tup_del
FROM pg_stat_user_tables
WHERE n_tup_upd > 0 OR n_tup_del > 0;
"
```

**Solutions**:
1. **Point-in-Time Recovery**: Use PITR if WAL archiving enabled
2. **Logical Restore**: Restore from backup and replay changes
3. **Manual Reconciliation**: Manually sync data differences

#### Extension Version Conflicts
**Symptoms**: Can't downgrade extension due to dependencies

**Diagnosis**:
```sql
-- Check extension dependencies
SELECT * FROM pg_depend WHERE objid = (
    SELECT oid FROM pg_extension WHERE extname = 'pg_tviews'
);
```

**Solutions**:
1. **Clean Removal**: Drop dependent objects first
2. **Force Downgrade**: Use CASCADE if safe
3. **Fresh Install**: Drop and recreate extension

## Emergency Procedures

### Complete System Reset
```bash
# Stop all services
sudo systemctl stop application postgresql

# Restore from backup
# (Use your backup restoration procedure)

# Reinstall extension
psql -d $DB_NAME -c "CREATE EXTENSION pg_tviews;"

# Restart services
sudo systemctl start postgresql application
```

### Data Recovery
```bash
# If data corruption suspected
# Use pg_dump from known good state
pg_dump -d $GOOD_DB > recovery.sql

# Restore to clean database
createdb clean_db
psql -d clean_db -f recovery.sql
```

## Prevention Measures

### Pre-Upgrade Preparation
- [ ] Test upgrade in staging environment
- [ ] Run pre-upgrade checks thoroughly
- [ ] Document all custom configurations
- [ ] Prepare detailed rollback procedures
- [ ] Schedule adequate maintenance window

### Monitoring Setup
- [ ] Enable detailed logging during upgrade
- [ ] Set up monitoring alerts for key metrics
- [ ] Prepare dashboards for upgrade monitoring
- [ ] Document normal vs. abnormal behavior

### Team Preparation
- [ ] Train team on upgrade procedures
- [ ] Assign clear roles and responsibilities
- [ ] Prepare communication templates
- [ ] Set up war room for complex upgrades

## Getting Help

### Internal Resources
- **Database Team**: PostgreSQL-specific issues
- **Application Team**: Integration problems
- **Infrastructure Team**: System-level issues

### External Resources
- **PostgreSQL Documentation**: https://www.postgresql.org/docs/
- **pg_tviews Issues**: GitHub repository issues
- **Community Forums**: PostgreSQL mailing lists

### Escalation Path
1. **Team Level**: Initial troubleshooting
2. **Senior Level**: Complex technical issues
3. **Executive Level**: Business impact decisions
4. **Vendor Level**: Product-specific issues

## Common Upgrade Mistakes

### ❌ What Not to Do
- **Skip Testing**: Never upgrade production without staging tests
- **No Rollback Plan**: Always have verified rollback procedures
- **Ignore Warnings**: Address all pre-upgrade check warnings
- **Rush the Process**: Take time for proper validation
- **Skip Documentation**: Document all steps and changes

### ✅ Best Practices
- **Test Thoroughly**: Multiple test cycles in staging
- **Have Fallbacks**: Multiple rollback options available
- **Monitor Closely**: Watch systems during and after upgrade
- **Communicate Clearly**: Keep all stakeholders informed
- **Learn Continuously**: Document lessons for future upgrades</content>
<parameter name="filePath">docs/operations/upgrade/postgresql/troubleshooting-upgrades.md