# Upgrade & Migration Guide

**Version**: 0.1.0-beta.1
**Last Updated**: December 11, 2025

## Overview

This guide provides procedures for upgrading pg_tviews and migrating data between versions. All upgrades follow a safe, rollback-capable process.

## Pre-Upgrade Checklist

Before any upgrade:

```bash
# 1. Backup your database
pg_dump -Fc your_database > backup_$(date +%Y%m%d_%H%M%S).dump

# 2. Check current version
psql -d your_database -c "SELECT pg_tviews_version();"

# 3. Review breaking changes in CHANGELOG.md
# 4. Test upgrade in staging environment first
# 5. Schedule maintenance window
```

## Upgrade Procedures

### Minor Version Upgrades (0.x.y → 0.x.z)

Safe upgrades with no breaking changes:

```bash
# 1. Stop application (optional for minor versions)
# 2. Upgrade extension
psql -d your_database -c "ALTER EXTENSION pg_tviews UPDATE;"

# 3. Verify version
psql -d your_database -c "SELECT pg_tviews_version();"

# 4. Run health check
psql -d your_database -c "SELECT * FROM pg_tviews_health_check();"
```

### Major Version Upgrades (0.x → 0.y)

May include breaking changes:

```bash
# 1. Stop application
# 2. Backup database (extra careful)
# 3. Drop extension
psql -d your_database -c "DROP EXTENSION pg_tviews;"

# 4. Install new version
# (Follow installation instructions for new version)

# 5. Recreate extension
psql -d your_database -c "CREATE EXTENSION pg_tviews;"

# 6. Recreate TVIEWs (they are dropped with extension)
# (Run your TVIEW creation scripts)

# 7. Verify functionality
psql -d your_database -c "SELECT * FROM pg_tviews_health_check();"
```

## Rollback Procedures

### Immediate Rollback (Extension Still Works)

If issues discovered immediately after upgrade:

```bash
# 1. Stop application
# 2. Restore from backup
pg_restore -d your_database backup_file.dump

# 3. Verify rollback
psql -d your_database -c "SELECT pg_tviews_version();"
```

### Delayed Rollback (Extension Modified Data)

If TVIEWs have been modified since upgrade:

```bash
# 1. Export current TVIEW data
psql -d your_database -c "
  \COPY (SELECT * FROM tv_table1) TO 'tv_table1_backup.csv' CSV HEADER
  \COPY (SELECT * FROM tv_table2) TO 'tv_table2_backup.csv' CSV HEADER
"

# 2. Restore from backup
pg_restore -d your_database backup_file.dump

# 3. Recreate TVIEWs
# (Run TVIEW creation scripts)

# 4. Reimport modified data if needed
# (Careful: may cause conflicts)
```

## Data Migration

### Schema Changes Between Versions

When upgrading between versions with schema changes:

```sql
-- Example: Adding new metadata columns
ALTER TABLE pg_tview_meta ADD COLUMN IF NOT EXISTS version_created TEXT;
UPDATE pg_tview_meta SET version_created = '0.1.0' WHERE version_created IS NULL;
```

### TVIEW Recreation

After schema changes that affect TVIEWs:

```sql
-- 1. Export TVIEW definitions (if available)
-- 2. Drop TVIEWs
SELECT pg_tviews_drop(entity, true) FROM pg_tview_meta;

-- 3. Recreate with updated definitions
-- (Run updated TVIEW creation scripts)

-- 4. Verify data integrity
SELECT COUNT(*) FROM tv_table;
SELECT COUNT(*) FROM v_table;
```

## Version Compatibility Matrix

| Current Version | Target Version | Upgrade Path | Notes |
|----------------|----------------|--------------|-------|
| 0.1.0-alpha | 0.1.0-beta.1 | Direct | Safe, no data migration |
| 0.1.0-beta.1 | 0.1.0-rc.1 | Direct | Test in staging first |
| 0.1.0-rc.1 | 1.0.0 | Migration required | Breaking changes possible |

## Troubleshooting Upgrades

### Extension Won't Load

```bash
# Check PostgreSQL logs
tail -f /var/log/postgresql/postgresql-*.log

# Verify shared library
ls -la $(pg_config --pkglibdir)/pg_tviews.so

# Check dependencies
ldd $(pg_config --pkglibdir)/pg_tviews.so
```

### TVIEWs Not Working After Upgrade

```sql
-- Check extension is loaded
SELECT * FROM pg_extension WHERE extname = 'pg_tviews';

-- Verify functions exist
SELECT proname FROM pg_proc WHERE proname LIKE 'pg_tviews_%';

-- Check TVIEW metadata
SELECT * FROM pg_tview_meta;

-- Recreate TVIEWs if needed
SELECT pg_tviews_drop(entity, true) FROM pg_tview_meta;
-- Then recreate manually
```

### Performance Issues After Upgrade

```sql
-- Check for missing indexes
SELECT schemaname, tablename, indexname
FROM pg_indexes
WHERE tablename LIKE 'tv_%' AND indexname NOT LIKE 'idx_tv_%';

-- Rebuild statistics
ANALYZE;

-- Check for query plan changes
EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM tv_table LIMIT 1;
```

## Post-Upgrade Verification

After any upgrade:

```sql
-- 1. Version check
SELECT pg_tviews_version();

-- 2. Health check
SELECT * FROM pg_tviews_health_check();

-- 3. TVIEW integrity
SELECT
    entity,
    (SELECT COUNT(*) FROM pg_class WHERE relname = 'tv_' || entity) as tv_exists,
    (SELECT COUNT(*) FROM pg_class WHERE relname = 'v_' || entity) as v_exists
FROM pg_tview_meta;

-- 4. Data consistency (spot check)
SELECT COUNT(*) FROM tv_table;
SELECT COUNT(*) FROM v_table;

-- 5. Trigger verification
SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE '%tview%';

-- 6. Performance test
EXPLAIN (ANALYZE) SELECT * FROM tv_table LIMIT 10;
```

## Emergency Procedures

### Complete Extension Reset

If everything goes wrong:

```sql
-- 1. Disconnect all users
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = current_database() AND pid != pg_backend_pid();

-- 2. Drop extension (cascades TVIEWs)
DROP EXTENSION pg_tviews CASCADE;

-- 3. Clean up any remaining objects
DROP TABLE IF EXISTS pg_tview_meta;
DROP TABLE IF EXISTS pg_tviews_metrics;
-- (Check for other extension tables)

-- 4. Restore from backup
-- pg_restore -d your_database backup_file.dump

-- 5. Reinstall and recreate TVIEWs
```

## Best Practices

1. **Always backup before upgrading**
2. **Test upgrades in staging first**
3. **Have rollback plan ready**
4. **Schedule maintenance windows**
5. **Monitor after upgrade for 24-48 hours**
6. **Keep multiple backup versions**
7. **Document custom TVIEWs for recreation**

## See Also

- [Installation Guide](../getting-started/installation.md)
- [Troubleshooting Guide](troubleshooting.md)
- [CHANGELOG.md](../../CHANGELOG.md)