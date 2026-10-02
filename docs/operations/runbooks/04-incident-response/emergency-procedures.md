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

### Action 1: Let a writer proceed without refresh
Refresh runs inside each writing transaction, so the writer that fails or is too
slow is the one to change. In that session:

```sql
SET pg_tviews.suspend_triggers = on;     -- this session only, until RESET
-- ... the writes that must go through ...
RESET pg_tviews.suspend_triggers;
```

Or, for a single transaction, `SELECT tviews.pg_tviews_suspend_triggers();` after
`BEGIN` (refreshed at `COMMIT`; see [Batch Refresh](../02-refresh-operations/batch-refresh.md)).
The TVIEWs those writes touch are stale until refreshed (Action 3).
[emergency-disable.sql](../scripts/emergency-disable.sql) shows the state and
these commands.

To stop refresh for every session, set the GUC for the application role or the
database and have clients reconnect:

```sql
ALTER ROLE app_writer SET pg_tviews.suspend_triggers = on;
-- undo: ALTER ROLE app_writer RESET pg_tviews.suspend_triggers;
```

Writes then leave every TVIEW stale until Action 3. Use it only when wrong TVIEW
data is acceptable for a while.

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

-- No session left suspended (in each session that suspended)
SELECT tviews.pg_tviews_is_suspended(),
       current_setting('pg_tviews.suspend_triggers', true) AS suspend_triggers;

-- No role or database still suspended
SELECT coalesce(r.rolname, '(all roles)') AS role, coalesce(d.datname, '(all)') AS db, s.setconfig
FROM pg_db_role_setting s
LEFT JOIN pg_roles r ON r.oid = s.setrole
LEFT JOIN pg_database d ON d.oid = s.setdatabase
WHERE array_to_string(s.setconfig, ',') LIKE '%pg_tviews.suspend_triggers%';
```

Then compare every TVIEW with its view (step 4 of post-upgrade-validation.sql,
expect 0) and run the application's critical reads.

## Communication Template
```
TVIEW INCIDENT - [TIMESTAMP]
Status: [ACTIVE/MITIGATED/RESOLVED]
Impact: [writes failing / stale data / slow writes]
Action taken: [e.g. refresh suspended for role X since HH:MM]
Next step / ETA: [...]
Contact: [incident coordinator]
```

## Related Runbooks
- [Incident Checklist](incident-checklist.md) - Systematic incident response
- [Post-Incident Review](post-incident-review.md) - After-action analysis
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Health verification
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) - Refresh errors and stale rows
