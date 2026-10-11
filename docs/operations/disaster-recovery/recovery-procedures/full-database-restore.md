# Full Database Restore Procedure

## Purpose
Restore a PostgreSQL database that uses pg_tviews from backup, after data loss,
corruption, or to move it to new infrastructure.

What is specific to pg_tviews:

- A `pg_dump` contains the pg_tviews catalog (`tviews.pg_tview_meta`), so a
  `pg_restore` gives back registered TVIEWs that keep propagating. There is nothing to
  recreate by hand.
- A physical restore (base backup, PITR) leaves UNLOGGED TVIEWs (declared `logged: false`;
  TVIEWs are LOGGED by default) empty;
  they are rebuilt from their views with `tviews.pg_tviews_rebuild_all()`.

## Prerequisites
- A tested backup ([Backup Testing](../backup-strategy/backup-testing.md))
- Target server with the same PostgreSQL major version (for a physical restore) and the
  same pg_tviews release installed (`pg_tviews.control`, `pg_tviews--*.sql`,
  `pg_tviews.so`), plus `jsonb_delta` if the source database uses it
- `shared_preload_libraries = 'pg_tviews'` in `postgresql.conf` on the target
- Superuser access and enough disk space for the restored database

## Pre-Restore Preparation

### Step 1: Check the backup
```bash
BACKUP_FILE=/backups/mydb-recent.dump
ls -lh "$BACKUP_FILE"
pg_restore --list "$BACKUP_FILE" | grep 'tviews pg_tview_meta'
```

### Step 2: Check the target server
```bash
psql -X -d postgres -c "SELECT version();"
psql -X -d postgres -c "SELECT name, default_version FROM pg_available_extensions WHERE name IN ('pg_tviews', 'jsonb_delta');"
psql -X -d postgres -c "SHOW shared_preload_libraries;"
```
`default_version` of `pg_tviews` must match the release the backup was taken with (see
`extversion` in the source database's `pg_extension`). To restore into a newer release,
restore into the old release first, then follow
[Extension Updates](../../upgrade/extension/extension-minor-update.md).

### Step 3: Stop writers and create the target database
```bash
TARGET_DB=mydb_restored
createdb "$TARGET_DB"
```

## Logical Restore (pg_restore)

### Step 1: Restore
```bash
RESTORE_LOG=/var/log/pg_restore_$(date +%Y%m%d_%H%M%S).log
pg_restore --exit-on-error --jobs=4 --dbname="$TARGET_DB" "$BACKUP_FILE" 2>&1 | tee "$RESTORE_LOG"
```
Run it as one `pg_restore` (or by `--section`), without `--disable-triggers`. With
`--disable-triggers` the health check afterwards reports orphaned triggers until you run
`SELECT * FROM tviews.pg_tviews_reregister_all();`.

### Step 2: If the dump skipped UNLOGGED data
When the dump was taken with `--no-unlogged-table-data`, the TVIEW tables are empty:
```sql
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

## Physical Restore (base backup and WAL)

### Step 1: Restore the data directory
Follow the PostgreSQL documentation for your backup tool: stop the server, move the old
data directory aside, extract the base backup, and for point-in-time recovery set
`restore_command` (and `recovery_target_time`) in `postgresql.conf` and create
`recovery.signal` in the data directory. `recovery.conf` no longer exists since
PostgreSQL 12.

### Step 2: Start and wait for recovery to end
```bash
psql -X -d mydb -c "SELECT pg_is_in_recovery();"   # false once recovery ended
```

### Step 3: Rebuild UNLOGGED TVIEWs
```sql
SELECT * FROM tviews.pg_tviews_replication_status();  -- is_empty / needs_rebuild
SELECT * FROM tviews.pg_tviews_rebuild_all();
```
With `shared_preload_libraries = 'pg_tviews'`, the rebuild launcher runs this in every
database when recovery ends (unless `pg_tviews.auto_rebuild_databases` leaves the
database out); check `needs_rebuild = false` instead. Until the
rebuild, readers of a reset UNLOGGED TVIEW see an empty table. After a `pg_dump`
restore every UNLOGGED TVIEW needs one rebuild, even one restored with its rows:
which TVIEWs can be trusted is not dumped. See
[Replication](../../replication.md).

## Post-Restore Validation

### Step 1: pg_tviews health
```bash
psql -X -v ON_ERROR_STOP=1 -d "$TARGET_DB" -f docs/operations/runbooks/scripts/health-check.sql
```
Or the essentials:
```sql
SELECT tviews.pg_tviews_version() AS library,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;

SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';

SELECT schema, name, view, base_tables, needs_reregister
FROM tviews.registry
ORDER BY schema, name;
```

### Step 2: TVIEW contents
A TVIEW must equal its view. For each TVIEW (here `post`):
```sql
SELECT count(*) AS differing_rows
FROM (
    (SELECT pk_post, data FROM public.v_post EXCEPT SELECT pk_post, data FROM public.tv_post)
    UNION ALL
    (SELECT pk_post, data FROM public.tv_post EXCEPT SELECT pk_post, data FROM public.v_post)
) d;
```
If a TVIEW differs, rebuild it from its view:
```sql
SELECT tviews.pg_tviews_refresh('post');
```

### Step 3: Propagation
```sql
BEGIN;
UPDATE public.tb_post SET title = title || ' (restore check)' WHERE pk_post = 1;
SELECT data->>'title' FROM public.tv_post WHERE pk_post = 1;  -- shows the new title
ROLLBACK;
```

### Step 4: Statistics
```sql
ANALYZE;
```

### Step 5: Applications
Point the applications at the restored database and run their health checks.

## Success Criteria
- [ ] PostgreSQL is out of recovery and accepts connections
- [ ] `tviews.pg_tviews_health_check()` reports no warning or error
- [ ] Every expected TVIEW is in `tviews.registry`, none is empty unexpectedly
- [ ] TVIEWs equal their views; a test write propagates
- [ ] Applications work

## Rollback
For a logical restore into a new database, the original database is untouched: drop
the restored one (`dropdb "$TARGET_DB"`) and point applications back. For a physical
restore, stop the server and move the saved data directory back.

## Troubleshooting

### `could not open extension control file ".../pg_tviews.control"`
pg_tviews is not installed on the target server. Install the release the backup was
taken with (see `docs/development/extension-versioning.md`, "Release tarball").

### `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
The installed library is a different release from the restored extension catalog. Run
`ALTER EXTENSION pg_tviews UPDATE;` then `SELECT * FROM tviews.pg_tviews_reregister_all();`
(see [Extension Updates](../../upgrade/extension/extension-minor-update.md)).

### A TVIEW is empty after the restore
It is UNLOGGED and the restore did not carry its rows (physical restore, crash, or
`--no-unlogged-table-data`). Run `SELECT * FROM tviews.pg_tviews_rebuild_all();`.

### A TVIEW is not in `tviews.registry`
The restore did not include the rows of `tviews.pg_tview_meta`, for example a
`--schema-only` dump. Restore from a full dump, or
recreate the TVIEW from your schema migrations.

## Related Documentation
- [Backup Types](../backup-strategy/backup-types.md)
- [Backup Testing](../backup-strategy/backup-testing.md)
- [Replication and UNLOGGED TVIEWs](../../replication.md)
