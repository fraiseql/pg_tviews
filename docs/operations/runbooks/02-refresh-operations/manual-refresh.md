# Manual Refresh Runbook

## Purpose
Perform manual refresh operations on individual TVIEWs or groups of TVIEWs when automated refresh is insufficient or when immediate synchronization is required.

## When to Use
- **Data Inconsistencies**: When TVIEW data appears out of sync with source tables
- **Emergency Updates**: After critical data changes that must be reflected immediately
- **Testing Changes**: Validate TVIEW behavior after schema or logic modifications
- **Recovery Operations**: Restore synchronization after system failures
- **One-off Updates**: Handle special cases not covered by automated refresh

## Prerequisites
- PostgreSQL CLI access (`psql`)
- Database credentials with TVIEW refresh permissions
- Knowledge of TVIEW names and primary key values
- Understanding of data dependencies between TVIEWs

## Single TVIEW Refresh (5 minutes)

### Step 1: Identify the TVIEW
```sql
-- Find the correct TVIEW name
SELECT entity_name, primary_key_column, created_at
FROM pg_tviews_metadata
WHERE entity_name LIKE '%your_table_name%'
ORDER BY created_at DESC;
```

### Step 2: Check Current Status
```sql
-- Verify current refresh status
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    last_error
FROM pg_tviews_metadata
WHERE entity_name = 'your_tview_name';
```

### Step 3: Perform Refresh
```sql
-- Refresh the entire TVIEW
SELECT pg_tviews_refresh('your_tview_name');

-- Expected result: Number of rows refreshed
-- Example: pg_tviews_refresh
--          ------------------
--                      1,247
```

### Step 4: Verify Success
```sql
-- Confirm refresh completed successfully
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    CASE
        WHEN last_refresh_duration_ms < 5000 THEN 'SUCCESS'
        WHEN last_refresh_duration_ms < 30000 THEN 'SLOW_BUT_SUCCESS'
        ELSE 'POTENTIAL_ISSUE'
    END as status
FROM pg_tviews_metadata
WHERE entity_name = 'your_tview_name';
```

## Specific Row Refresh (3 minutes)

### Step 1: Identify Primary Key
```sql
-- Find the primary key column for the TVIEW
SELECT entity_name, primary_key_column
FROM pg_tviews_metadata
WHERE entity_name = 'your_tview_name';
```

### Step 2: Refresh Specific Row
```sql
-- Refresh only the specific row
-- Note: This function may not be available in all versions
-- Check your pg_tviews version first
SELECT pg_tviews_refresh_row('your_tview_name', 12345);

-- Alternative: Use general refresh with filters if available
SELECT pg_tviews_refresh('your_tview_name', 12345);
```

### Step 3: Verify Row Update
```sql
-- Check that the specific row was updated
SELECT last_refreshed
FROM pg_tviews_metadata
WHERE entity_name = 'your_tview_name'
  AND last_refreshed > NOW() - INTERVAL '1 minute';
```

## Batch Refresh Operations (10 minutes)

### Step 1: Assess Scope
```sql
-- Check how many TVIEWs need refreshing
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refreshed < NOW() - INTERVAL '1 hour') as stale_tviews,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as error_tviews
FROM pg_tviews_metadata;
```

### Step 2: Selective Batch Refresh
```sql
-- Refresh only TVIEWs that haven't been updated recently
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE last_refreshed < NOW() - INTERVAL '1 hour'
  AND last_error IS NULL
ORDER BY last_refreshed ASC;
```

### Step 3: Error Recovery Batch
```sql
-- Retry TVIEWs that had errors
SELECT
    entity_name,
    last_error,
    pg_tviews_refresh(entity_name) as refresh_result
FROM pg_tviews_metadata
WHERE last_error IS NOT NULL
  AND last_refreshed < NOW() - INTERVAL '30 minutes';
```

## Dependency-Aware Refresh (15 minutes)

### Step 1: Analyze Dependencies
```sql
-- Check for TVIEW dependencies (if your system tracks them)
SELECT
    parent_tview,
    child_tview,
    dependency_type
FROM pg_tviews_dependencies
ORDER BY parent_tview, child_tview;
```

### Step 2: Refresh in Dependency Order
```sql
-- Refresh base TVIEWs first, then dependent ones
-- This is a conceptual example - adjust based on your dependency tracking

-- Level 1: Base TVIEWs (no dependencies)
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE entity_name NOT IN (
    SELECT DISTINCT child_tview FROM pg_tviews_dependencies
);

-- Level 2: TVIEWs that depend on level 1
-- Add appropriate delays between levels to avoid conflicts
SELECT pg_sleep(5);  -- Wait 5 seconds

SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE entity_name IN (
    SELECT DISTINCT child_tview FROM pg_tviews_dependencies
);
```

## Large TVIEW Refresh Strategy (30+ minutes)

### Step 1: Assess Size and Impact
```sql
-- Check TVIEW size and refresh history
SELECT
    entity_name,
    pg_size_pretty(pg_total_relation_size(entity_name)) as size,
    last_refresh_duration_ms / 1000 as last_duration_seconds,
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_schema || '.' || table_name = m.entity_name) as column_count
FROM pg_tviews_metadata m
WHERE entity_name = 'your_large_tview';
```

### Step 2: Chunked Refresh (if supported)
```sql
-- For very large TVIEWs, refresh in chunks
-- This is system-dependent - check your pg_tviews capabilities

-- Example: Refresh in primary key ranges
SELECT pg_tviews_refresh_range('your_tview', 1, 10000);
SELECT pg_sleep(10);  -- Allow system to recover
SELECT pg_tviews_refresh_range('your_tview', 10001, 20000);
```

### Step 3: Monitor Progress
```sql
-- Monitor refresh progress for large operations
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    pg_size_pretty(pg_total_relation_size(entity_name)) as current_size
FROM pg_tviews_metadata
WHERE entity_name = 'your_large_tview';
```

## Refresh Validation

### Step 1: Data Consistency Check
```sql
-- Verify TVIEW data matches source (sample check)
SELECT
    COUNT(*) as tview_rows,
    (SELECT COUNT(*) FROM your_source_table) as source_rows
FROM your_tview_name;

-- For more detailed validation, compare key metrics
SELECT
    SUM(amount) as tview_total,
    (SELECT SUM(amount) FROM your_source_table) as source_total
FROM your_tview_name;
```

### Step 2: Performance Impact Assessment
```sql
-- Check system impact of refresh
SELECT
    now() as check_time,
    (SELECT COUNT(*) FROM pg_stat_activity WHERE state = 'active') as active_connections,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as total_blocks_accessed
FROM pg_stat_bgwriter;
```

## Error Handling

### Common Refresh Errors

#### "TVIEW not found"
```sql
-- Error: TVIEW 'nonexistent_tview' does not exist
-- Solution: Check TVIEW name spelling and existence
SELECT entity_name FROM pg_tviews_metadata WHERE entity_name LIKE '%nonexistent%';
```

#### "Permission denied"
```sql
-- Error: permission denied for table your_tview
-- Solution: Verify user has appropriate permissions
GRANT SELECT, UPDATE ON your_tview TO your_user;
```

#### "Lock timeout"
```sql
-- Error: canceling statement due to lock timeout
-- Solution: Identify and resolve blocking transactions
SELECT * FROM pg_stat_activity WHERE state = 'idle in transaction';
```

#### "Out of memory"
```sql
-- Error: out of memory during refresh
-- Solution: Consider chunked refresh or system memory increase
-- Check current memory settings
SELECT name, setting FROM pg_settings WHERE name LIKE '%mem%';
```

## Rollback Procedures

### Immediate Rollback (if refresh caused issues)
```sql
-- If refresh introduced bad data, you may need to:
-- 1. Stop accepting new queries to the TVIEW
-- 2. Restore from backup if critical
-- 3. Or wait for next automated refresh to correct

-- Example: Temporarily disable TVIEW access (if supported)
-- ALTER TABLE your_tview SET UNLOGGED;  -- Caution: data loss risk
```

### Partial Rollback
```sql
-- For specific row issues, you might need manual data correction
-- This depends entirely on your data and business logic

-- Example: Restore specific records from backup table
INSERT INTO your_tview (id, data)
SELECT id, data FROM your_tview_backup
WHERE id IN (123, 456, 789)
ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data;
```

## Monitoring and Alerts

### Success Criteria
- ✅ Refresh completes without errors
- ✅ `last_refreshed` timestamp updates
- ✅ `last_error` remains NULL
- ✅ Performance within acceptable limits (< 30 seconds for normal TVIEWs)

### Warning Signs
- ⚠️ Refresh takes significantly longer than usual
- ⚠️ Multiple retries required
- ⚠️ System performance degrades during refresh
- ⚠️ Queue backlog increases

### Automated Monitoring
```bash
# Check refresh status after manual operations
psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "
SELECT entity_name, last_refreshed, last_error
FROM pg_tviews_metadata
WHERE entity_name = 'your_tview_name';"
```

## Related Runbooks

- [Batch Refresh](batch-refresh.md) - Multiple TVIEW refresh operations
- [Refresh Troubleshooting](refresh-troubleshooting.md) - Debug refresh issues
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Monitor refresh performance
- [Emergency Procedures](../04-incident-response/emergency-procedures.md) - Crisis refresh operations

## Best Practices

1. **Test First**: Always test refresh operations in staging environment
2. **Monitor Impact**: Watch system performance during large refreshes
3. **Schedule Wisely**: Avoid peak hours for large refresh operations
4. **Validate Results**: Always verify refresh success and data consistency
5. **Document Changes**: Record any manual refresh operations for audit trails
6. **Plan Dependencies**: Consider TVIEW dependency order for multi-TVIEW refreshes</content>
<parameter name="filePath">docs/operations/runbooks/02-refresh-operations/manual-refresh.md