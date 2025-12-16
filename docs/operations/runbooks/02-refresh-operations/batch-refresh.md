# Batch Refresh Runbook

## Purpose
Perform coordinated refresh operations across multiple TVIEWs with proper sequencing, error handling, and progress monitoring.

## When to Use
- **Scheduled Maintenance**: Regular batch refresh of all or groups of TVIEWs
- **Bulk Data Updates**: After large data imports or ETL operations
- **System Recovery**: Restore synchronization across multiple TVIEWs
- **Testing**: Validate refresh behavior across the entire TVIEW ecosystem
- **Performance Optimization**: Refresh during off-peak hours with monitoring

## Prerequisites
- PostgreSQL CLI access with batch operation permissions
- Knowledge of TVIEW dependencies and relationships
- Monitoring access to track progress and performance
- Maintenance window scheduling (for large operations)
- Backup verification (recommended before large batch operations)

## Batch Refresh Planning (15 minutes)

### Step 1: Assess Scope and Impact
```sql
-- Analyze the refresh scope
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refreshed < NOW() - INTERVAL '1 hour') as need_refresh,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as have_errors,
    SUM(pg_total_relation_size(entity_name)) as total_size_bytes
FROM pg_tviews_metadata;
```

### Step 2: Estimate Duration and Resources
```sql
-- Estimate based on historical performance
SELECT
    entity_name,
    pg_size_pretty(pg_total_relation_size(entity_name)) as size,
    last_refresh_duration_ms / 1000 as last_duration_sec,
    CASE
        WHEN last_refresh_duration_ms > 300000 THEN 'VERY_SLOW (>5min)'
        WHEN last_refresh_duration_ms > 60000 THEN 'SLOW (1-5min)'
        WHEN last_refresh_duration_ms > 10000 THEN 'MODERATE (10s-1min)'
        ELSE 'FAST (<10s)'
    END as estimated_speed
FROM pg_tviews_metadata
ORDER BY pg_total_relation_size(entity_name) DESC;
```

### Step 3: Check System Capacity
```sql
-- Verify system can handle batch load
SELECT
    (SELECT setting FROM pg_settings WHERE name = 'max_connections') as max_connections,
    (SELECT COUNT(*) FROM pg_stat_activity) as active_connections,
    (SELECT setting FROM pg_settings WHERE name = 'shared_buffers') as shared_buffers,
    (SELECT sum(blks_hit) + sum(blks_read) FROM pg_stat_database WHERE datname = current_database()) as recent_io
FROM pg_stat_bgwriter;
```

## Full System Batch Refresh (30-60 minutes)

### Step 1: Pre-Refresh Health Check
```sql
-- Ensure system is ready for batch operation
SELECT
    COUNT(*) as pending_queue_items,
    MAX(created_at) as newest_queue_item,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as tviews_with_errors
FROM pg_tviews_metadata m
LEFT JOIN pg_tviews_queue q ON true
WHERE q.processed_at IS NULL;

-- Expected: Low queue, no critical errors
```

### Step 2: Execute Batch Refresh
```sql
-- Refresh all TVIEWs (basic approach)
SELECT
    entity_name,
    pg_tviews_refresh(entity_name) as rows_refreshed
FROM pg_tviews_metadata
WHERE last_error IS NULL
ORDER BY pg_total_relation_size(entity_name) ASC;  -- Smallest first
```

### Step 3: Monitor Progress
```sql
-- Track refresh progress (run in separate session)
SELECT
    COUNT(*) as completed_refreshes,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as failed_refreshes,
    AVG(last_refresh_duration_ms) / 1000 as avg_duration_seconds,
    MAX(last_refresh_duration_ms) / 1000 as max_duration_seconds
FROM pg_tviews_metadata
WHERE last_refreshed > NOW() - INTERVAL '30 minutes';
```

### Step 4: Post-Refresh Validation
```sql
-- Verify all TVIEWs refreshed successfully
SELECT
    entity_name,
    last_refreshed,
    last_refresh_duration_ms,
    CASE
        WHEN last_error IS NOT NULL THEN 'FAILED'
        WHEN last_refreshed > NOW() - INTERVAL '30 minutes' THEN 'SUCCESS'
        ELSE 'NOT_REFRESHED'
    END as status
FROM pg_tviews_metadata
ORDER BY
    CASE
        WHEN last_error IS NOT NULL THEN 1
        WHEN last_refreshed < NOW() - INTERVAL '30 minutes' THEN 2
        ELSE 3
    END;
```

## Selective Batch Refresh (10-30 minutes)

### Step 1: Define Selection Criteria
```sql
-- Refresh TVIEWs by schema
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE entity_name LIKE 'public.%'
  AND last_error IS NULL;

-- Refresh TVIEWs by age (oldest first)
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE last_refreshed < NOW() - INTERVAL '24 hours'
ORDER BY last_refreshed ASC;
```

### Step 2: Priority-Based Refresh
```sql
-- Refresh critical TVIEWs first
WITH priority_tviews AS (
    SELECT entity_name,
           CASE
               WHEN entity_name LIKE '%orders%' THEN 1
               WHEN entity_name LIKE '%customers%' THEN 2
               WHEN entity_name LIKE '%products%' THEN 3
               ELSE 4
           END as priority
    FROM pg_tviews_metadata
    WHERE last_refreshed < NOW() - INTERVAL '1 hour'
)
SELECT pg_tviews_refresh(entity_name)
FROM priority_tviews
ORDER BY priority ASC, entity_name;
```

## Dependency-Aware Batch Refresh (20-45 minutes)

### Step 1: Analyze Dependencies
```sql
-- Identify TVIEW dependency chains
-- Note: This assumes you have dependency tracking
-- Adjust based on your actual dependency schema

WITH RECURSIVE dependency_chain AS (
    -- Base TVIEWs (no dependencies)
    SELECT
        entity_name,
        entity_name as root_tview,
        0 as level,
        ARRAY[entity_name] as path
    FROM pg_tviews_metadata
    WHERE entity_name NOT IN (
        SELECT DISTINCT dependent_tview
        FROM pg_tviews_dependencies  -- Adjust table name as needed
    )

    UNION ALL

    -- Dependent TVIEWs
    SELECT
        d.dependent_tview,
        dc.root_tview,
        dc.level + 1,
        dc.path || d.dependent_tview
    FROM pg_tviews_dependencies d  -- Adjust table name as needed
    JOIN dependency_chain dc ON d.source_tview = dc.entity_name
)
SELECT * FROM dependency_chain
ORDER BY root_tview, level;
```

### Step 2: Refresh by Dependency Level
```sql
-- Refresh level by level to respect dependencies
DO $$
DECLARE
    current_level INTEGER := 0;
    max_level INTEGER;
BEGIN
    -- Find maximum dependency level
    SELECT MAX(level) INTO max_level
    FROM dependency_chain;

    -- Refresh level by level
    WHILE current_level <= max_level LOOP
        RAISE NOTICE 'Refreshing dependency level %', current_level;

        -- Refresh all TVIEWs at this level
        PERFORM pg_tviews_refresh(entity_name)
        FROM dependency_chain
        WHERE level = current_level;

        -- Wait between levels to allow processing
        PERFORM pg_sleep(5);

        current_level := current_level + 1;
    END LOOP;
END $$;
```

## Large-Scale Batch Operations (1-4 hours)

### Step 1: Phased Approach
```sql
-- Phase 1: Fast TVIEWs (< 10 seconds)
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE last_refresh_duration_ms < 10000
   OR last_refresh_duration_ms IS NULL
ORDER BY pg_total_relation_size(entity_name) ASC;

-- Phase 2: Medium TVIEWs (10s - 5min)
SELECT pg_sleep(30);  -- Allow system recovery
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE last_refresh_duration_ms BETWEEN 10000 AND 300000
ORDER BY last_refresh_duration_ms ASC;

-- Phase 3: Slow TVIEWs (> 5min) - manual review
SELECT entity_name, last_refresh_duration_ms
FROM pg_tviews_metadata
WHERE last_refresh_duration_ms > 300000
ORDER BY last_refresh_duration_ms DESC;
```

### Step 2: Parallel Processing (Advanced)
```sql
-- If your system supports parallel refreshes
-- This is conceptual - adjust based on your capabilities

-- Create refresh jobs
CREATE TEMP TABLE refresh_jobs AS
SELECT
    entity_name,
    ROW_NUMBER() OVER (ORDER BY pg_total_relation_size(entity_name) DESC) % 4 as worker_id
FROM pg_tviews_metadata
WHERE last_refreshed < NOW() - INTERVAL '1 hour';

-- Process in parallel (would require external coordination)
-- Worker 1: SELECT pg_tviews_refresh(entity_name) FROM refresh_jobs WHERE worker_id = 1;
-- Worker 2: SELECT pg_tviews_refresh(entity_name) FROM refresh_jobs WHERE worker_id = 2;
-- etc.
```

## Error Handling and Recovery

### Handle Partial Failures
```sql
-- Identify and retry failed refreshes
SELECT
    entity_name,
    last_error,
    pg_tviews_refresh(entity_name) as retry_result
FROM pg_tviews_metadata
WHERE last_error IS NOT NULL
  AND last_refreshed < NOW() - INTERVAL '1 hour';
```

### Circuit Breaker Pattern
```sql
-- Stop batch if too many failures
DO $$
DECLARE
    failure_count INTEGER;
BEGIN
    SELECT COUNT(*) INTO failure_count
    FROM pg_tviews_metadata
    WHERE last_error IS NOT NULL;

    IF failure_count > 5 THEN
        RAISE EXCEPTION 'Too many refresh failures (%). Stopping batch operation.', failure_count;
    END IF;
END $$;
```

## Monitoring and Reporting

### Progress Tracking
```sql
-- Create progress tracking table
CREATE TEMP TABLE batch_progress (
    batch_id TEXT DEFAULT 'batch_' || EXTRACT(EPOCH FROM NOW())::TEXT,
    entity_name TEXT,
    started_at TIMESTAMP DEFAULT NOW(),
    completed_at TIMESTAMP,
    status TEXT DEFAULT 'RUNNING',
    error_message TEXT
);

-- Update progress (call after each refresh)
INSERT INTO batch_progress (entity_name, completed_at, status)
VALUES ('your_tview_name', NOW(), 'SUCCESS');
```

### Final Report
```sql
-- Generate batch operation report
SELECT
    COUNT(*) as total_tviews,
    COUNT(*) FILTER (WHERE last_refreshed > NOW() - INTERVAL '1 hour') as refreshed,
    COUNT(*) FILTER (WHERE last_error IS NOT NULL) as failed,
    AVG(last_refresh_duration_ms) / 1000 as avg_duration_seconds,
    SUM(last_refresh_duration_ms) / 1000 / 60 as total_duration_minutes
FROM pg_tviews_metadata;
```

## Rollback Considerations

### Batch Rollback Strategy
```sql
-- For batch operations, rollback is typically not practical
-- Instead, focus on fixing individual failed TVIEWs

-- Option 1: Wait for next automated refresh cycle
-- Option 2: Manually refresh failed TVIEWs
-- Option 3: Restore from backup (last resort)
```

### Partial Recovery
```sql
-- Fix specific failed TVIEWs
SELECT pg_tviews_refresh(entity_name)
FROM pg_tviews_metadata
WHERE last_error LIKE '%specific_error_pattern%';
```

## Performance Optimization

### Optimal Batch Size
```sql
-- Calculate optimal batch size based on system capacity
SELECT
    (SELECT setting::INTEGER FROM pg_settings WHERE name = 'max_connections') / 4 as recommended_concurrent_refreshes,
    (SELECT COUNT(*) FROM pg_tviews_metadata) as total_tviews,
    CASE
        WHEN (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata) < 30000 THEN 'Can run all concurrently'
        WHEN (SELECT AVG(last_refresh_duration_ms) FROM pg_tviews_metadata) < 120000 THEN 'Run in groups of 5-10'
        ELSE 'Run sequentially with monitoring'
    END as recommended_approach
FROM pg_stat_bgwriter;
```

## Related Runbooks

- [Manual Refresh](manual-refresh.md) - Individual TVIEW refresh operations
- [Refresh Troubleshooting](refresh-troubleshooting.md) - Debug batch refresh issues
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Monitor batch operation impact
- [Queue Management](../01-health-monitoring/queue-management.md) - Handle queue during batch operations

## Best Practices

1. **Test in Staging**: Always test batch procedures in staging first
2. **Schedule Wisely**: Run during maintenance windows or off-peak hours
3. **Monitor Closely**: Track progress and system impact throughout
4. **Have Fallbacks**: Know how to pause or cancel if issues arise
5. **Document Results**: Record batch operation outcomes for future reference
6. **Consider Dependencies**: Refresh in dependency order when possible
7. **Size Appropriately**: Don't overwhelm system with too many concurrent refreshes</content>
<parameter name="filePath">docs/operations/runbooks/02-refresh-operations/batch-refresh.md