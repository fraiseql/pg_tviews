# Troubleshooting Upgrade Issues

## Overview
Problems seen when upgrading PostgreSQL or pg_tviews, by the message you get. Start with
the health check, which reports most of them without raising:
```sql
SELECT status, component, severity, message
FROM tviews.pg_tviews_check_health()
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

- `run ALTER EXTENSION pg_tviews UPDATE`: the library is the next release. Run in this
  database
  ```sql
  ALTER EXTENSION pg_tviews UPDATE;
  ```
- `the installed extension is newer than this library`: an older package was installed
  over a newer one. Install the package matching `extversion` and restart.
- `this release updates only from <previous>`: the database has an older release
  (`(0.1.0, no revision)` for a `0.1.0` install). Drop the extension and create the
  TVIEWs again ([Upgrading pg_tviews](../../upgrade.md#from-an-older-release)).

The library is shared by every database of the server: each database with pg_tviews
needs its own `ALTER EXTENSION pg_tviews UPDATE`.

### Health check warning: TVIEWs `registered by an older release`
They are marked `needs_reregister`: their plan and triggers were not re-derived since
the catalog changed. `ALTER EXTENSION pg_tviews UPDATE` re-derives every TVIEW, so a
successful update leaves none. Run:
```sql
SELECT * FROM tviews.pg_tviews_reregister_all();
```
Each row's `status` is `reregistered` or the error for that TVIEW. A TVIEW that fails
needs its definition fixed for the new release (see the CHANGELOG), then
`tviews.pg_tviews_create_or_replace(name, query, options)`.

### `extension "pg_tviews" has no update path from version "X" to version "Y"`
A release ships one update script, from the release before it. Either the installed
version is older: drop the extension and create the TVIEWs again
([Upgrading pg_tviews](../../upgrade.md#from-an-older-release)); or the script is
missing from `$(pg_config --sharedir)/extension/`: copy every `pg_tviews--*.sql` from
the release tarball. Check what is installed:
```sql
SELECT name, default_version, installed_version
FROM pg_available_extensions WHERE name = 'pg_tviews';
```

### `pg_tviews requires PostgreSQL 18 or later`
`CREATE EXTENSION pg_tviews` on PostgreSQL 17 or older. Upgrade PostgreSQL first:
[Upgrading to PostgreSQL 18](to-pg18.md).

### `could not access file "pg_tviews"` at server start
`shared_preload_libraries` names pg_tviews but `pg_tviews.so` is missing for this
PostgreSQL major version. Install it into `$(pg_config --pkglibdir)` (release tarball,
or `cargo pgrx install --release --pg-config "$(which pg_config)"`).

### `required extension "jsonb_delta" is not installed`
pg_tviews requires jsonb_delta: `ALTER EXTENSION pg_tviews UPDATE` and
`CREATE EXTENSION pg_tviews` refuse without it. Install `jsonb_delta` for this
PostgreSQL version and `CREATE EXTENSION jsonb_delta;`, then retry.

### `ALTER EXTENSION pg_tviews UPDATE` fails naming a TVIEW
The update re-derives every TVIEW; this one no longer analyses with the new release
(see the error and the CHANGELOG). Nothing was changed: fix or drop it, then update
again.

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
SELECT * FROM tviews.pg_tviews_show_replication();
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
- [Upgrading to PostgreSQL 18](to-pg18.md)
- [Extension versioning](../../../development/extension-versioning.md)
