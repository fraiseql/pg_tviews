# Upgrading from PostgreSQL 15 to 16

## Scope
pg_tviews supports PostgreSQL 16, 17 and 18; on an older server
`CREATE EXTENSION pg_tviews` fails with `pg_tviews requires PostgreSQL 16 or later`.
A database that runs pg_tviews on PostgreSQL 15 (with a release up to `0.1.0-beta.20`)
must move to PostgreSQL 16 or later **before** pg_tviews can be updated to a current
release. The same steps apply to 15 to 17 or 15 to 18.

## Approach
Change one thing at a time:

1. Move PostgreSQL from 15 to 16, keeping the pg_tviews release you run now.
2. Then update pg_tviews on PostgreSQL 16.

Doing both at once means restoring a catalog written by one release into another, and
leaves no clean rollback point.

## Prerequisites
- A verified backup (`pg_dump -Fc` of each database)
- PostgreSQL 16 installed next to 15
- **The pg_tviews release you run now, built for PostgreSQL 16**, installed into the
  16 installation (`cargo pgrx install --release --pg-config /usr/lib/postgresql/16/bin/pg_config --no-default-features --features pg16`
  from that release's tag), plus `jsonb_delta` for 16 if you use it
- `shared_preload_libraries = 'pg_tviews'` in the new cluster's `postgresql.conf`
- A test run of the whole procedure on a copy of production

## Pre-Upgrade Checks
On the PostgreSQL 15 server, in each database with pg_tviews:
```sql
SELECT version();
SELECT extversion, extnamespace::regnamespace AS schema
FROM pg_extension WHERE extname = 'pg_tviews';
SELECT * FROM pg_tviews_health_check();
```
Note `extversion`. `0.1.0` means a release up to `0.1.0-beta.19`, installed in the
schema shown (its functions are not in `tviews`); later releases live in schema
`tviews`, so qualify the call as `tviews.pg_tviews_health_check()` there.

## Method 1: pg_upgrade
pg_upgrade copies the data files and keeps object OIDs, so the extension, its catalog
and the TVIEWs come over as they are.

```bash
# On a stopped PostgreSQL 15 cluster and a fresh, stopped PostgreSQL 16 cluster
/usr/lib/postgresql/16/bin/pg_upgrade \
    --old-datadir=/var/lib/postgresql/15/main \
    --new-datadir=/var/lib/postgresql/16/main \
    --old-bindir=/usr/lib/postgresql/15/bin \
    --new-bindir=/usr/lib/postgresql/16/bin \
    --check
```
`--check` fails with `Your installation references loadable libraries that are missing
from the new installation` when pg_tviews (or jsonb_delta) is not installed for 16.
Run without `--check` once it passes, start the 16 cluster, then run the validation
below. If an UNLOGGED TVIEW comes back empty, rebuild it (validation step 3).

## Method 2: pg_dump / pg_restore
```bash
pg_dump -Fc -h old-host -f /backups/mydb-pg15.dump mydb          # with the 15 client or newer
createdb -h new-host mydb
pg_restore -h new-host --exit-on-error --dbname=mydb /backups/mydb-pg15.dump
```
The dump carries the TVIEW registrations from `0.1.0-beta.19` on (the catalog is
dumped with `pg_extension_config_dump`). With an older release, the restored TVIEWs
are not registered: use pg_upgrade, or first update pg_tviews on PostgreSQL 15 to
`0.1.0-beta.19` or `0.1.0-beta.20`.

## Validation
On PostgreSQL 16, in each database (here for a release installed in schema `tviews`;
for a `0.1.0` install use the functions in its own schema):

1. Versions and health:
   ```sql
   SELECT version();
   SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';
   SELECT status, component, severity, message
   FROM tviews.pg_tviews_health_check()
   WHERE severity <> 'info';
   ```
2. TVIEWs and their contents:
   ```bash
   psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/runbooks/scripts/health-check.sql
   ```
3. Emptied UNLOGGED TVIEWs:
   ```sql
   SELECT * FROM tviews.pg_tviews_replication_status();
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
With PostgreSQL 16 running:

- `extversion` = `0.1.0` (releases up to `0.1.0-beta.19`): install the current release
  and run `scripts/migrate-from-0.1.0.sql` in each database (README, "Upgrading").
- Later releases: [Extension Updates](../extension/extension-minor-update.md)
  (`ALTER EXTENSION pg_tviews UPDATE`, then `SELECT * FROM tviews.pg_tviews_reregister_all();`).

## Rollback
- pg_upgrade (without `--link`): the 15 cluster is untouched until you start 16 and
  remove it; start 15 again.
- pg_upgrade with `--link`: once the 16 cluster has started, the 15 cluster can no
  longer be used safely; restore from backup.
- pg_dump / pg_restore: the source server is untouched; point applications back to it.

## Related Guides
- [PostgreSQL Minor Upgrade](minor-version-upgrade.md)
- [Extension Updates](../extension/extension-minor-update.md)
- [Extension versioning](../../../development/extension-versioning.md)
- [Troubleshooting Upgrades](troubleshooting-upgrades.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
