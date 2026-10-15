# Backup Testing Procedures

## Overview

A backup is only proven by restoring it. This page lists checks for backups of a
database that uses pg_tviews. What pg_tviews adds to an ordinary PostgreSQL restore test:

- **Logical backups** (`pg_dump`): the catalog table `tviews.pg_tview_meta` is dumped
  (it is registered with `pg_extension_config_dump`), so after `pg_restore` the TVIEWs
  come back registered and keep propagating. Covered by
  `test/sql/regress/catalog/regress_dump_restore.sql`.
- **Physical backups** (`pg_basebackup`, PITR) and crash restarts: UNLOGGED TVIEW tables
  (declared `logged: false`) have no WAL and no data in a base backup. They come back **empty** and
  must be rebuilt with `tviews.pg_tviews_rebuild_all()` (or automatically, see
  [Replication](../../replication.md)).
- The restore target needs the same pg_tviews release and the `jsonb_delta` extension
  installed (when the source database uses it).

## Testing Frequency

- **Daily**: the latest dump exists and `pg_restore --list` reads it
- **Weekly**: full restore into a scratch database and the checks below
- **Before upgrades** of PostgreSQL or pg_tviews, and after TVIEW changes

## Daily Integrity Check

```bash
#!/bin/bash
set -euo pipefail
BACKUP_DIR=/backups

latest=$(find "$BACKUP_DIR" -name '*.dump' -mtime -1 | sort | tail -1)
if [ -z "$latest" ]; then
    echo "FAIL: no dump from the last 24 hours in $BACKUP_DIR"; exit 1
fi

pg_restore --list "$latest" > /dev/null || { echo "FAIL: unreadable dump $latest"; exit 1; }

# The pg_tviews catalog rows are in the dump as TABLE DATA of tviews.pg_tview_meta
if ! pg_restore --list "$latest" | grep -q 'TABLE DATA tviews pg_tview_meta'; then
    echo "FAIL: $latest has no pg_tview_meta data"; exit 1
fi
echo "OK: $latest"
```

## Weekly Restore Test

### Step 1: Restore into a scratch database
```bash
TEST_DB="restore_test_$(date +%Y%m%d_%H%M%S)"
BACKUP_FILE=$(ls -t /backups/*.dump | head -1)

createdb "$TEST_DB"
time pg_restore --exit-on-error --dbname="$TEST_DB" "$BACKUP_FILE"
```

### Step 2: Validate pg_tviews in the restored database
```bash
psql -X -v ON_ERROR_STOP=1 -d "$TEST_DB" -f docs/operations/runbooks/scripts/health-check.sql
```
The key checks, if you prefer to run them by hand:
```sql
-- No component in error or warning
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';

-- Every TVIEW is registered and points at existing relations
SELECT schema, name, view, base_tables, needs_reregister
FROM tviews.registry
ORDER BY schema, name;

-- Row counts per TVIEW
SELECT format('SELECT %L AS tview, count(*) AS rows FROM %I.%I',
              schema || '.' || name, schema, name)
FROM tviews.registry
ORDER BY schema, name
\gexec
```

### Step 3: Check that TVIEWs equal their views
A TVIEW must equal its backing view. For each TVIEW (here `post`):
```sql
SELECT count(*) AS differing_rows
FROM (
    (SELECT pk_post, data FROM public.v_post EXCEPT SELECT pk_post, data FROM public.tv_post)
    UNION ALL
    (SELECT pk_post, data FROM public.tv_post EXCEPT SELECT pk_post, data FROM public.v_post)
) d;
```

### Step 4: Check that propagation still works
Write to a base table inside a transaction that you roll back, and check that the TVIEW
followed:
```sql
BEGIN;
UPDATE public.tb_post SET title = title || ' (restore test)' WHERE pk_post = 1;
SELECT data->>'title' FROM public.tv_post WHERE pk_post = 1;  -- shows the new title
ROLLBACK;
```

### Step 5: Clean up
```bash
dropdb "$TEST_DB"
```

## Physical Backup / PITR Test

Restore the base backup and WAL to a spare data directory and port, following the
PostgreSQL documentation (`recovery.signal`, `restore_command`, `recovery_target_time`
in `postgresql.conf`; `recovery.conf` no longer exists since PostgreSQL 12). Once the
restored server is out of recovery:
```sql
-- UNLOGGED TVIEWs are empty after a physical restore
SELECT * FROM tviews.pg_tviews_replication_status();

-- Rebuild the emptied ones, then rerun Steps 2-4 above
SELECT * FROM tviews.pg_tviews_rebuild_all();
```
With `shared_preload_libraries = 'pg_tviews'`, the rebuild launcher runs it in every
database when recovery ends (`pg_tviews.auto_rebuild_databases`, default `*`); `pg_tviews_replication_status()` then shows `needs_rebuild = false`.

## Recording Results

Record date, backup file, restore duration and the outcome of Steps 2-4 in your usual
operations log. pg_tviews does not keep backup or test history.

## Related Documentation

- [Backup Types](../backup-strategy/backup-types.md)
- [Backup Frequency](../backup-strategy/backup-frequency.md)
- [Backup Retention](../backup-strategy/backup-retention.md)
- [Full Database Restore](../recovery-procedures/full-database-restore.md)
- [Replication and UNLOGGED TVIEWs](../../replication.md)
