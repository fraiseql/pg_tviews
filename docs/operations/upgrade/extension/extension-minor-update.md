# pg_tviews Extension Update

## Scope
Updating pg_tviews to the next release on the same PostgreSQL major version. A release
updates only from the release before it (`0.1.0-beta.34` to `0.1.0-beta.35`), with one
script, `pg_tviews--<previous>--<current>.sql`. An install of an older release, `0.1.0`
installs included, is dropped and its TVIEWs created again:
[Upgrading pg_tviews](../../upgrade.md#from-an-older-release). How releases and the
update script work: [Extension versioning](../../../development/extension-versioning.md).

## What to expect
- The library (`pg_tviews.so`) is shared by every database of the server and is
  preloaded (`shared_preload_libraries`), so installing it needs a PostgreSQL restart.
- From the restart until `ALTER EXTENSION pg_tviews UPDATE` has run in a database, the
  library refuses to work with that database's older catalog: writes to TVIEW base
  tables and `pg_tviews_*` calls fail with SQLSTATE `55000`,
  `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
  and the hint `run ALTER EXTENSION pg_tviews UPDATE`. A function the release removed
  raises the same error. Reads of `tv_*` tables keep working. Pause the writers for the
  window.
- The update re-derives every TVIEW's plan and triggers from its definition, as the
  TVIEW's owner, without touching its rows. A TVIEW that no longer analyses fails the
  whole update and nothing is changed. It takes about 50 ms for 1 TVIEW, 1.2 s for 50,
  2.2 s for 200.
- There are no downgrade scripts. Rolling back means reinstalling the previous package
  and restoring the backup taken before the update.

## Prerequisites
- A `pg_dump -Fc` backup of every database that has pg_tviews, taken just before
- The new release built or downloaded for your PostgreSQL major version
- jsonb_delta installed in each database: the update refuses without it
  (`required extension "jsonb_delta" is not installed`)
- The release notes (CHANGELOG) read, for the grants to restore

## Pre-Update Checks
In each database that has pg_tviews:
```bash
PGDATABASE=mydb docs/operations/upgrade/scripts/pre-upgrade-checks.sh
```
It checks the PostgreSQL version (16, 17 or 18), the installed and available
extension versions, health-check errors and prepared transactions. The essentials by
hand:
```sql
SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';
SELECT tviews.pg_tviews_version();
SELECT status, component, severity, message
FROM tviews.pg_tviews_check_health()
WHERE severity <> 'info';
SELECT gid, prepared FROM pg_prepared_xacts WHERE database = current_database();
```

## Update Procedure

### Step 1: Back up and pause the writers
```bash
pg_dump -Fc -f /backups/mydb-before-pg_tviews-update.dump mydb
```
Stop the application, workers and scheduled jobs that write. Readers may stay.

### Step 2: Install the new package and restart
From a release tarball (`pg_tviews-v<version>.tar.gz`):
```bash
cp lib/pg_tviews.so            "$(pg_config --pkglibdir)/"
cp extension/pg_tviews.control "$(pg_config --sharedir)/extension/"
cp extension/pg_tviews--*.sql  "$(pg_config --sharedir)/extension/"
```
Or from source: `cargo pgrx install --release --pg-config "$(which pg_config)"`
(add `--no-default-features --features pg16` or `pg17` for those versions).

Stop PostgreSQL before overwriting the library (overwriting a loaded library crashes
its backends), then start it. Check that the new version is available:
```sql
SELECT name, default_version, installed_version
FROM pg_available_extensions WHERE name = 'pg_tviews';
```

### Step 3: Update the extension, in each database
```sql
ALTER EXTENSION pg_tviews UPDATE;
SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';
SELECT tviews.pg_tviews_version();
```
The two versions must now be equal. Restore the grants on functions the release
replaced (the release notes list them).

### Step 4: Validate, then resume the writers
```bash
psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/upgrade/scripts/post-upgrade-validation.sql
```
It shows the versions, the catalog comparison, the full health check, and compares
each TVIEW with its backing view (expect 0 differing rows). Make a test write in a
transaction you roll back and check that the TVIEW follows:
```sql
BEGIN;
UPDATE public.tb_post SET title = title || ' (update check)' WHERE pk_post = 1;
SELECT data->>'title' FROM public.tv_post WHERE pk_post = 1;
ROLLBACK;
```

## Success Criteria
- [ ] `extversion` equals `tviews.pg_tviews_version()` in every database
- [ ] The health check's `catalog` component is OK
- [ ] No health-check warning or error
- [ ] TVIEWs equal their views; a test write propagates

## Rollback
1. Stop the applications.
2. Reinstall the previous release's package and restart PostgreSQL.
3. Restore each database from the backup of Step 1 into a fresh database
   ([Full Database Restore](../../disaster-recovery/recovery-procedures/full-database-restore.md)),
   then swap it in.

## Troubleshooting

### `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
The library was updated but this database's extension was not: run Step 3 in it. If
the hint says `this release updates only from <previous>`, the database has an older
release: drop and recreate ([Upgrading pg_tviews](../../upgrade.md#from-an-older-release)).

### `extension "pg_tviews" has no update path from version "X" to version "Y"`
The installed version is not the previous release: drop and recreate, as above.

### `required extension "jsonb_delta" is not installed`
Install jsonb_delta in the database (`CREATE EXTENSION jsonb_delta;`), then update.

### The update fails naming a TVIEW
That TVIEW's definition no longer registers with the new release (see the error and
the release notes). Nothing was changed: fix or drop it, then run
`ALTER EXTENSION pg_tviews UPDATE` again.

## Related Guides
- [Upgrading pg_tviews](../../upgrade.md)
- [Extension versioning](../../../development/extension-versioning.md)
- [PostgreSQL Minor Upgrade](../postgresql/minor-version-upgrade.md)
- [Troubleshooting Upgrades](../postgresql/troubleshooting-upgrades.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
