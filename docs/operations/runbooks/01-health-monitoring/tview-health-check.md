# TVIEW Health Check Runbook

## Purpose
Check that pg_tviews is installed correctly, every TVIEW is registered with working
triggers, and TVIEW contents match their backing views.

## When to Use
- Routine monitoring
- After an extension upgrade, a schema change or a bulk data change
- When users report stale data in a `tv_*` table
- As the first step of an incident

## Prerequisites
- `psql` access to the database
- SELECT on the `tviews` schema and on the TVIEW tables

## Quick Health Check (2 minutes)

```bash
psql -X -v ON_ERROR_STOP=1 -d "$DB_NAME" -f docs/operations/runbooks/scripts/health-check.sql
```

The script prints versions, the health check, the registry, freshness per TVIEW,
physical health and replication readiness. The core of it:

```sql
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
ORDER BY CASE severity WHEN 'error' THEN 1 WHEN 'warning' THEN 2 ELSE 3 END, component;
```

Expected: every row has severity `info`. The components covered include the
extension version, jsonb_delta, catalog revision, metadata, re-registration and triggers.

## Comprehensive Health Check (10 minutes)

### Step 1: Registered TVIEWs

```sql
SELECT schema, name, entity, options->'logged' AS logged, needs_reregister,
       base_tables, uncascaded_tables, options->>'uncascaded_policy' AS uncascaded_policy, cascade_kinds
FROM tviews.registry
ORDER BY schema, name;
```

- `needs_reregister = true`: run `SELECT * FROM tviews.pg_tviews_reregister_all();`
- `uncascaded_tables` not empty: writes to those tables do not reach the TVIEW by key;
  `uncascaded_policy` says what happens instead (`warn`, `error`, `full_refresh`).

### Step 2: Freshness

`updated_at` on a TVIEW row moves only when the row's content changes.

```sql
SELECT count(*) AS rows, max(updated_at) AS last_change,
       now() - max(updated_at) AS since_last_change
FROM public.tv_user;
```

A TVIEW that has not changed for a long time is not necessarily stale: compare with
recent writes to its base tables (`n_tup_ins`, `n_tup_upd`, `n_tup_del` in
`pg_stat_user_tables`). `docs/operations/runbooks/scripts/refresh-status.sql` shows
both for every TVIEW.

### Step 3: Content matches the view

The TVIEW should hold exactly what its view returns. Rows that differ:

```sql
(SELECT pk_user, data FROM public.v_user
 EXCEPT
 SELECT pk_user, data FROM public.tv_user)
UNION ALL
(SELECT pk_user, data FROM public.tv_user
 EXCEPT
 SELECT pk_user, data FROM public.v_user);
```

Expected: no rows. This reads the whole view; run it off-peak on large TVIEWs.
If rows differ, rebuild the TVIEW (`SELECT tviews.pg_tviews_refresh('user');`) and
report the case: it is a bug.

### Step 4: Physical health

```sql
SELECT entity, rows_estimate, pg_size_pretty(heap_bytes) AS heap,
       round(hot_ratio::numeric, 2) AS hot_ratio, n_dead_tup, warnings
FROM tviews.pg_tviews_profile()
ORDER BY entity;
```

See [Performance Monitoring](performance-monitoring.md) for how to read it.

### Step 5: Replication readiness

```sql
SELECT * FROM tviews.pg_tviews_replication_status() ORDER BY entity;
```

UNLOGGED TVIEWs (declared `logged: false`) are empty on a standby and after a crash
restart, until the launcher refills them.
`needs_rebuild = true`: run `SELECT * FROM tviews.pg_tviews_rebuild_all();`.

## Expected Results

### Healthy
- Every `pg_tviews_health_check()` row has severity `info`
- No TVIEW with `needs_reregister`
- Step 3 returns no rows
- No `needs_rebuild` in Step 5

### Warning signs
- Health check rows with severity `warning` (for example jsonb_delta missing, orphaned triggers)
- `uncascaded_tables` with policy `warn` on TVIEWs whose base tables are written often
- `warnings` from `pg_tviews_profile()`

### Critical
- Health check rows with severity `error` (catalog revision mismatch, missing triggers)
- Step 3 returns rows
- Writes to base tables failing with pg_tviews errors

## Troubleshooting

### Health check reports missing or orphaned triggers

```sql
SELECT * FROM tviews.pg_tviews_reregister_all();
```

An orphaned trigger named in the message can be dropped with `DROP TRIGGER ... ON ...`.

### Catalog revision mismatch after an upgrade

```sql
ALTER EXTENSION pg_tviews UPDATE;
SELECT * FROM tviews.pg_tviews_reregister_all();
```

### A TVIEW shows old data

```sql
SELECT tviews.pg_tviews_refresh('user');
```

Then find out why: check Step 1 (`uncascaded_tables`) and the
[Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md) runbook.

## Escalation

1. Warnings: record them and watch for trends
2. Errors in the health check, or Step 3 differences: follow the
   [Incident Checklist](../04-incident-response/incident-checklist.md)

## Related Runbooks

- [Refresh Queue](queue-management.md)
- [Performance Monitoring](performance-monitoring.md)
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md)
