# pg_tviews Extension Update

## Scope
Updating pg_tviews from one release to a later one (for example `0.1.0-beta.20` to
`0.1.0-beta.21`) on the same PostgreSQL major version. Each release has its own
extension version and ships an upgrade script from the previous release; PostgreSQL
chains them, so `ALTER EXTENSION pg_tviews UPDATE` moves across several releases at
once. How releases and upgrade scripts work:
[Extension versioning](../../../development/extension-versioning.md).

Installs of `0.1.0` (every release up to `0.1.0-beta.19`) cannot be updated in place:
use `scripts/migrate-from-0.1.0.sql` instead of step 3 below (see the README,
"Upgrading").

## What to expect
- The library (`pg_tviews.so`) is shared by every database of the server and is
  preloaded (`shared_preload_libraries`), so installing it needs a PostgreSQL restart.
- From the restart until `ALTER EXTENSION pg_tviews UPDATE` has run in a database, the
  library refuses to work with that database's older catalog: writes to TVIEW base
  tables and `pg_tviews_*` calls fail with
  `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
  and the hint `run ALTER EXTENSION pg_tviews UPDATE`. Reads of `tv_*` tables keep
  working. Plan the window so that the update runs right after the restart.
- There are no downgrade scripts. Rolling back means reinstalling the previous package
  and restoring the backup taken before the update.

## Prerequisites
- A `pg_dump -Fc` backup of every database that has pg_tviews, taken just before
- The new release built or downloaded for your PostgreSQL major version
- The release notes (CHANGELOG) read for the versions you skip over

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
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
SELECT gid, prepared FROM pg_prepared_xacts WHERE database = current_database();
```

## Update Procedure

### Step 1: Back up
```bash
pg_dump -Fc -f /backups/mydb-before-pg_tviews-update.dump mydb
```

### Step 2: Install the new package and restart
From a release tarball (`pg_tviews-v<version>.tar.gz`):
```bash
cp lib/pg_tviews.so            "$(pg_config --pkglibdir)/"
cp extension/pg_tviews.control "$(pg_config --sharedir)/extension/"
cp extension/pg_tviews--*.sql  "$(pg_config --sharedir)/extension/"
```
Or from source: `cargo pgrx install --release --pg-config "$(which pg_config)"`
(add `--no-default-features --features pg16` or `pg17` for those versions).

Then restart PostgreSQL. Check that the new version is available:
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
The two versions must now be equal.

### Step 4: Re-register TVIEWs
An upgrade script that changes what registration derives marks every TVIEW
`needs_reregister`; upgrade scripts never re-derive metadata themselves. Re-derive it
(rows are not touched):
```sql
SELECT schema, name FROM tviews.registry WHERE needs_reregister;
SELECT * FROM tviews.pg_tviews_reregister_all();
```
Running it when nothing needs it is harmless. Each TVIEW's `status` is `reregistered`
or the error that stopped it; `pg_tviews_reregister_all(strict => true)` also raises
at the end when any TVIEW failed. One TVIEW:
`SELECT tviews.pg_tviews_reregister('tv_post');`.

### Step 5: Validate
```bash
psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/upgrade/scripts/post-upgrade-validation.sql
```
It shows the versions, the catalog comparison, remaining re-registrations, the full
health check, and compares each TVIEW with its backing view (expect 0 differing rows).
Finally, make a test write in a transaction you roll back and check that the TVIEW
follows:
```sql
BEGIN;
UPDATE public.tb_post SET title = title || ' (update check)' WHERE pk_post = 1;
SELECT data->>'title' FROM public.tv_post WHERE pk_post = 1;
ROLLBACK;
```

## Success Criteria
- [ ] `extversion` equals `tviews.pg_tviews_version()` in every database
- [ ] The health check's `catalog` component is OK and nothing needs re-registration
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
The library was updated but this database's extension was not: run Step 3 and Step 4
in it. If the hint names `scripts/migrate-from-0.1.0.sql`, the database still has a
`0.1.0` install: run that script instead.

### `extension "pg_tviews" has no update path from version "X" to version "Y"`
The upgrade scripts for some intermediate release are missing from
`$(pg_config --sharedir)/extension/`. Copy every `pg_tviews--*.sql` from the release
tarball.

### `pg_tviews_reregister_all()` shows an error as a TVIEW's status
Its definition no longer registers with the new release (see the error and the release
notes). Fix the definition and recreate it with
`tviews.pg_tviews_create_or_replace(name, query, options)`.

## Related Guides
- [Extension versioning](../../../development/extension-versioning.md)
- [PostgreSQL Minor Upgrade](../postgresql/minor-version-upgrade.md)
- [Troubleshooting Upgrades](../postgresql/troubleshooting-upgrades.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
