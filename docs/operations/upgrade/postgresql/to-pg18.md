# Upgrading to PostgreSQL 18

## Scope
pg_tviews supports PostgreSQL 18 only; on an older server
`CREATE EXTENSION pg_tviews` fails with `pg_tviews requires PostgreSQL 18 or later`.
Releases up to `0.1.0-beta.37` also run on PostgreSQL 16 and 17 (and up to
`0.1.0-beta.20` on 15). A database that runs pg_tviews on one of those majors must move
to PostgreSQL 18 **before** the current pg_tviews release can be installed.

## Approach
Change one thing at a time:

1. Move PostgreSQL to 18, keeping the pg_tviews release you run now.
2. Then move pg_tviews to the current release on PostgreSQL 18.

Doing both at once means restoring a catalog written by one release into another, and
leaves no clean rollback point.

## Prerequisites
- A verified backup (`pg_dump -Fc` of each database)
- PostgreSQL 18 installed next to the old major
- **The pg_tviews release you run now, built for PostgreSQL 18**, installed into the
  18 installation (`cargo pgrx install --release --pg-config /usr/lib/postgresql/18/bin/pg_config`
  from that release's tag), plus `jsonb_delta` for 18
- `shared_preload_libraries = 'pg_tviews'` in the new cluster's `postgresql.conf`
- A test run of the whole procedure on a copy of production

## Pre-Upgrade Checks
On the old server, in each database with pg_tviews:
```sql
SELECT version();
SELECT extversion, extnamespace::regnamespace AS schema
FROM pg_extension WHERE extname = 'pg_tviews';
SELECT * FROM pg_tviews_check_health();
```
Note `extversion`. `0.1.0` means a release up to `0.1.0-beta.19`, installed in the
schema shown (its functions are not in `tviews`); later releases live in schema
`tviews`, so qualify the call as `tviews.pg_tviews_check_health()` there.

## Method 1: pg_upgrade
pg_upgrade copies the data files and keeps object OIDs, so the extension, its catalog
and the TVIEWs come over as they are.

```bash
# On a stopped old cluster (here 17) and a fresh, stopped PostgreSQL 18 cluster
/usr/lib/postgresql/18/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/17/main \
    --new-datadir=/var/lib/postgresql/18/main \
    --old-bindir=/usr/lib/postgresql/17/bin \
    --new-bindir=/usr/lib/postgresql/18/bin \
    --check
```
`--check` fails with `Your installation references loadable libraries that are missing
from the new installation` when pg_tviews (or jsonb_delta) is not installed for 18.
Run without `--check` once it passes, start the 18 cluster, then run the validation
below. If an UNLOGGED TVIEW comes back empty, rebuild it (validation step 3).

## Method 2: pg_dump / pg_restore
```bash
pg_dump -Fc -h old-host -f /backups/mydb.dump mydb          # with the 18 client
createdb -h new-host mydb
pg_restore -h new-host --exit-on-error --dbname=mydb /backups/mydb.dump
```
The dump carries the TVIEW registrations from `0.1.0-beta.19` on (the catalog is
dumped with `pg_extension_config_dump`). With an older release, the restored TVIEWs
are not registered: use pg_upgrade, or first update pg_tviews on the old server to
`0.1.0-beta.19` or later.

## Validation
On PostgreSQL 18, in each database (here for a release installed in schema `tviews`;
for a `0.1.0` install use the functions in its own schema):

1. Versions and health:
   ```sql
   SELECT version();
   SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';
   SELECT status, component, severity, message
   FROM tviews.pg_tviews_check_health()
   WHERE severity <> 'info';
   ```
2. TVIEWs and their contents:
   ```bash
   psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/runbooks/scripts/health-check.sql
   ```
3. Emptied UNLOGGED TVIEWs:
   ```sql
   SELECT * FROM tviews.pg_tviews_show_replication();
   SELECT * FROM tviews.pg_tviews_rebuild_all();
   ```
4. A write propagates:
   ```sql
   BEGIN;
   UPDATE public.tb_post SET title = title || ' (upgrade check)' WHERE pk_post = 1;
   SELECT data->>'title' FROM public.tv_post WHERE pk_post = 1;
   ROLLBACK;
   ```
5. Statistics: pg_upgrade does not carry planner statistics over. Run
   `vacuumdb --all --analyze-in-stages`.

## Then update pg_tviews
With PostgreSQL 18 running:

- The installed release is the one before the current release:
  [Extension Updates](../extension/extension-minor-update.md)
  (`ALTER EXTENSION pg_tviews UPDATE`).
- An older release: drop the extension,
  install the current release and create the TVIEWs again
  ([Upgrading pg_tviews](../../upgrade.md#from-an-older-release)).

## Rollback
- pg_upgrade (without `--link`): the old cluster is untouched until you start 18 and
  remove it; start the old cluster again.
- pg_upgrade with `--link`: once the 18 cluster has started, the old cluster can no
  longer be used safely; restore from backup.
- pg_dump / pg_restore: the source server is untouched; point applications back to it.

## Related Guides
- [PostgreSQL Minor Upgrade](minor-version-upgrade.md)
- [Extension Updates](../extension/extension-minor-update.md)
- [Extension versioning](../../../development/extension-versioning.md)
- [Troubleshooting Upgrades](troubleshooting-upgrades.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
