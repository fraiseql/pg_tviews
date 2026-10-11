# Emergency Procedures Runbook

## Purpose
Restore service quickly when pg_tviews blocks writes, serves wrong data, or loads
the database.

## When to Use
- **Writes failing**: base-table writes or `COMMIT`s fail with TVIEW refresh errors
- **Wrong data**: TVIEWs are empty or differ from their views
- **Load**: refresh work slows writes enough to affect the application
- **After a crash, failover or upgrade** that left TVIEWs empty or unusable

## Prerequisites
- `psql` access as a superuser or the TVIEW owner
- A recent backup (for the last-resort option)
- An incident record started ([Incident Checklist](incident-checklist.md))

## Emergency Assessment (2 minutes)

```sql
-- What pg_tviews reports as broken
SELECT component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';

-- Empty TVIEWs (UNLOGGED after a crash, or on a standby)
SELECT * FROM tviews.pg_tviews_replication_status() WHERE is_empty OR needs_rebuild;

-- Sessions waiting on locks
SELECT pid, state, wait_event_type, now() - xact_start AS xact_age, left(query, 80) AS query
FROM pg_stat_activity
WHERE datname = current_database() AND wait_event_type = 'Lock'
ORDER BY xact_age DESC;
```

The PostgreSQL log and the failing clients carry the refresh error text; pg_tviews
stores no error state.

## Emergency Actions

### Action 1: Defer refresh to the end of a writing transaction
Refresh runs inside each writing transaction, so the writer that fails or is too
slow is the one to change. Suspend refresh in that transaction:

```sql
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();   -- this transaction only
-- ... the writes that must go through ...
SELECT tviews.pg_tviews_resume_triggers();    -- refreshes the TVIEWs it skipped
COMMIT;                                       -- or COMMIT directly: same refresh
```

Suspension lasts until the end of the transaction. It records the TVIEWs the writes
touched and refreshes them at resume or at `COMMIT`, so the transaction still pays
for the refresh, once, at the end; other sessions keep refreshing. If that final
refresh is what fails, the transaction rolls back; then fix the cause (a broken
definition: Action 5) and refresh with Action 3. See
[Batch Refresh](../02-refresh-operations/batch-refresh.md).
[emergency-disable.sql](../scripts/emergency-disable.sql) shows the state and
these commands.

There is no switch that stops refresh for every session: a TVIEW that does not
follow its base tables is not offered.

### Action 2: Clear blocking sessions
```sql
-- Cancel the query of a session holding locks for too long
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE datname = current_database()
  AND state = 'idle in transaction'
  AND xact_start < now() - interval '30 minutes';
```

Use `pg_terminate_backend(pid)` if cancelling is not enough. A cancelled
transaction rolls back its base-table writes and their TVIEW refreshes together.

### Action 3: Bring TVIEWs back to their views
```sql
SELECT tviews.pg_tviews_refresh_all();                         -- everything
SELECT tviews.pg_tviews_refresh('user');                       -- one entity
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true); -- empty ones
```

### Action 4: Repair registration after an upgrade
```sql
SELECT * FROM tviews.pg_tviews_reregister_all();
SELECT * FROM tviews.pg_tviews_health_check();
```

### Action 5: Recreate a TVIEW
If a definition is broken, replace it, or drop it and recreate it from your schema
scripts:

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_user',
    $$SELECT pk_user, id, jsonb_build_object('name', name) AS data FROM tb_user$$);
-- SELECT tviews.pg_tviews_drop('tv_user', if_exists => true, cascade => true);
```

### Action 6: Restore from backup (last resort)
TVIEWs are derived data: restoring base tables and recreating or refreshing the
TVIEWs is enough. Restore to a separate database first and check it with
[health-check.sql](../scripts/health-check.sql) and step 4 of
[post-upgrade-validation.sql](../../upgrade/scripts/post-upgrade-validation.sql).

## Post-Emergency Verification
```sql
-- No warnings or errors
SELECT component, severity, message
FROM tviews.pg_tviews_health_check() WHERE severity <> 'info';

-- Refresh statistics: full_refreshes counts the Action 3 refreshes
SELECT entity, full_refreshes, rows_written, stats_reset FROM tviews.stats ORDER BY entity;
```

Then compare every TVIEW with its view (step 4 of post-upgrade-validation.sql,
expect 0) and run the application's critical reads.

## Communication Template
```
TVIEW INCIDENT - [TIMESTAMP]
Status: [ACTIVE/MITIGATED/RESOLVED]
Impact: [writes failing / stale data / slow writes]
Action taken: [e.g. bulk load run with refresh suspended, TVIEWs refreshed at HH:MM]
Next step / ETA: [...]
Contact: [incident coordinator]
```

## Related Runbooks
- [Incident Checklist](incident-checklist.md) - Systematic incident response
- [Post-Incident Review](post-incident-review.md) - After-action analysis
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Health verification
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Refresh errors and stale rows
