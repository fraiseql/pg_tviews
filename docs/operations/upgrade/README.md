# pg_tviews Upgrade Guides

Procedures for upgrading PostgreSQL and pg_tviews on servers that run TVIEWs.

## Quick Reference

| Upgrade | Guide | Downtime | What pg_tviews needs |
|---------|-------|----------|----------------------|
| PostgreSQL minor (18.0 to 18.1) | [Minor Version Upgrade](postgresql/minor-version-upgrade.md) | a restart | nothing; rebuild UNLOGGED TVIEWs only after an unclean shutdown |
| An older PostgreSQL to 18 | [To PostgreSQL 18](postgresql/to-pg18.md) | pg_upgrade or dump/restore | same pg_tviews release built for the new major; update pg_tviews afterwards |
| pg_tviews: from the previous release | [Upgrading pg_tviews](../upgrade.md) | a restart, then one statement per database; writes refused in between | `ALTER EXTENSION pg_tviews UPDATE` (it re-derives every TVIEW) |
| pg_tviews: from an older release, `0.1.0` installs included | [Upgrading pg_tviews](../upgrade.md#from-an-older-release) | until the last TVIEW is created again | `DROP EXTENSION pg_tviews CASCADE`, `CREATE EXTENSION pg_tviews`, the TVIEWs created again |
| Problems | [Troubleshooting Upgrades](postgresql/troubleshooting-upgrades.md) | | |

Supported PostgreSQL version: 18. How pg_tviews versions its extension SQL and
update script: [Extension versioning](../../development/extension-versioning.md).

## Rules that apply to every upgrade

- **One change at a time.** Upgrade PostgreSQL with the pg_tviews release you run, then
  pg_tviews (or the reverse), never both in one step.
- **Back up first.** `pg_dump -Fc` of each database with pg_tviews; the dump includes
  the TVIEW registrations (`tviews.pg_tview_meta`).
- **No downgrade scripts.** Rolling back pg_tviews means reinstalling the previous
  package and restoring the backup.
- **Every database.** The library is shared by the whole server; each database with
  pg_tviews needs its own `ALTER EXTENSION pg_tviews UPDATE`. Until then, writes to its
  TVIEW base tables fail with
  `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`.
- **Drop the extension only to leave an older release.** `DROP EXTENSION pg_tviews`
  removes every TVIEW registration; from the previous release, update instead.
- **UNLOGGED TVIEWs** (the default) are empty after a crash, an immediate shutdown or a
  physical restore. Check with `SELECT * FROM tviews.pg_tviews_show_replication();`
  and rebuild with `SELECT * FROM tviews.pg_tviews_rebuild_all();`.

## Pre-Upgrade Checklist
- [ ] Backup taken and restore tested ([Backup Testing](../disaster-recovery/backup-strategy/backup-testing.md))
- [ ] `scripts/pre-upgrade-checks.sh` passes in each database
- [ ] Release notes (CHANGELOG) read for the release installed
- [ ] jsonb_delta installed in each database (the update refuses without it)
- [ ] Procedure rehearsed on a copy of production
- [ ] Maintenance window agreed; applications can be stopped or put read-only

## Supporting Scripts

In [`scripts/`](scripts/):

- `pre-upgrade-checks.sh`: read-only checks before an upgrade (PostgreSQL version,
  installed and available pg_tviews versions, health-check errors, prepared
  transactions). Run as `PGDATABASE=<db> docs/operations/upgrade/scripts/pre-upgrade-checks.sh`.
- `post-upgrade-validation.sql`: versions, catalog revision, TVIEWs marked
  `needs_reregister` (none after an update), full health check, and each TVIEW
  compared with its view. Run as
  `psql -X -v ON_ERROR_STOP=1 -d <db> -f docs/operations/upgrade/scripts/post-upgrade-validation.sql`.

## Success Criteria
- [ ] `extversion` equals `tviews.pg_tviews_version()` in every database
- [ ] `tviews.pg_tviews_check_health()` reports no warning or error
- [ ] No TVIEW is unexpectedly empty
- [ ] Each TVIEW equals its view (post-upgrade validation step 4)
- [ ] A test write to a base table propagates; applications work
