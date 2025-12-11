# Operational Runbooks

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

## Overview

This document provides step-by-step procedures for diagnosing and resolving common pg_tviews operational issues. Each runbook includes symptoms, diagnosis steps, and resolution procedures.

## Runbook 1: TVIEW Not Updating

**Symptom**: Data changes in base tables not reflected in tv_* tables

**Diagnosis Steps**:

```sql
-- 1. Check if triggers exist
-- Trinity pattern: tb_your_table has pk_your_table (integer), id (UUID)
SELECT
  pg_trigger.tgname,
  pg_trigger.tgrelid::regclass,
  pg_trigger.tgenabled
FROM pg_trigger
WHERE pg_trigger.tgname LIKE 'tview%'
  AND pg_trigger.tgrelid = 'tb_your_table'::regclass;
```

```sql
-- 2. Check metadata
SELECT * FROM pg_tview_meta WHERE pg_tview_meta.entity = 'your_entity';
```

```sql
-- 3. Check for errors in logs
-- (Review PostgreSQL logs for TVIEW-related errors)
```

```sql
-- 4. Manual refresh test
-- Note: pk_your_table is integer (SERIAL), id is UUID
UPDATE tb_your_table
SET some_field = tb_your_table.some_field
WHERE tb_your_table.pk_your_table = 1;
COMMIT;
SELECT * FROM tv_your_entity WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:

1. **If triggers missing**: Recreate TVIEW
   ```sql
   DROP TABLE tv_your_entity CASCADE;
   CREATE TABLE tv_your_entity AS
   SELECT
     tb_your_table.pk_your_table,
     tb_your_table.id,
     jsonb_build_object('id', tb_your_table.id, 'field', tb_your_table.field) as data
   FROM tb_your_table;
   ```

2. **If metadata corrupt**: Clean up and recreate
   ```sql
   DELETE FROM pg_tview_meta WHERE entity = 'your_entity';
   -- Then recreate TVIEW as above
   ```

3. **If errors in logs**: Address root cause (permissions, syntax, etc.)

---

## Runbook 2: Slow Cascade Updates

**Symptom**: Cascade updates taking >1 second

**Diagnosis Steps**:

```sql
-- 1. Check if jsonb_ivm installed
SELECT pg_tviews_check_jsonb_ivm();
```

```sql
-- 2. Check dependency depth
SELECT
  pg_tview_meta.entity,
  array_length(pg_tview_meta.dependencies, 1) as dep_count
FROM pg_tview_meta
ORDER BY dep_count DESC;
```

```sql
-- 3. Check for missing indexes
SELECT
  pg_indexes.schemaname,
  pg_indexes.tablename,
  pg_indexes.indexname
FROM pg_indexes
WHERE pg_indexes.tablename LIKE 'tv_%'
  AND pg_indexes.indexname NOT LIKE '%pkey%';
```

```sql
-- 4. Analyze query plans
-- Note: pk_your_entity is integer, id is UUID
EXPLAIN ANALYZE
UPDATE tv_your_entity
SET data = tv_your_entity.data
WHERE tv_your_entity.pk_your_entity = 1;
```

**Resolution**:

1. **Install jsonb_ivm if missing** (1.5-3× speedup)
   ```sql
   CREATE EXTENSION jsonb_ivm;
   ```

2. **Create indexes on fk_* columns**
   ```sql
   CREATE INDEX idx_tv_your_entity_fk_parent ON tv_your_entity(fk_parent);
   ```

3. **Enable statement-level triggers for bulk operations**
   ```sql
   SELECT pg_tviews_install_stmt_triggers();
   ```

4. **Consider flattening deep dependency chains**
   - Redesign to reduce cascade depth
   - Use computed columns instead of joins where possible

---

## Runbook 3: Out of Memory During Cascade

**Symptom**: PostgreSQL OOM killer or "out of memory" errors

**Diagnosis Steps**:

```sql
-- 1. Check cascade size
-- Trinity pattern: tv_your_entity has pk_your_entity (int), id (UUID), data (JSONB)
SELECT
  pg_tview_meta.entity,
  pg_size_pretty(pg_relation_size('tv_' || pg_tview_meta.entity)) as tview_size,
  array_length(pg_tview_meta.dependencies, 1) as cascade_depth
FROM pg_tview_meta
ORDER BY pg_relation_size('tv_' || pg_tview_meta.entity) DESC;
```

```sql
-- 2. Check work_mem setting
SHOW work_mem;
```

```sql
-- 3. Monitor memory during cascade
SELECT
  pg_stat_activity.pid,
  pg_stat_activity.query,
  pg_stat_activity.state,
  pg_size_pretty(pg_backend_memory_contexts.total_bytes)
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
WHERE pg_stat_activity.backend_type = 'client backend';
```

**Resolution**:

1. **Increase work_mem** (session or global)
   ```sql
   SET work_mem = '256MB';  -- Or higher
   -- Or globally: ALTER SYSTEM SET work_mem = '256MB';
   ```

2. **Batch large updates**
   ```sql
   -- Instead of updating all rows at once:
   UPDATE tb_large_table SET field = value;

   -- Do it in batches:
   UPDATE tb_large_table SET field = value
   WHERE pk_large_table BETWEEN 1 AND 10000;

   UPDATE tb_large_table SET field = value
   WHERE pk_large_table BETWEEN 10001 AND 20000;
   ```

3. **Consider partitioning large TVIEWs**
   ```sql
   -- Partition by date for time-series data
   CREATE TABLE tv_event_y2025_m01 PARTITION OF tv_event
   FOR VALUES FROM ('2025-01-01') TO ('2025-02-01');
   ```

4. **Implement rate limiting for bulk operations**
   - Add application-level throttling
   - Use smaller transaction sizes

---

## Runbook 4: Extension Upgrade Failed

**Symptom**: `ALTER EXTENSION pg_tviews UPDATE` fails

**Diagnosis Steps**:

```sql
-- 1. Check current version
SELECT * FROM pg_extension WHERE pg_extension.extname = 'pg_tviews';
```

```sql
-- 2. Check for version mismatch
SELECT pg_tviews_version();
```

```sql
-- 3. Review upgrade script
-- cat $(pg_config --sharedir)/extension/pg_tviews--oldver--newver.sql
```

**Resolution**:

1. **Backup metadata**: `CREATE TABLE pg_tview_meta_backup AS SELECT * FROM pg_tview_meta;`

2. **If upgrade fails, rollback**:
   ```sql
   ALTER EXTENSION pg_tviews UPDATE TO 'old_version';
   ```

3. **Restore metadata if needed**
   ```sql
   TRUNCATE pg_tview_meta;
   INSERT INTO pg_tview_meta SELECT * FROM pg_tview_meta_backup;
   ```

4. **Contact maintainer if persistent issue**
   - Include full error messages
   - PostgreSQL version and logs
   - pg_tviews version being upgraded from/to

---

## Runbook 5: Orphaned Triggers After TVIEW Drop

**Symptom**: Triggers remain after dropping TVIEW

**Diagnosis Steps**:

```sql
-- Find orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
SELECT
  pg_trigger.tgname,
  pg_class.relname
FROM pg_trigger
JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
WHERE pg_trigger.tgname LIKE 'tview_%'
  AND NOT EXISTS (
    SELECT 1 FROM pg_tview_meta
    WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
  );
```

**Resolution**:

```sql
-- Drop orphaned triggers
-- Trinity pattern: All base tables named tb_{entity} (singular)
DO $$
DECLARE
    r RECORD;
BEGIN
    FOR r IN
        SELECT
          pg_trigger.tgname,
          pg_class.relname
        FROM pg_trigger
        JOIN pg_class ON pg_trigger.tgrelid = pg_class.oid
        WHERE pg_trigger.tgname LIKE 'tview_%'
          AND NOT EXISTS (
            SELECT 1 FROM pg_tview_meta
            WHERE pg_class.relname = 'tb_' || pg_tview_meta.entity
          )
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', r.tgname, r.relname);
    END LOOP;
END $$;
```

---

## Runbook 6: Data Inconsistency Between Base and TVIEW

**Symptom**: tv_* table shows different data than base table

**Diagnosis Steps**:

```sql
-- 1. Compare row counts
SELECT
  (SELECT COUNT(*) FROM tb_your_table) as base_count,
  (SELECT COUNT(*) FROM tv_your_table) as tview_count;
```

```sql
-- 2. Check for missing rows in TVIEW
SELECT tb_your_table.pk_your_table
FROM tb_your_table
LEFT JOIN tv_your_table ON tb_your_table.pk_your_table = tv_your_table.pk_your_table
WHERE tv_your_table.pk_your_table IS NULL;
```

```sql
-- 3. Check for stale data in TVIEW
SELECT
  tb_your_table.pk_your_table,
  tb_your_table.last_updated,
  tv_your_table.data->>'lastUpdated' as tview_updated
FROM tb_your_table
JOIN tv_your_table ON tb_your_table.pk_your_table = tv_your_table.pk_your_table
WHERE tb_your_table.last_updated > (tv_your_table.data->>'lastUpdated')::timestamptz;
```

**Resolution**:

1. **Force refresh affected rows**
   ```sql
   -- Manual cascade for specific rows
   SELECT pg_tviews_cascade('tb_your_table'::regclass::oid, pk_value);
   ```

2. **Full TVIEW recreation**
   ```sql
   DROP TABLE tv_your_table;
   CREATE TABLE tv_your_table AS
   SELECT
     tb_your_table.pk_your_table,
     tb_your_table.id,
     jsonb_build_object('id', tb_your_table.id, /* ... */) as data
   FROM tb_your_table;
   ```

3. **Check for trigger failures**
   ```sql
   -- Review PostgreSQL logs for trigger errors
   -- Check trigger enablement: ALTER TABLE tb_your_table ENABLE TRIGGER ALL;
   ```

---

## Runbook 7: High CPU Usage During Refresh

**Symptom**: CPU spikes during TVIEW refresh operations

**Diagnosis Steps**:

```sql
-- 1. Check active refresh operations
SELECT
  pg_stat_activity.pid,
  pg_stat_activity.query,
  pg_stat_activity.state,
  pg_stat_activity.wait_event_type,
  pg_stat_activity.wait_event
FROM pg_stat_activity
WHERE pg_stat_activity.query LIKE '%tview%' OR pg_stat_activity.query LIKE '%tv_%';
```

```sql
-- 2. Check for expensive operations
EXPLAIN (ANALYZE, BUFFERS)
UPDATE tb_your_table SET field = value WHERE condition;
```

```sql
-- 3. Monitor system resources
SELECT
  pg_stat_activity.pid,
  pg_stat_activity.usename,
  pg_stat_activity.client_addr,
  pg_size_pretty(pg_backend_memory_contexts.total_bytes) as memory_used
FROM pg_stat_activity
JOIN LATERAL pg_backend_memory_contexts ON true
WHERE pg_stat_activity.state = 'active';
```

**Resolution**:

1. **Optimize queries**
   ```sql
   -- Add missing indexes
   CREATE INDEX idx_tb_your_table_field ON tb_your_table(field);

   -- Rewrite expensive operations
   -- Instead of: UPDATE tb_large SET computed = expensive_function(field)
   -- Use: UPDATE tb_large SET computed = expensive_function(field) WHERE pk_large IN (SELECT ... LIMIT 1000)
   ```

2. **Reduce cascade frequency**
   ```sql
   -- Batch updates instead of individual ones
   -- Use statement-level triggers for bulk operations
   SELECT pg_tviews_install_stmt_triggers();
   ```

3. **Scale resources**
   ```sql
   -- Increase CPU allocation
   -- Consider read replicas for heavy queries
   ```

---

## Emergency Procedures

### Complete System Reset

**Use only as last resort**

```sql
-- 1. Stop all application connections
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = current_database()
  AND pid != pg_backend_pid();

-- 2. Drop all TVIEWs
DO $$
DECLARE
    rec record;
BEGIN
    FOR rec IN SELECT entity FROM pg_tview_meta LOOP
        EXECUTE 'DROP TABLE tv_' || rec.entity || ' CASCADE';
    END LOOP;
END $$;

-- 3. Clean metadata
TRUNCATE pg_tview_meta;
TRUNCATE pg_tview_helpers;

-- 4. Recreate TVIEWs from application scripts
-- (Run your TVIEW creation scripts)

-- 5. Verify system health
SELECT * FROM pg_tviews_health_check();
```

## Monitoring Integration

### Automated Health Checks

```bash
#!/bin/bash
# daily_health_check.sh

# Run health check
psql -d your_db -c "SELECT * FROM pg_tviews_health_check()" > health_check.log

# Check for issues
if grep -q "ERROR\|WARNING" health_check.log; then
    echo "Health check failed" | mail -s "pg_tviews Health Alert" admin@yourcompany.com
fi
```

### Alert Thresholds

- **Queue size > 1000**: Immediate alert
- **Update latency > 5 seconds**: Warning
- **Orphaned triggers > 0**: Warning
- **Metadata inconsistencies**: Critical alert
- **Memory usage > 80%**: Warning

## See Also

- [Monitoring Guide](../MONITORING.md) - Health check details
- [Troubleshooting Guide](troubleshooting.md) - Additional debugging steps
- [Performance Tuning](performance-tuning.md) - Optimization strategies