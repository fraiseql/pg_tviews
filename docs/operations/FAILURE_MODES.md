# Failure Modes and Recovery Procedures

## Database Failures

### PostgreSQL Crash During Refresh

**Symptoms**:
- Transaction in progress when PostgreSQL crashes
- TVIEW may be out of sync with backing table

**Recovery**:
1. PostgreSQL will automatically roll back uncommitted transactions
2. TVIEW will be consistent with pre-crash state
3. Re-run refresh if needed:
   ```sql
   SELECT pg_tviews_refresh('entity_name');
   ```

**Prevention**: Use 2PC for critical transactions requiring atomicity across systems.

### Disk Full

**Symptoms**:
- `ERROR: could not extend file` messages
- Transactions fail

**Recovery**:
1. Free up disk space
2. Check TVIEW consistency:
   ```sql
   -- Compare row counts
   SELECT COUNT(*) FROM backing_table;
   SELECT COUNT(*) FROM tview_table;
   ```
3. If inconsistent, force refresh:
   ```sql
   SELECT pg_tviews_refresh('entity_name', force => true);
   ```

**Prevention**: Monitor disk usage, set up alerts at 80% capacity.

### Out of Memory

**Symptoms**:
- `ERROR: out of memory` during large refresh
- PostgreSQL may restart

**Recovery**:
1. Increase `work_mem` for session:
   ```sql
   SET work_mem = '256MB';
   SELECT pg_tviews_refresh('large_entity');
   RESET work_mem;
   ```
2. Consider batch refresh instead of full refresh

**Prevention**: Use incremental refresh, avoid `force => true` on large TVIEWs.

### Connection Loss

**Symptoms**:
- Client disconnects during transaction
- Network partition

**Recovery**:
1. Transaction automatically rolled back
2. TVIEW remains consistent
3. Reconnect and retry operation

**Prevention**: Use connection pooling with proper timeout settings.

---

## Extension Failures

### Circular Dependency

**Symptoms**:
```
ERROR: Circular dependency detected: tv_a -> tv_b -> tv_a
```

**Recovery**:
1. Identify cycle in dependencies
2. Break cycle by dropping one TVIEW:
   ```sql
   DROP TABLE tv_b CASCADE;
   ```
3. Recreate without circular reference

**Prevention**: Design TVIEW dependency graph as DAG (directed acyclic graph).

### Metadata Corruption

**Symptoms**:
- `ERROR: Metadata not found for TVIEW: entity_name`
- Triggers exist but no metadata entry

**Recovery**:
1. Re-convert TVIEW:
   ```sql
   SELECT pg_tviews_convert_existing_table('entity_name');
   ```
2. Verify metadata:
   ```sql
   SELECT * FROM pg_tviews_metadata WHERE entity_name = 'entity_name';
   ```

**Prevention**: Do not manually modify `pg_tviews_metadata` table.

### Queue Persistence Corruption

**Symptoms**:
- Orphaned entries in `pg_tview_pending_refreshes`
- 2PC transactions never committed/rolled back

**Recovery**:
1. List orphaned entries:
   ```sql
   SELECT gid, prepared FROM pg_tview_pending_refreshes
   WHERE age(now(), prepared) > interval '1 hour';
   ```
2. Manually clean up:
   ```sql
   DELETE FROM pg_tview_pending_refreshes
   WHERE gid = 'orphaned_transaction_id';
   ```

**Prevention**: Always commit or rollback prepared transactions promptly.

### Trigger Malfunction

**Symptoms**:
- Triggers disabled or dropped
- Refresh not happening on DML

**Recovery**:
1. Check trigger status:
   ```sql
   SELECT * FROM information_schema.triggers
   WHERE trigger_name LIKE 'pg_tviews_%';
   ```
2. Recreate TVIEW to restore triggers:
   ```sql
   SELECT pg_tviews_convert_existing_table('entity_name');
   ```

**Prevention**: Avoid DDL operations on TVIEW tables.

---

## Operational Failures

### PostgreSQL Version Upgrade

**Procedure**:
1. Before upgrade:
   ```bash
   # Backup
   pg_dump mydb > backup.sql

   # Note pg_tviews version
   psql -c "SELECT pg_tviews_version();"
   ```

2. Upgrade PostgreSQL:
   ```bash
   # Standard PostgreSQL upgrade procedure
   pg_upgrade ...
   ```

3. Reinstall pg_tviews:
   ```bash
   cargo pgrx install --release --pg-config=/path/to/new/pg_config
   ```

4. Verify:
   ```sql
   SELECT pg_tviews_version();
   SELECT entity_name, COUNT(*) FROM pg_tviews_metadata;
   ```

**Known Issues**: Extension must be reinstalled after major PostgreSQL upgrades.

### Backup and Restore

**Backup** (recommended):
```bash
# Full logical backup includes TVIEW definition and data
pg_dump -Fc mydb > mydb.dump
```

**Restore**:
```bash
pg_restore -d mydb_restored mydb.dump
```

**Verification**:
```sql
-- Check all TVIEWs restored
SELECT entity_name FROM pg_tviews_metadata;

-- Verify refresh works
INSERT INTO backing_table VALUES (...);
-- Check TVIEW updated
```

**Caution**: Physical backups (PITR) may have consistency issues if taken during refresh.

### Replication Lag

**Symptoms**:
- Replica TVIEW data stale
- Cascade refresh on replica

**Recovery**:
1. Wait for replication to catch up
2. Manual refresh on replica if needed:
   ```sql
   SELECT pg_tviews_refresh('entity_name');
   ```

**Prevention**: Monitor replication lag, avoid heavy refresh operations during peak hours.

### Concurrent DDL

**Scenario**: `DROP TABLE tv_entity` while refresh in progress

**Behavior**:
- Refresh transaction will fail
- Error message: `relation "tv_entity" does not exist`
- No data corruption

**Recovery**: None needed - transaction rolled back cleanly.

**Prevention**: Use DDL locks or maintenance windows for TVIEW DDL.

---

## Emergency Procedures

### Force Refresh All TVIEWs

```sql
-- Refresh all TVIEWs (use with caution on large databases)
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN SELECT entity_name FROM pg_tviews_metadata LOOP
        RAISE NOTICE 'Refreshing %', rec.entity_name;
        PERFORM pg_tviews_refresh(rec.entity_name, force => true);
    END LOOP;
END $$;
```

### Disable All TVIEW Triggers (Emergency)

```sql
-- Disable refresh triggers (stops automatic refresh)
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN
        SELECT DISTINCT trigger_name, event_object_table
        FROM information_schema.triggers
        WHERE trigger_name LIKE 'pg_tviews_%'
    LOOP
        EXECUTE format('ALTER TABLE %I DISABLE TRIGGER %I',
                      rec.event_object_table, rec.trigger_name);
    END LOOP;
END $$;
```

### Re-enable Triggers

```sql
DO $$
DECLARE
    rec RECORD;
BEGIN
    FOR rec IN
        SELECT DISTINCT trigger_name, event_object_table
        FROM information_schema.triggers
        WHERE trigger_name LIKE 'pg_tviews_%'
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE TRIGGER %I',
                      rec.event_object_table, rec.trigger_name);
    END LOOP;
END $$;
```

---

## Monitoring and Alerts

### Key Metrics to Monitor

1. **Queue size**: Should be 0 between transactions
   ```sql
   SELECT jsonb_array_length(pg_tviews_debug_queue());
   ```

2. **Orphaned prepared transactions**:
   ```sql
   SELECT COUNT(*) FROM pg_tview_pending_refreshes
   WHERE age(now(), prepared_at) > interval '1 hour';
   ```

3. **TVIEW consistency** (periodic check):
   ```sql
   SELECT entity_name,
          (SELECT COUNT(*) FROM tb_||entity_name) as backing_count,
          (SELECT COUNT(*) FROM tv_||entity_name) as tview_count
   FROM pg_tviews_metadata;
   ```

### Alert Thresholds

- **Critical**: Queue size > 1000 for > 5 minutes
- **Warning**: Orphaned 2PC transactions > 10
- **Info**: TVIEW refresh took > 1 second

---

## Support

For issues not covered here, see:
- [GitHub Issues](https://github.com/fraiseql/pg_tviews/issues)
- [Troubleshooting Guide](./TROUBLESHOOTING.md)