# Upgrade & Migration Guide

Short version of the upgrade procedures. Step-by-step guides are in
[upgrade/](upgrade/README.md); how releases and the update script are built is in
[Extension versioning](../development/extension-versioning.md).

## Pre-Upgrade Checklist

```bash
# 1. Back up each database with pg_tviews (the dump includes the TVIEW registrations)
pg_dump -Fc -f backup_$(date +%Y%m%d_%H%M%S).dump your_database

# 2. Check versions, health and blockers
PGDATABASE=your_database docs/operations/upgrade/scripts/pre-upgrade-checks.sh

# 3. Read the CHANGELOG of the release you install
# 4. Rehearse on a copy of production
```

Versions by hand:
```sql
SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';  -- catalog in this database
SELECT tviews.pg_tviews_version();                                -- library loaded by the server
```

## Upgrading pg_tviews

A release updates from the release before it, with one `ALTER EXTENSION pg_tviews
UPDATE` per database after the package swap and restart. The update re-derives every
TVIEW inside it; between the restart and the update, writes to the TVIEWs' base tables
fail with SQLSTATE `55000`. An install of an older release is dropped and its TVIEWs
created again; the base tables keep their rows. The procedures:
[Upgrading pg_tviews](upgrade.md).

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
SELECT * FROM tviews.pg_tviews_show_replication();
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

## Rollback

There are no downgrade scripts. To roll back pg_tviews: stop the applications,
reinstall the previous package, restart PostgreSQL, and restore the backup into a new
database ([Full Database Restore](disaster-recovery/recovery-procedures/full-database-restore.md)).

`DROP EXTENSION pg_tviews` removes every TVIEW registration: use it only to move an
install of an older release ([Upgrading pg_tviews](upgrade.md#from-an-older-release)).

## Post-Upgrade Verification

```sql
-- 1. Library and catalog agree
SELECT tviews.pg_tviews_version(),
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;

-- 2. Health check: nothing above info
SELECT status, component, severity, message
FROM tviews.pg_tviews_check_health()
WHERE severity <> 'info';

-- 3. Registered TVIEWs (needs_reregister is false after the update)
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
