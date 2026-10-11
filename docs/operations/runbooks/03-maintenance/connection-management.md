# Connection Management Runbook

## Purpose
Monitor database connections and long transactions in a database that uses pg_tviews.

## How pg_tviews uses connections
pg_tviews opens no connections of its own and has no background worker that refreshes
TVIEWs. A TVIEW is refreshed inside the backend of the session that wrote to its base
tables: row triggers queue the affected keys in memory for the current transaction, and
the queue is flushed at the end of each statement and on `COMMIT`. Consequences:

- connection count is driven only by your applications and poolers;
- a write statement that touches TVIEW base tables takes longer, because it also
  refreshes the TVIEW rows, and holds row locks on those `tv_*` rows until it commits;
- a long or idle-in-transaction session that refreshed TVIEW rows keeps those rows
  locked, so other writers that refresh the same rows wait on it.

## When to Use
- Connection usage alerts or pool exhaustion
- Writers waiting on locks in `tv_*` tables
- Before a maintenance window

## Prerequisites
- Access to `pg_stat_activity` (`pg_monitor` role or superuser)
- Permission to cancel or terminate backends, if needed

## Connection Overview
```sql
SELECT
    count(*)                                              AS total_connections,
    count(*) FILTER (WHERE state = 'active')              AS active,
    count(*) FILTER (WHERE state = 'idle')                AS idle,
    count(*) FILTER (WHERE state = 'idle in transaction') AS idle_in_transaction,
    count(*) FILTER (WHERE wait_event_type = 'Lock')      AS waiting_on_lock,
    current_setting('max_connections')::int               AS max_connections,
    round(100.0 * count(*) / current_setting('max_connections')::int, 1) AS utilization_percent
FROM pg_stat_activity
WHERE backend_type = 'client backend';
```

## Long and Idle Transactions
```sql
SELECT pid, usename, client_addr, state,
       now() - xact_start  AS transaction_age,
       wait_event_type, wait_event,
       left(query, 80)     AS query_preview
FROM pg_stat_activity
WHERE xact_start IS NOT NULL
  AND now() - xact_start > interval '5 minutes'
  AND pid <> pg_backend_pid()
ORDER BY xact_start;
```

Cancel the current statement (`pg_cancel_backend`) or end the session
(`pg_terminate_backend`) only after confirming with the owner of the session. Either
rolls the transaction back, including the TVIEW rows it refreshed; nothing needs to be
repaired in pg_tviews afterwards.

To prevent sessions from sitting idle in a transaction:
```sql
ALTER DATABASE mydb SET idle_in_transaction_session_timeout = '10min';
```

## Lock Waits on TVIEW Tables
```sql
SELECT w.pid                  AS waiting_pid,
       left(w.query, 60)      AS waiting_query,
       b.pid                  AS blocking_pid,
       b.state                AS blocking_state,
       now() - b.xact_start   AS blocking_transaction_age,
       left(b.query, 60)      AS blocking_query
FROM pg_stat_activity w
JOIN LATERAL unnest(pg_blocking_pids(w.pid)) AS bp(pid) ON true
JOIN pg_stat_activity b ON b.pid = bp.pid
WHERE w.wait_event_type = 'Lock';
```

If many writers contend on the same TVIEW rows (for example many child rows embedded
in one parent document), keep the writing transactions short. `tviews.pg_tviews_profile()`
reports fan-out (how many TVIEW rows one base-table write refreshes) in its `fanout`
and `warnings` columns.

## Connection Poolers
- Session pooling and transaction pooling both work for ordinary writes: the refresh
  queue lives and is flushed inside one transaction.
- `tviews.pg_tviews_suspend_triggers()` / `tviews.pg_tviews_resume_triggers()` last
  until the end of the transaction: run the sequence inside one `BEGIN` … `COMMIT`,
  which works with any pooler.

## Configuration Review
```sql
SELECT name, setting, unit, context
FROM pg_settings
WHERE name IN ('max_connections', 'idle_in_transaction_session_timeout',
               'statement_timeout', 'lock_timeout', 'shared_preload_libraries')
ORDER BY name;
```

## Troubleshooting

### Connection pool exhaustion
Diagnose with the connection overview above. Fixes are the usual PostgreSQL ones:
a pooler (pgbouncer), fewer application connections, or a higher `max_connections`
(restart required).

### Writes to base tables became slow
Each write also refreshes the TVIEW rows it affects. Check fan-out and indexes with
`SELECT * FROM tviews.pg_tviews_profile();` and missing propagation indexes with
`SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);`.
See [Performance Monitoring](../01-health-monitoring/performance-monitoring.md).

## Related Runbooks
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md)
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md)
- [Regular Maintenance](regular-maintenance.md)
- [Emergency Procedures](../04-incident-response/emergency-procedures.md)
