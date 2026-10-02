# Batch Refresh Runbook

## Purpose
Load or change many base-table rows without overloading TVIEW refresh, and bring
many TVIEWs up to date at once.

## When to Use
- **Bulk writes**: imports, ETL, backfills, mass updates or deletes
- **After refresh was suspended**: bring the affected TVIEWs back to their views
- **Repair**: rebuild every TVIEW after a suspected inconsistency

## How refresh behaves under bulk writes
- Triggers on the base tables queue the affected TVIEW keys in memory, per
  transaction. The queue is flushed (the TVIEW rows are recomputed) at the end of
  each statement and on `COMMIT`. There is no queue table and no background job.
- `pg_tviews.max_queue_size` (default 10000) caps the keys queued before a flush. A
  statement that queues more fails with
  `ERROR: refresh queue backpressure: queue size (...) would exceed max_queue_size (...)`
  and rolls back.
- `pg_tviews.batch_size` (default 1000) is the number of keys recomputed per
  refresh statement during a flush; larger flushes are split into chunks.
- A refresh error fails the writing statement, which rolls back with it.

## Option 1: Let the triggers refresh (default)

Write in statements of a size the flush handles, and raise the limits for the
session if one statement must touch more keys:

```sql
SET pg_tviews.max_queue_size = 100000;  -- this session only
SET pg_tviews.batch_size = 5000;        -- larger refresh chunks
-- ... bulk statements ...
RESET pg_tviews.max_queue_size;
RESET pg_tviews.batch_size;
```

## Option 2: Suspend refresh for one transaction

Suspension lasts until the end of the transaction. While suspended, writes
queue nothing; pg_tviews records which TVIEWs they touched, and resuming rebuilds
those TVIEWs and every TVIEW that embeds them.

```sql
BEGIN;
SELECT tviews.pg_tviews_suspend_triggers();
-- ... bulk statements ...
SELECT tviews.pg_tviews_is_suspended(), tviews.pg_tviews_suspended_entities();
SELECT tviews.pg_tviews_resume_triggers();   -- rebuilds the recorded TVIEWs
COMMIT;
```

An explicit `COMMIT` while still suspended resumes and rebuilds the same way. An
implicit commit (a suspended autocommit statement, such as a `DO` block) cannot
rebuild: it logs a `WARNING` naming the TVIEWs it wrote to, which, with the TVIEWs
that embed them, then need a refresh (`pg_tviews_refresh_all()`, below).
Calls nest: each `pg_tviews_suspend_triggers()` needs its own
`pg_tviews_resume_triggers()`.

## Option 3: Suspend refresh for a whole session

The GUC `pg_tviews.suspend_triggers` stops refresh across transactions in one
session. It records nothing, so refresh afterwards yourself:

```sql
SET pg_tviews.suspend_triggers = on;
-- ... bulk transactions ...
RESET pg_tviews.suspend_triggers;
SELECT tviews.pg_tviews_refresh_all();
```

Other sessions keep refreshing normally in all three options.

## Refreshing many TVIEWs

```sql
-- Every TVIEW, dependencies first; returns the order, count and duration_ms
SELECT tviews.pg_tviews_refresh_all();

-- One TVIEW (by entity, without the tv_ prefix)
SELECT tviews.pg_tviews_refresh('user');

-- Rebuild only the TVIEWs that are empty (e.g. UNLOGGED ones after a crash),
-- or all of them with only_empty => false
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true);
```

`pg_tviews_refresh(entity)` recomputes that TVIEW from its view, writes only the
rows that differ and deletes rows no longer in the view. It does not refresh the
TVIEWs that embed it; list those with `pg_tviews_show_cascade_path` and refresh
them after it, or use `pg_tviews_refresh_all()`:

```sql
SELECT * FROM tviews.pg_tviews_show_cascade_path('user') ORDER BY depth;
```

`pg_tviews_refresh_all()` raises an error while refresh is suspended in the
session.

## Planning a large refresh
pg_tviews keeps no per-refresh duration history. Estimate from the TVIEW sizes and
time a refresh in staging (`\timing` in psql, or `duration_ms` from
`pg_tviews_refresh_all()`):

```sql
SELECT entity, tview, rows_estimate,
       pg_size_pretty(heap_bytes + index_bytes + toast_bytes) AS size
FROM tviews.pg_tviews_profile()
ORDER BY heap_bytes + index_bytes + toast_bytes DESC;
```

Each refresh runs in the calling transaction; for large TVIEWs run it at low
traffic.

## Verification
- Compare each TVIEW with its view: step 4 of
  [post-upgrade-validation.sql](../../upgrade/scripts/post-upgrade-validation.sql)
  (expect 0 differing rows).
- `SELECT * FROM tviews.pg_tviews_health_check() WHERE severity <> 'info';`
  returns nothing.
- Confirm the session is not left suspended:
  `SELECT tviews.pg_tviews_is_suspended();`

## Related Runbooks
- [Manual Refresh](manual-refresh.md) - Refresh one TVIEW
- [Refresh Troubleshooting](refresh-troubleshooting.md) - Failing or stale refreshes
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Refresh cost
- [emergency-disable.sql](../scripts/emergency-disable.sql) - Suspension commands
