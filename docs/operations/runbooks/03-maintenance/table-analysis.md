# Table Analysis Runbook

## Purpose
Analyze TVIEW table statistics, identify performance issues, and optimize storage and query performance for pg_tviews.

## When to Use
- **Monthly Analysis**: Regular table health assessment
- **Performance Issues**: When queries against TVIEWs are slow
- **Storage Alerts**: When disk usage grows unexpectedly
- **After Bulk Operations**: Following large data imports or updates
- **Query Optimization**: Before tuning slow TVIEW queries

## Prerequisites
- PostgreSQL analysis permissions (`ANALYZE`, `VACUUM`)
- Access to `pg_stat_user_tables` and `pg_stat_user_indexes`
- Understanding of PostgreSQL table statistics
- Backup recommended before major changes

## Table Statistics Analysis (20 minutes)

### Step 1: TVIEW Table Inventory
```sql
-- Get comprehensive TVIEW table statistics
SELECT
    t.entity_name,
    pg_size_pretty(pg_total_relation_size(t.entity_name)) as total_size,
    pg_size_pretty(pg_relation_size(t.entity_name)) as table_size,
    pg_size_pretty(pg_total_relation_size(t.entity_name) - pg_relation_size(t.entity_name)) as index_size,
    t.last_refreshed,
    t.last_refresh_duration_ms / 1000 as last_refresh_seconds
FROM pg_tviews_metadata t
ORDER BY pg_total_relation_size(t.entity_name) DESC;
```

### Step 2: Table Health Metrics
```sql
-- Analyze table health and maintenance status
SELECT
    schemaname,
    tablename,
    n_tup_ins as inserts,
    n_tup_upd as updates,
    n_tup_del as deletes,
    n_live_tup as live_rows,
    n_dead_tup as dead_rows,
    ROUND(n_dead_tup::numeric / NULLIF(n_live_tup + n_dead_tup, 0) * 100, 2) as bloat_ratio,
    last_vacuum,
    last_autovacuum,
    last_analyze,
    last_autoanalyze,
    CASE
        WHEN n_dead_tup > n_live_tup * 0.5 THEN 'HIGH_BLOAT'
        WHEN n_dead_tup > n_live_tup * 0.2 THEN 'MEDIUM_BLOAT'
        WHEN last_analyze < NOW() - INTERVAL '7 days' THEN 'STALE_STATS'
        ELSE 'HEALTHY'
    END as health_status
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY n_dead_tup DESC;
```

### Step 3: Index Effectiveness Analysis
```sql
-- Evaluate index usage and effectiveness
SELECT
    schemaname,
    tablename,
    indexname,
    idx_scan as index_scans,
    idx_tup_read as tuples_read_via_index,
    idx_tup_fetch as tuples_fetched_via_index,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size,
    CASE
        WHEN idx_scan = 0 THEN 'UNUSED'
        WHEN idx_scan < 100 THEN 'LOW_USAGE'
        WHEN idx_scan < 1000 THEN 'MODERATE_USAGE'
        ELSE 'HIGH_USAGE'
    END as usage_category
FROM pg_stat_user_indexes
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY idx_scan DESC, pg_relation_size(indexrelid) DESC;
```

## Performance Optimization Procedures

### Step 1: Statistics Update
```sql
-- Update statistics for all TVIEW tables
DO $$
DECLARE
    tview_record RECORD;
BEGIN
    FOR tview_record IN
        SELECT entity_name FROM pg_tviews_metadata
    LOOP
        BEGIN
            EXECUTE 'ANALYZE ' || tview_record.entity_name;
            RAISE NOTICE 'Updated statistics for TVIEW: %', tview_record.entity_name;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Failed to analyze TVIEW %: %', tview_record.entity_name, SQLERRM;
        END;
    END LOOP;
END $$;

-- Verify statistics are current
SELECT
    schemaname,
    tablename,
    last_analyze,
    CASE
        WHEN last_analyze > NOW() - INTERVAL '1 hour' THEN 'CURRENT'
        WHEN last_analyze > NOW() - INTERVAL '1 day' THEN 'RECENT'
        ELSE 'STALE'
    END as stats_status
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%';
```

### Step 2: Bloat Assessment and Cleanup
```sql
-- Identify tables needing vacuum
SELECT
    schemaname,
    tablename,
    n_dead_tup,
    n_live_tup,
    ROUND(n_dead_tup::numeric / NULLIF(n_live_tup + n_dead_tup, 0) * 100, 2) as bloat_percent,
    pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename)) as total_size
FROM pg_stat_user_tables
WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
  AND n_dead_tup > n_live_tup * 0.1
ORDER BY n_dead_tup DESC;

-- Vacuum tables with high bloat
DO $$
DECLARE
    table_record RECORD;
BEGIN
    FOR table_record IN
        SELECT schemaname, tablename
        FROM pg_stat_user_tables
        WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
          AND n_dead_tup > n_live_tup * 0.2
    LOOP
        BEGIN
            EXECUTE 'VACUUM ' || table_record.schemaname || '.' || table_record.tablename;
            RAISE NOTICE 'Vacuumed table: %.%', table_record.schemaname, table_record.tablename;
        EXCEPTION WHEN OTHERS THEN
            RAISE NOTICE 'Failed to vacuum %.%: %', table_record.schemaname, table_record.tablename, SQLERRM;
        END;
    END LOOP;
END $$;
```

### Step 3: Index Optimization
```sql
-- Identify potentially redundant or unused indexes
WITH index_usage AS (
    SELECT
        schemaname,
        tablename,
        indexname,
        idx_scan,
        pg_relation_size(indexrelid) as index_size_bytes
    FROM pg_stat_user_indexes
    WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
)
SELECT
    iu.schemaname,
    iu.tablename,
    iu.indexname,
    iu.idx_scan,
    pg_size_pretty(iu.index_size_bytes) as index_size,
    CASE
        WHEN iu.idx_scan = 0 AND iu.index_size_bytes > 10000000 THEN 'CONSIDER_DROP_LARGE_UNUSED'
        WHEN iu.idx_scan = 0 THEN 'CONSIDER_DROP_UNUSED'
        WHEN iu.idx_scan < 10 THEN 'LOW_USAGE_MONITOR'
        ELSE 'ACTIVE'
    END as recommendation
FROM index_usage iu
ORDER BY iu.index_size_bytes DESC;

-- Reindex indexes with high usage (if needed)
-- REINDEX INDEX CONCURRENTLY index_name;
```

## Query Performance Analysis

### Step 1: Slow Query Identification
```sql
-- Find slow queries involving TVIEWs
SELECT
    query,
    calls,
    total_time / 1000 as total_time_seconds,
    mean_time / 1000 as mean_time_seconds,
    rows,
    LEFT(query, 100) as query_preview
FROM pg_stat_statements
WHERE (query LIKE '%tview%' OR query LIKE '%refresh%')
  AND mean_time > 1000  -- Queries taking > 1 second on average
ORDER BY mean_time DESC
LIMIT 10;
```

### Step 2: Query Plan Analysis
```sql
-- Analyze query plans for TVIEW tables
-- Replace 'your_tview_name' with actual TVIEW name
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM your_tview_name
WHERE updated_at > NOW() - INTERVAL '1 day'
ORDER BY id
LIMIT 100;

-- Look for:
-- - Sequential scans on large tables
-- - Missing index usage
-- - High buffer usage
```

### Step 3: Index Recommendations
```sql
-- Suggest indexes based on query patterns
SELECT
    schemaname,
    tablename,
    attname,
    n_distinct,
    correlation,
    CASE
        WHEN n_distinct > 1000 AND correlation > 0.9 THEN 'EXCELLENT_INDEX_CANDIDATE'
        WHEN n_distinct > 1000 AND correlation > 0.5 THEN 'GOOD_INDEX_CANDIDATE'
        WHEN n_distinct > 100 THEN 'MODERATE_INDEX_CANDIDATE'
        ELSE 'POOR_INDEX_CANDIDATE'
    END as index_potential
FROM pg_stats
WHERE schemaname LIKE '%tview%'
  AND attname NOT IN ('id', 'created_at', 'updated_at')  -- Common already-indexed columns
ORDER BY n_distinct DESC, correlation DESC;
```

## Storage Optimization

### Step 1: Table Size Analysis
```sql
-- Detailed table size breakdown
SELECT
    schemaname,
    tablename,
    pg_size_pretty(pg_relation_size(schemaname || '.' || tablename)) as table_size,
    pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename) - pg_relation_size(schemaname || '.' || tablename)) as index_size,
    pg_size_pretty(pg_total_relation_size(schemaname || '.' || tablename)) as total_size,
    n_live_tup as live_rows,
    ROUND(pg_total_relation_size(schemaname || '.' || tablename)::numeric / NULLIF(n_live_tup, 0), 0) as bytes_per_row
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
ORDER BY pg_total_relation_size(schemaname || '.' || tablename) DESC;
```

### Step 2: Partitioning Assessment
```sql
-- Assess if large TVIEWs would benefit from partitioning
SELECT
    t.entity_name,
    pg_size_pretty(pg_total_relation_size(t.entity_name)) as size,
    t.last_refresh_duration_ms / 1000 as refresh_time_seconds,
    CASE
        WHEN pg_total_relation_size(t.entity_name) > 10000000000 THEN 'CONSIDER_PARTITIONING_VERY_LARGE'  -- > 10GB
        WHEN pg_total_relation_size(t.entity_name) > 1000000000 THEN 'CONSIDER_PARTITIONING_LARGE'     -- > 1GB
        WHEN t.last_refresh_duration_ms > 300000 THEN 'CONSIDER_PARTITIONING_SLOW_REFRESH'         -- > 5min
        ELSE 'PARTITIONING_NOT_NEEDED'
    END as partitioning_recommendation
FROM pg_tviews_metadata t
ORDER BY pg_total_relation_size(t.entity_name) DESC;
```

### Step 3: Compression Opportunities
```sql
-- Check for compression opportunities
SELECT
    schemaname,
    tablename,
    attname,
    n_distinct,
    avg_width,
    CASE
        WHEN avg_width > 100 AND n_distinct > 1000 THEN 'CONSIDER_COMPRESSION'
        WHEN avg_width > 500 THEN 'HIGH_COMPRESSION_CANDIDATE'
        ELSE 'COMPRESSION_NOT_NEEDED'
    END as compression_recommendation
FROM pg_stats
WHERE schemaname LIKE '%tview%'
  AND attname NOT LIKE '%id%'
ORDER BY avg_width DESC;
```

## Maintenance Automation

### Monthly Analysis Script
```sql
-- Create automated table analysis function
CREATE OR REPLACE FUNCTION pg_tviews_table_analysis()
RETURNS TABLE (
    table_name TEXT,
    issue_type TEXT,
    severity TEXT,
    recommendation TEXT,
    estimated_impact TEXT
) AS $$
BEGIN
    -- Bloat detection
    RETURN QUERY
    SELECT
        schemaname || '.' || tablename,
        'BLOAT'::TEXT,
        CASE WHEN n_dead_tup > n_live_tup * 0.5 THEN 'HIGH' ELSE 'MEDIUM' END,
        'Run VACUUM to reclaim space'::TEXT,
        pg_size_pretty(n_dead_tup * 100)::TEXT || ' estimated savings'
    FROM pg_stat_user_tables
    WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
      AND n_dead_tup > n_live_tup * 0.2;

    -- Stale statistics
    RETURN QUERY
    SELECT
        schemaname || '.' || tablename,
        'STALE_STATISTICS'::TEXT,
        'MEDIUM'::TEXT,
        'Run ANALYZE to update statistics'::TEXT,
        'Query performance degradation'::TEXT
    FROM pg_stat_user_tables
    WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
      AND last_analyze < NOW() - INTERVAL '7 days';

    -- Unused indexes
    RETURN QUERY
    SELECT
        schemaname || '.' || tablename,
        'UNUSED_INDEX'::TEXT,
        'LOW'::TEXT,
        'Consider dropping unused index'::TEXT,
        pg_size_pretty(pg_relation_size(indexrelid))::TEXT || ' space savings'
    FROM pg_stat_user_indexes
    WHERE (schemaname LIKE '%tview%' OR tablename LIKE '%tview%')
      AND idx_scan = 0
      AND pg_relation_size(indexrelid) > 1000000;  -- > 1MB

    RETURN;
END;
$$ LANGUAGE plpgsql;
```

### Automated Monitoring
```bash
# Monthly table analysis cron job
# 0 2 1 * * psql -h $DB_HOST -U $DB_USER -d $DB_NAME -c "SELECT * FROM pg_tviews_table_analysis();"
```

## Troubleshooting Table Issues

### High Bloat Resolution
```sql
-- For tables with > 50% bloat, consider full vacuum
-- WARNING: This locks the table and may take time

-- Check current bloat
SELECT
    schemaname || '.' || tablename as full_table_name,
    ROUND(n_dead_tup::numeric / (n_live_tup + n_dead_tup) * 100, 2) as bloat_percent
FROM pg_stat_user_tables
WHERE schemaname LIKE '%tview%' OR tablename LIKE '%tview%'
  AND n_dead_tup > n_live_tup * 0.5;

-- Perform full vacuum during maintenance window
VACUUM FULL your_bloated_tview;

-- Alternative: Concurrent reindex and vacuum
REINDEX TABLE CONCURRENTLY your_bloated_tview;
VACUUM your_bloated_tview;
```

### Index Performance Issues
```sql
-- For indexes with poor performance
ANALYZE your_tview_name;  -- Update statistics

-- Check if index needs rebuilding
REINDEX INDEX CONCURRENTLY your_index_name;

-- Consider index changes based on query patterns
-- CREATE INDEX CONCURRENTLY new_index_name ON your_tview_name (column_name);
-- DROP INDEX CONCURRENTLY old_index_name;
```

### Query Optimization
```sql
-- For slow TVIEW queries, analyze execution plan
EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT * FROM your_tview_name WHERE your_condition;

-- Common optimizations:
-- 1. Add missing indexes
-- 2. Rewrite queries to use indexed columns
-- 3. Update table statistics
-- 4. Consider query restructuring
```

## Related Runbooks

- [Regular Maintenance](regular-maintenance.md) - Overall maintenance procedures
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Performance trend analysis
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Query performance issues
- [Connection Management](connection-management.md) - Connection-related table issues

## Best Practices

1. **Regular Analysis**: Run table analysis monthly
2. **Monitor Bloat**: Keep bloat under 20% through regular vacuuming
3. **Update Statistics**: Ensure statistics are current for good query planning
4. **Index Maintenance**: Rebuild or drop unused indexes
5. **Query Monitoring**: Track slow queries and optimize as needed
6. **Storage Planning**: Monitor growth trends and plan for scaling
7. **Document Changes**: Record all table structure changes and their rationale</content>
<parameter name="filePath">docs/operations/runbooks/03-maintenance/table-analysis.md