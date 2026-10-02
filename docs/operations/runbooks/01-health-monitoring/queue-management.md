# Refresh Queue Runbook

## Purpose
Explain what the pg_tviews refresh queue is, how to observe it, and what to do when a
write fails because of it.

## What the queue is
- When a statement writes a base table, row triggers record the affected TVIEW keys in
  a queue held **in memory, in the writing backend, for the current transaction**.
- The queue is flushed (the TVIEW rows are refreshed) at the end of each statement, by a
  statement trigger, and again on `COMMIT` / `PREPARE TRANSACTION`.
- There is no queue table, no background worker and no refresh schedule. Nothing is left
  behind between transactions, so there is nothing to clean up, retry or vacuum.
- If a refresh fails, the writing statement fails with an ERROR and its transaction rolls
  back, base-table changes included. Errors appear to the client and in the server log.

## Observing the queue

Contents of the queue in the current transaction (normally `[]` when called from SQL,
because the previous statement already flushed it):

```sql
SELECT tviews.pg_tviews_debug_queue();
```

Counters for the current session, including `queue_size`, `total_refreshes`,
`max_iterations` and `total_timing_ms`:

```sql
SELECT tviews.pg_tviews_queue_stats();
```

The counters cover this session only; read them before and after a write in the same
session and compare.

What a transaction changed in TVIEWs (keys updated and deleted, per entity):

```sql
SELECT tviews.pg_tviews_flush_and_report();
```

## Queue size limit

`pg_tviews.max_queue_size` (default 10000) caps the number of keys one transaction may
queue. Beyond it the write fails with
`refresh queue backpressure: queue size (...) would exceed max_queue_size (...)`.

Options, in order of preference:
1. Split the bulk write into smaller transactions.
2. Suspend refresh for the bulk load and refresh afterwards:
   ```sql
   BEGIN;
   SELECT tviews.pg_tviews_suspend_triggers();
   -- bulk INSERT / UPDATE / DELETE
   SELECT tviews.pg_tviews_resume_triggers();  -- refreshes what changed
   COMMIT;
   ```
   Suspension ends with the transaction if `pg_tviews_resume_triggers()` is not called.
   `tviews.pg_tviews_is_suspended()` and `tviews.pg_tviews_suspended_entities()` show the
   current state.
3. Raise the limit for the session: `SET pg_tviews.max_queue_size = 100000;`

## Writes that look stuck

A write that waits is waiting on locks, not on a queue. Look for blocking sessions:

```sql
SELECT pid, state, wait_event_type, wait_event,
       now() - xact_start AS xact_age,
       pg_blocking_pids(pid) AS blocked_by,
       left(query, 80) AS query
FROM pg_stat_activity
WHERE datname = current_database()
  AND (state = 'idle in transaction' OR cardinality(pg_blocking_pids(pid)) > 0)
ORDER BY xact_start;
```

Also check prepared transactions, which keep their TVIEW row locks until
`COMMIT PREPARED` / `ROLLBACK PREPARED`:

```sql
SELECT gid, prepared, owner, database FROM pg_prepared_xacts ORDER BY prepared;
```

## Related Runbooks

- [TVIEW Health Check](tview-health-check.md)
- [Performance Monitoring](performance-monitoring.md)
- [Manual Refresh](../02-refresh-operations/manual-refresh.md)
- [Emergency Procedures](../04-incident-response/emergency-procedures.md)
