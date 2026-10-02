# Backup Types for pg_tviews

## Overview

pg_tviews needs no backup mechanism of its own: everything it stores is in ordinary
PostgreSQL tables. What matters is how each PostgreSQL backup type treats two things:

- **The catalog** `tviews.pg_tview_meta` (and `tviews.pg_tview_helpers`): extension
  tables registered with `pg_extension_config_dump`, so `pg_dump` includes their rows.
- **The TVIEW tables** `tv_*`: derived data that can always be rebuilt from the base
  tables. They are UNLOGGED by default (`pg_tviews.unlogged_by_default = on`), so they
  write no WAL and are **empty** after any restore from a physical backup or WAL.

| Backup type | Registrations | TVIEW rows (UNLOGGED) | TVIEW rows (LOGGED) |
|-------------|---------------|-----------------------|---------------------|
| `pg_dump` (full) | restored | restored | restored |
| `pg_dump --schema-only` | **not restored** | empty | empty |
| `pg_basebackup` / PITR | restored | **empty, rebuild** | restored |

## 1. Logical Backups (pg_dump)

### Full Database Backup
```bash
pg_dump --format=custom --file=/backups/mydb-$(date +%Y%m%d_%H%M%S).dump mydb
pg_restore --list /backups/mydb-*.dump | grep 'tviews pg_tview_meta'
```

Parallel, directory format:
```bash
pg_dump --format=directory --jobs=4 --file=/backups/mydb-$(date +%Y%m%d_%H%M%S) mydb
```

### Restore
```bash
createdb mydb_restored
pg_restore --exit-on-error --dbname=mydb_restored /backups/mydb-20251213.dump
psql -X -d mydb_restored -c "SELECT schema, name FROM tviews.registry;"
```
The restore target must have the same pg_tviews release (and `jsonb_delta`, when used)
installed on the server. After the restore the TVIEWs are registered, point at the
restored relations and keep propagating; this round trip is tested in
`test/sql/regress_issue_96_dump_restore.sql`. See
[Full Database Restore](../recovery-procedures/full-database-restore.md).

To keep dumps smaller you may skip the data of UNLOGGED tables with
`--no-unlogged-table-data`; then rebuild the TVIEWs after the restore with
`SELECT * FROM tviews.pg_tviews_rebuild_all();`.

### Schema-only dumps
`pg_dump --schema-only` does not include the rows of `tviews.pg_tview_meta`. A database
restored from it has the `tv_*` tables and the pg_tviews triggers on base tables, but
no registered TVIEW (`tviews.pg_tviews_health_check()` reports orphaned triggers).
Do not use schema-only dumps to copy TVIEWs; recreate them from your schema
migrations instead.

### Partial restores
Do not restore a single `tv_*` table on its own. Restore the base tables, then rebuild
the TVIEW from its view:
```sql
SELECT tviews.pg_tviews_refresh('post');
```

## 2. Physical Backups (pg_basebackup)

```bash
pg_basebackup --pgdata=/backups/base-$(date +%Y%m%d_%H%M%S) \
    --format=tar --gzip --checkpoint=fast --wal-method=stream
```

A base backup restores to the same PostgreSQL major version and the same pg_tviews
library must be installed. UNLOGGED TVIEW tables are not in the backup: after the
restore they are empty. Check and rebuild:
```sql
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all();
```
With `pg_tviews.auto_rebuild_databases` set in `postgresql.conf`, the rebuild runs when
the server leaves recovery. Make a TVIEW LOGGED
(`SELECT tviews.pg_tviews_set_logged('post', true);`) if it must be complete right after
a physical restore or failover. See [Replication](../../replication.md).

## 3. WAL Archiving (Point-in-Time Recovery)

Standard PostgreSQL setup (`wal_level = replica`, `archive_mode = on`,
`archive_command`). On recovery, set `restore_command` and `recovery_target_time` in
`postgresql.conf` and create `recovery.signal` in the data directory
(`recovery.conf` was removed in PostgreSQL 12).

Writes to UNLOGGED TVIEWs are not in the WAL; LOGGED TVIEWs are recovered like any table.
After recovery, rebuild emptied UNLOGGED TVIEWs as in section 2.

## 4. TVIEW Definitions

TVIEW definitions belong in your schema migrations. For a reference copy of what is
currently registered:
```sql
\copy (SELECT schema, name, entity, query, options FROM tviews.registry ORDER BY schema, name) TO 'tview-definitions.csv' WITH (FORMAT csv, HEADER)
```
`query` is the SELECT the TVIEW was created from and `options` the options passed to
`tviews.pg_tviews_create_or_replace(name, query, options)`. Recreating TVIEWs from this
file must follow dependency order (a TVIEW whose query reads `v_user` comes after `user`;
see `tviews.pg_tviews_show_cascade_path(entity)`).

## Recommendations

- Small and medium databases: daily `pg_dump`, plus physical backups and WAL archiving
  if you need point-in-time recovery.
- Large databases: physical backups and WAL archiving, with periodic `pg_dump` for
  portability.
- In every case, include the rebuild of UNLOGGED TVIEWs in the restore procedure, and
  test restores ([Backup Testing](backup-testing.md)).

## Security Considerations

- Backups contain all base-table data and the TVIEW documents built from it; protect
  them as you protect the database.
- Encrypt backups at rest and restrict access to backup storage and credentials.

## Related Documentation

- [Backup Frequency](backup-frequency.md)
- [Backup Retention](backup-retention.md)
- [Backup Testing](backup-testing.md)
- [Full Database Restore](../recovery-procedures/full-database-restore.md)
- [Replication and UNLOGGED TVIEWs](../../replication.md)
