# Troubleshooting Upgrade Issues

## Overview
Problems seen when upgrading PostgreSQL or pg_tviews, by the message you get. Start with
the health check, which reports most of them without raising:
```sql
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
```
And compare the versions:
```sql
SELECT current_setting('server_version') AS postgresql,
       tviews.pg_tviews_version() AS library,
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension,
       tviews.pg_tviews_catalog_revision() AS catalog_revision;
```

## 1. pg_tviews Extension Issues

### `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
The library on disk is a different release from the extension catalog of this
database. Writes to TVIEW base tables and `pg_tviews_*` calls fail until it is fixed;
reads of `tv_*` tables still work. The hint says which way:

- `run ALTER EXTENSION pg_tviews UPDATE`: the library is newer. Run in this database
  ```sql
  ALTER EXTENSION pg_tviews UPDATE;
  SELECT * FROM tviews.pg_tviews_reregister_all();
  ```
- `the installed extension is newer than this library`: an older package was installed
  over a newer one. Install the package matching `extversion` and restart.
- `(0.1.0, no revision)` / `a 0.1.0 install cannot be updated in place`: the database
  has a release up to `0.1.0-beta.19`. Run `scripts/migrate-from-0.1.0.sql` from the new
  release (README, "Upgrading").

The library is shared by every database of the server: each database with pg_tviews
needs its own `ALTER EXTENSION pg_tviews UPDATE`.

### Health check warning: TVIEWs `registered by an older release`
An upgrade script marked them `needs_reregister`. Until re-registered they keep their
old triggers and may refresh more than needed. Run:
```sql
SELECT * FROM tviews.pg_tviews_reregister_all();
```
Each row's `status` is `reregistered` or the error for that TVIEW. A TVIEW that fails
needs its definition fixed for the new release (see the CHANGELOG), then
`tviews.pg_tviews_create_or_replace(name, query, options)`.

### `extension "pg_tviews" has no update path from version "X" to version "Y"`
An intermediate upgrade script is missing from `$(pg_config --sharedir)/extension/`.
Copy every `pg_tviews--*.sql` from the release tarball. Check what is installed:
```sql
SELECT name, default_version, installed_version
FROM pg_available_extensions WHERE name = 'pg_tviews';
```

### `pg_tviews requires PostgreSQL 16 or later`
`CREATE EXTENSION pg_tviews` on PostgreSQL 15 or older. Upgrade PostgreSQL first:
[Upgrading from PostgreSQL 15](pg15-to-pg16.md).

### `could not access file "pg_tviews"` at server start
`shared_preload_libraries` names pg_tviews but `pg_tviews.so` is missing for this
PostgreSQL major version. Install it into `$(pg_config --pkglibdir)` (release tarball,
or `cargo pgrx install --release --pg-config "$(which pg_config)"`).

### Health check warning: `jsonb_delta not installed`
pg_tviews works without it, more slowly. If it was installed before, install
`jsonb_delta` for the new PostgreSQL version and `CREATE EXTENSION jsonb_delta;`.

### Health check warning: orphaned triggers
pg_tviews triggers exist on base tables for TVIEWs that are not in the catalog. Typical
causes: a restore from a `--schema-only` dump (the catalog rows are not in it), or a
data restore with `--disable-triggers`. In the second case
`SELECT * FROM tviews.pg_tviews_reregister_all();` clears it; in the first, restore from
a full dump or recreate the TVIEWs.

### A TVIEW is empty after the upgrade
UNLOGGED TVIEWs (the default) are emptied by a crash or immediate shutdown, a physical
restore, or a dump taken with `--no-unlogged-table-data`.
```sql
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

### A TVIEW differs from its view after the upgrade
`docs/operations/upgrade/scripts/post-upgrade-validation.sql` lists differing rows per
TVIEW. Rebuild one from its view:
```sql
SELECT tviews.pg_tviews_refresh('post');
```
and report the case: a TVIEW must always equal its view.

## 2. PostgreSQL Upgrade Issues

### pg_upgrade `--check`: loadable libraries missing from the new installation
Install pg_tviews (the same release) and jsonb_delta built for the new major version,
and set `shared_preload_libraries` in the new cluster.

### Queries slower after a major upgrade
pg_upgrade does not carry planner statistics. Run `vacuumdb --all --analyze-in-stages`,
or in each database:
```sql
ANALYZE;
```

### pg_restore errors
Run `pg_restore --exit-on-error` and fix the first error; later ones usually follow
from it. Restore into a server that has the same pg_tviews release as the source
database. See [Full Database Restore](../../disaster-recovery/recovery-procedures/full-database-restore.md).

## 3. Rollback
There are no downgrade scripts: `ALTER EXTENSION pg_tviews UPDATE TO` an older version
fails. To go back, reinstall the previous package, restart, and restore the backup
taken before the upgrade. Do not drop the extension to "reset" it:
`DROP EXTENSION pg_tviews` removes every TVIEW registration.

## Getting Help
- PostgreSQL documentation: https://www.postgresql.org/docs/
- pg_tviews issues: https://github.com/fraiseql/pg_tviews/issues (include the output of
  `docs/operations/runbooks/scripts/health-check.sql`)

## Related Guides
- [Extension Updates](../extension/extension-minor-update.md)
- [PostgreSQL Minor Upgrade](minor-version-upgrade.md)
- [Upgrading from PostgreSQL 15](pg15-to-pg16.md)
- [Extension versioning](../../../development/extension-versioning.md)
