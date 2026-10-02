# Upgrade & Migration Guide

Short version of the upgrade procedures. Step-by-step guides are in
[upgrade/](upgrade/README.md); how releases and upgrade scripts are built is in
[Extension versioning](../development/extension-versioning.md).

## Pre-Upgrade Checklist

```bash
# 1. Back up each database with pg_tviews (the dump includes the TVIEW registrations)
pg_dump -Fc -f backup_$(date +%Y%m%d_%H%M%S).dump your_database

# 2. Check versions, health and blockers
PGDATABASE=your_database docs/operations/upgrade/scripts/pre-upgrade-checks.sh

# 3. Read the CHANGELOG for every release you skip
# 4. Rehearse on a copy of production
```

Versions by hand:
```sql
SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';  -- catalog in this database
SELECT tviews.pg_tviews_version();                                -- library loaded by the server
```

## Upgrading pg_tviews

Each release has its own extension version and ships an upgrade script from the
previous one.

1. Install the new package and restart PostgreSQL (the library is preloaded).
2. In each database with pg_tviews:
   ```sql
   ALTER EXTENSION pg_tviews UPDATE;
   SELECT * FROM tviews.pg_tviews_reregister_all();
   ```
   `pg_tviews_reregister_all()` re-derives each TVIEW's metadata and triggers from its
   definition, without touching its rows. It is needed when the release notes say so or
   the health check reports TVIEWs to re-register, and harmless otherwise.
3. Validate:
   ```bash
   psql -X -v ON_ERROR_STOP=1 -d your_database -f docs/operations/upgrade/scripts/post-upgrade-validation.sql
   ```

Between steps 1 and 2, writes to the TVIEWs' base tables fail with
`pg_tviews library catalog revision … does not match the installed extension`: they are
never served by a mismatched library.

Installs of `0.1.0` (every release up to 0.1.0-beta.19) cannot be updated in place.
After step 1, run `scripts/migrate-from-0.1.0.sql` in each database instead: it moves the
extension to the schema `tviews` and re-registers every TVIEW, keeping their rows (not
the audit log).

Details: [Extension Updates](upgrade/extension/extension-minor-update.md).

## Upgrading PostgreSQL

- Minor versions: install and restart; nothing to do in the databases.
  [Minor Version Upgrade](upgrade/postgresql/minor-version-upgrade.md).
- Major versions: pg_upgrade or pg_dump/pg_restore with the same pg_tviews release
  installed for the new major version, then update pg_tviews. pg_tviews supports
  PostgreSQL 16, 17 and 18; a server on 15 moves first
  ([Upgrading from PostgreSQL 15](upgrade/postgresql/pg15-to-pg16.md)).

After any restart that was not a clean shutdown, or a physical restore, UNLOGGED TVIEWs
are empty:
```sql
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

## Rollback

There are no downgrade scripts. To roll back pg_tviews: stop the applications,
reinstall the previous package, restart PostgreSQL, and restore the backup into a new
database ([Full Database Restore](disaster-recovery/recovery-procedures/full-database-restore.md)).

Never use `DROP EXTENSION pg_tviews` as part of an upgrade or a rollback: it removes
every TVIEW registration.

## Post-Upgrade Verification

```sql
-- 1. Library and catalog agree
SELECT tviews.pg_tviews_version(),
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;

-- 2. Health check: nothing above info
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';

-- 3. Registered TVIEWs, none waiting for re-registration
SELECT schema, name, view, needs_reregister
FROM tviews.registry
ORDER BY schema, name;

-- 4. Triggers installed on base tables
SELECT tgrelid::regclass AS base_table, count(*) AS trigger_count
FROM pg_trigger
WHERE tgname LIKE 'trg_tview_%'
GROUP BY tgrelid
ORDER BY 1;
```
`post-upgrade-validation.sql` also compares each TVIEW with its view.

## Troubleshooting

See [Troubleshooting Upgrades](upgrade/postgresql/troubleshooting-upgrades.md).

## See Also

- [Installation Guide](../getting-started/installation.md)
- [Troubleshooting Guide](troubleshooting.md)
- [CHANGELOG.md](../../CHANGELOG.md)
