# pg_tviews Extension Minor Update

## Scope
Upgrading pg_tviews extension within the same major version (e.g., 0.1.0 → 0.1.1, 0.2.3 → 0.2.4)

## Risk Level
**LOW** - Minor updates are backward compatible and usually safe

## Prerequisites
- pg_tviews extension currently installed and functional
- Database backup completed (recommended for safety)
- Maintenance window scheduled (5-15 minutes)
- Application can tolerate brief TVIEW unavailability
- New extension version downloaded and available

## Impact Assessment

### Downtime
- **Planned**: 1-5 minutes for extension update
- **Unplanned**: 10-15 minutes if rollback needed

### TVIEW Impact
- **Data**: No changes required
- **Functionality**: Fully backward compatible
- **Performance**: May include performance improvements
- **API**: No breaking changes

### Compatibility
- **Applications**: No code changes required
- **Existing TVIEWs**: All continue to work
- **Configuration**: No changes needed
- **Dependencies**: No additional requirements

## Pre-Update Checklist

### Environment Verification
- [ ] Current pg_tviews version confirmed (`SELECT pg_tviews_version();`)
- [ ] All TVIEWs functioning normally (no errors in metadata)
- [ ] Recent backup available and tested
- [ ] New extension version downloaded and verified
- [ ] Application teams notified of brief maintenance

### Health Check
```sql
-- Verify system is ready for update
SELECT
    'Pre-update health check' as check_type,
    (SELECT COUNT(*) FROM pg_tviews_metadata) as total_tviews,
    (SELECT COUNT(*) FROM pg_tviews_metadata WHERE last_error IS NOT NULL) as tviews_with_errors,
    (SELECT COUNT(*) FROM pg_tviews_queue WHERE processed_at IS NULL) as pending_refreshes,
    (SELECT pg_tviews_version()) as current_version
FROM pg_stat_bgwriter;
```

## Step-by-Step Update Procedure

### Phase 1: Preparation (5 minutes)

#### Step 1: Download and Verify New Version
```bash
# Download the new extension version
# Adjust URL and version as needed
wget https://github.com/your-org/pg_tviews/releases/download/v0.1.1/pg_tviews-0.1.1.tar.gz
tar -xzf pg_tviews-0.1.1.tar.gz

# Verify download integrity
sha256sum pg_tviews-0.1.1.tar.gz

# Build the extension
cd pg_tviews-0.1.1
make clean && make

# Verify build success
ls -la pg_tviews.so
```

#### Step 2: Backup Current State
```sql
-- Document current state for rollback reference
CREATE TABLE pre_update_backup AS
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    last_error,
    pg_tviews_version() as extension_version,
    NOW() as backup_timestamp
FROM pg_tviews_metadata;
```

### Phase 2: Update Execution (2 minutes)

#### Step 3: Update Extension
```sql
-- Update the extension (this is usually safe for minor versions)
ALTER EXTENSION pg_tviews UPDATE TO '0.1.1';

-- Verify update success
SELECT pg_tviews_version();
```

#### Step 4: Verify Extension Loading
```sql
-- Check that extension is properly loaded
SELECT * FROM pg_extension WHERE extname = 'pg_tviews';

-- Verify all functions are available
SELECT
    proname,
    pg_get_function_identity_arguments(oid) as arguments
FROM pg_proc
WHERE proname LIKE 'pg_tviews%'
ORDER BY proname;
```

### Phase 3: Post-Update Validation (3 minutes)

#### Step 5: Test TVIEW Functionality
```sql
-- Basic functionality test
SELECT pg_tviews_health_check();

-- Test a simple refresh operation
SELECT pg_tviews_refresh('test_tview_name');
```

#### Step 6: Verify Data Integrity
```sql
-- Ensure all TVIEWs are still present and functional
SELECT
    'Post-update validation' as check_type,
    COUNT(*) as tviews_present,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as tviews_with_errors,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as recently_refreshed
FROM pg_tviews_metadata;
```

#### Step 7: Performance Check
```sql
-- Quick performance validation
SELECT
    entity_name,
    last_refresh_duration_ms,
    CASE
        WHEN last_refresh_duration_ms < 1000 THEN 'FAST'
        WHEN last_refresh_duration_ms < 5000 THEN 'NORMAL'
        WHEN last_refresh_duration_ms < 30000 THEN 'SLOW'
        ELSE 'VERY_SLOW'
    END as performance_status
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '1 hour'
ORDER BY last_refresh_duration_ms DESC
LIMIT 5;
```

## Success Criteria

### Technical Success
- [ ] Extension updated to new version
- [ ] All TVIEWs remain functional
- [ ] No new errors introduced
- [ ] Performance maintained or improved
- [ ] Extension functions accessible

### Application Success
- [ ] Application continues to work normally
- [ ] TVIEW queries return expected results
- [ ] No application errors related to TVIEWs
- [ ] Response times acceptable

## Rollback Procedure

### Immediate Rollback (< 5 minutes)
If issues discovered immediately after update:

```sql
-- Downgrade extension to previous version
ALTER EXTENSION pg_tviews UPDATE TO '0.1.0';

-- Verify rollback success
SELECT pg_tviews_version();
```

### Complete Rollback (< 15 minutes)
If major issues require full rollback:

```sql
-- Stop application services
-- (Application-specific commands)

-- Restore from backup if needed
-- (Use your backup restoration procedure)

-- Reinstall previous extension version
ALTER EXTENSION pg_tviews UPDATE TO '0.1.0';

-- Restart application services
-- (Application-specific commands)
```

## Troubleshooting

### Extension Won't Update
```sql
-- Check for blocking operations
SELECT * FROM pg_stat_activity WHERE query LIKE '%tview%';

-- Check extension dependencies
SELECT * FROM pg_depend WHERE objid = (SELECT oid FROM pg_extension WHERE extname = 'pg_tviews');

-- Force update if needed (use with caution)
DROP EXTENSION pg_tviews;
CREATE EXTENSION pg_tviews VERSION '0.1.1';
```

### Functions Not Available
```sql
-- Check if extension is properly installed
SELECT * FROM pg_extension WHERE extname = 'pg_tviews';

-- Reload PostgreSQL configuration
SELECT pg_reload_conf();

-- Check function existence
SELECT proname FROM pg_proc WHERE proname LIKE 'pg_tviews%';
```

### Performance Issues
```sql
-- Compare before/after performance
SELECT
    entity_name,
    last_refresh_duration_ms as current_duration,
    (SELECT last_refresh_duration_ms FROM pre_update_backup pub WHERE pub.entity_name = m.entity_name) as previous_duration
FROM pg_tviews_metadata m
WHERE last_refreshed > NOW() - INTERVAL '1 hour';
```

## Automated Update Process

### For Regular Maintenance
```bash
# Example automated update script
#!/bin/bash

# Pre-update checks
psql -c "SELECT pg_tviews_health_check();" > /tmp/pre_update_health.txt

# Update extension
psql -c "ALTER EXTENSION pg_tviews UPDATE TO '$NEW_VERSION';"

# Post-update validation
psql -c "SELECT pg_tviews_health_check();" > /tmp/post_update_health.txt

# Compare results
diff /tmp/pre_update_health.txt /tmp/post_update_health.txt || echo "Differences detected - manual review required"
```

### Monitoring Integration
```sql
-- Create monitoring for extension updates
CREATE OR REPLACE FUNCTION monitor_extension_updates()
RETURNS TABLE (
    check_time TIMESTAMP,
    extension_name TEXT,
    current_version TEXT,
    expected_version TEXT,
    status TEXT
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        NOW() as check_time,
        'pg_tviews'::TEXT as extension_name,
        pg_tviews_version() as current_version,
        '0.1.1'::TEXT as expected_version,
        CASE
            WHEN pg_tviews_version() = '0.1.1' THEN 'UP_TO_DATE'
            WHEN pg_tviews_version() LIKE '0.1.%' THEN 'UPDATE_AVAILABLE'
            ELSE 'VERSION_MISMATCH'
        END as status;
END;
$$ LANGUAGE plpgsql;
```

## Version Compatibility

### Supported Upgrade Paths
- ✅ 0.1.0 → 0.1.1 (patch update)
- ✅ 0.1.1 → 0.1.2 (patch update)
- ✅ 0.2.0 → 0.2.1 (patch update within minor)
- ⚠️ 0.1.x → 0.2.x (minor version - see major update guide)
- ❌ 0.x → 1.x (major version - requires migration)

### Feature Additions in Minor Updates
Minor updates may include:
- Performance improvements
- Bug fixes
- New optional parameters
- Enhanced error messages
- Monitoring improvements

## Documentation Updates

### Post-Update Tasks
- [ ] Update internal documentation with new version
- [ ] Notify application teams of update completion
- [ ] Update monitoring dashboards if needed
- [ ] Document any new features or improvements
- [ ] Schedule next regular update

### Change Log Review
```sql
-- Review what changed in the update
-- Check the extension changelog or release notes for:
-- - Bug fixes included
-- - Performance improvements
-- - New features (if any)
-- - Known issues or limitations
```

## Related Guides

- [Major Extension Update](0.1-to-0.2-migration.md) - For minor version upgrades
- [PostgreSQL Minor Upgrade](../postgresql/minor-version-upgrade.md) - For database upgrades
- [Troubleshooting Upgrades](../postgresql/troubleshooting-upgrades.md) - For update issue resolution
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md) - For update failures</content>
<parameter name="filePath">docs/operations/upgrade/extension/extension-minor-update.md