# PostgreSQL Minor Version Upgrade

## Scope
Upgrading PostgreSQL within the same major version (for example 16.4 to 16.6, or 18.0
to 18.1). pg_tviews supports PostgreSQL 16, 17 and 18.

## What it means for pg_tviews
- A minor upgrade replaces the server binaries and restarts the server. The data
  directory, the extension's catalog and the TVIEWs are unchanged; there is nothing to
  run in the databases.
- The `pg_tviews.so` built for that major version keeps working (PostgreSQL keeps the
  extension ABI stable within a major version). Do **not** drop and recreate the
  extension: `DROP EXTENSION pg_tviews` drops every TVIEW registration.
- UNLOGGED TVIEW tables (the default) keep their rows across a clean shutdown and
  restart. They are emptied only by a crash or an immediate shutdown
  (`pg_ctl stop -m immediate`); then rebuild them (see below).

## Prerequisites
- A recent, verified backup ([Backup Types](../../disaster-recovery/backup-strategy/backup-types.md))
- A maintenance window for the restart
- The release notes of the PostgreSQL minor versions you skip (some ask for a
  `REINDEX` of certain index types)

## Procedure

### Step 1: Record the current state
```bash
psql -X -d mydb -c "SELECT version();"
psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/runbooks/scripts/health-check.sql > pre-upgrade-health.txt
```

### Step 2: Install the new binaries and restart
Follow your packaging (for example `apt install --only-upgrade postgresql-16`), then
restart with a clean (`fast` or `smart`) shutdown:
```bash
sudo systemctl restart postgresql
```

### Step 3: Verify
```sql
SELECT version();
SELECT tviews.pg_tviews_version(),
       (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews') AS extension;
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
SELECT * FROM tviews.pg_tviews_replication_status();
```
`is_empty = true` with `needs_rebuild = true` means an UNLOGGED TVIEW lost its rows
(the server did not shut down cleanly). Rebuild:
```sql
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

For a full check, including each TVIEW against its view:
```bash
psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/upgrade/scripts/post-upgrade-validation.sql
```

### Step 4: Restart applications
Point applications back and run their health checks.

## Success Criteria
- [ ] `SELECT version()` shows the new minor version
- [ ] The health check reports no warning or error
- [ ] No TVIEW `needs_rebuild`
- [ ] Applications work

## Rollback
Reinstall the previous minor version's packages and restart. Minor versions of one
major share the on-disk format, so no restore is needed.

## Troubleshooting

### PostgreSQL does not start: `could not access file "pg_tviews"`
`shared_preload_libraries` names pg_tviews but the library is missing from
`$(pg_config --pkglibdir)`. Some packaging removes files on upgrade: reinstall the
pg_tviews package (or `cargo pgrx install --release --pg-config "$(which pg_config)"`
from source) for this major version, then start the server.

### Writes fail with `pg_tviews library catalog revision <n> does not match the installed extension (<m>)`
A different pg_tviews release was installed along with the PostgreSQL upgrade. Either
reinstall the release matching `extversion`, or update the extension:
[Extension Updates](../extension/extension-minor-update.md).

## Related Guides
- [Upgrading to PostgreSQL 16 from 15](pg15-to-pg16.md)
- [Extension Updates](../extension/extension-minor-update.md)
- [Troubleshooting Upgrades](troubleshooting-upgrades.md)
- [Emergency Procedures](../../runbooks/04-incident-response/emergency-procedures.md)
