# Monitoring Guide

What to monitor for pg_tviews in production, and how.

## What there is to monitor

pg_tviews refreshes TVIEW rows inside the transaction that writes the base tables.
There is no refresh job, no queue table and no refresh history. Monitoring therefore
covers:

1. **Health**: installation, catalog, triggers, re-registration (`tviews.pg_tviews_health_check()`)
2. **Correctness**: TVIEW content matches its view
3. **Cost**: refresh work per TVIEW (`tviews.stats`), time of writes to base tables,
   TVIEW sizes, HOT ratio, dead tuples, fan-out
4. **Availability**: UNLOGGED TVIEWs emptied by a crash or a failover

## Health Checks

```sql
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check();

-- Only problems
SELECT component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
```

`severity` is `info`, `warning` or `error`. The full report, including freshness per
TVIEW, is in `docs/operations/runbooks/scripts/health-check.sql`.

## Freshness

`updated_at` on a TVIEW row moves only when the row's content changes:

```sql
SELECT max(updated_at) AS last_change, now() - max(updated_at) AS since_last_change
FROM public.tv_user;
```

Compare with writes to the base tables (`pg_stat_user_tables.n_tup_ins / n_tup_upd /
n_tup_del`); `docs/operations/runbooks/scripts/refresh-status.sql` shows both for every
TVIEW.

## Correctness

Rows where a TVIEW differs from its view (expect none):

```sql
SELECT count(*) AS differing_rows FROM (
  (SELECT pk_user, data FROM public.v_user EXCEPT SELECT pk_user, data FROM public.tv_user)
  UNION ALL
  (SELECT pk_user, data FROM public.tv_user EXCEPT SELECT pk_user, data FROM public.v_user)
) d;
```

This reads the whole view: run it periodically, off-peak, on large TVIEWs. Also watch
which base tables cannot reach a TVIEW by key:

```sql
SELECT schema, name, uncascaded_tables, options->>'uncascaded_policy' AS uncascaded_policy
FROM tviews.registry
WHERE cardinality(uncascaded_tables) > 0;
```

## Cost

### Per TVIEW

```sql
SELECT entity, rows_estimate,
       pg_size_pretty(heap_bytes + index_bytes + toast_bytes) AS total_size,
       round(hot_ratio::numeric, 2) AS hot_ratio, n_dead_tup,
       missing_propagation_indexes, fanout, warnings
FROM tviews.pg_tviews_profile();
```

Alert on non-empty `warnings` and `missing_propagation_indexes`.

### Writes

Refresh time is part of the time of the statement that writes a base table. Use
`pg_stat_statements` (`mean_exec_time` of writes to tables feeding TVIEWs) and
`log_min_duration_statement`.

### Refresh statistics

`tviews.stats` has one row per TVIEW of the database, readable from any session and
by any role:

| Column | Meaning |
|--------|---------|
| `schema`, `name`, `entity` | the TVIEW, as in `tviews.registry` (join on `entity`) |
| `view_recomputes` | rows recomputed from the backing view |
| `noop_skipped` | refreshes skipped because the row already held the result |
| `patch_captured`, `patch_applied`, `patch_fallbacks` | the direct-patch path |
| `propagation_pruned` | parent lookups skipped because the child row did not change |
| `rows_written`, `rows_deleted` | TVIEW rows inserted or updated, deleted, by a refresh of some rows or a reconcile |
| `full_refreshes` | whole-TVIEW refreshes (uncascaded `full_refresh`, `pg_tviews_refresh`, the refill of a reset TVIEW) |
| `refresh_ms` | time spent refreshing this TVIEW |
| `stats_reset` | when its counters started |
| `untracked` | true when the shared table was full as the TVIEW first refreshed; its counters are NULL |

```sql
SELECT entity, view_recomputes, noop_skipped, rows_written, full_refreshes,
       round(refresh_ms::numeric, 1) AS refresh_ms, stats_reset
FROM tviews.stats
ORDER BY refresh_ms DESC NULLS LAST;
```

The counters are cumulative since the server started or the last reset, counted at
the end of each transaction, committed or aborted. They are not kept across a
restart. A TVIEW rebuilt by a replace gets a new table, and its counters start over.
An operator resets them for one TVIEW or for every TVIEW of the database:

```sql
SELECT tviews.pg_tviews_stats_reset('user');
SELECT tviews.pg_tviews_stats_reset();
```

The counters live in shared memory, so `tviews.stats` needs
`shared_preload_libraries = 'pg_tviews'`; without it, reading the view fails with a
hint. The table holds 4096 TVIEWs for the whole cluster. On a standby every counter
is zero: nothing refreshes there. Poll it as you would `pg_stat_user_tables`, and
graph the differences between samples.

### Session counters

`tviews.pg_tviews_queue_stats()` is the session's own debugging view: counters for
the **current session only** (`total_refreshes`, `view_recomputes`,
`refresh_noop_skipped`, `direct_patches_applied`, `direct_patch_fallbacks`,
`total_timing_ms`, `graph_cache_hit_rate`, `table_cache_hit_rate`, ...), not
attributed to a TVIEW. Use it to analyse one workload in one session; poll
`tviews.stats` from a monitoring connection instead.

```sql
SELECT tviews.pg_tviews_queue_stats();
```

### Lock waits between writers

A write waits for a concurrent transaction that writes related rows: the locks are
in `pg_locks` as advisory locks with `objsubid` 21622 (a join value, or the key of a
row being created) and 21623 (a relation: its intent and escalated locks), `classid`
naming the relation ([Concurrency](../concurrency.md)). Who waits for whom:

```sql
SELECT l.pid, c.relname AS relation,
       CASE l.objsubid WHEN 21622 THEN 'value' ELSE 'relation' END AS lock,
       l.mode, l.granted, pg_blocking_pids(l.pid) AS blocked_by
FROM pg_locks l
JOIN pg_class c ON c.oid = l.classid
WHERE l.locktype = 'advisory' AND l.objsubid IN (21622, 21623)
  AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY l.granted, l.pid;
```

In a session, `pg_tviews_queue_stats()` counts the transaction's `value_locks`,
`value_lock_escalations`, `value_lock_waits` and `value_lock_wait_ms`. Many waits on a
hot value are expected; deadlocks (`40P01`) and, under `REPEATABLE READ`,
serialization failures (`40001`) are retried by the application.

## Availability

```sql
SELECT entity, persistence, replica_readable, is_empty, needs_rebuild
FROM tviews.pg_tviews_replication_status();
```

UNLOGGED TVIEWs (declared `logged: false`) are empty on a standby and after a crash
restart or promotion, until the launcher refills them
([Replication](replication.md)). Alert on `needs_rebuild` that lasts; fix with
`SELECT * FROM tviews.pg_tviews_rebuild_all();`.

## Alerting Setup

### Nagios/Icinga check

```bash
#!/bin/bash
PSQL="psql -X -At -h $PGHOST -U $PGUSER -d $PGDATABASE -c"

if ! VERSION=$($PSQL "SELECT tviews.pg_tviews_version()"); then
    echo "CRITICAL: cannot query pg_tviews"
    exit 2
fi

ERRORS=$($PSQL "SELECT count(*) FROM tviews.pg_tviews_health_check() WHERE severity = 'error'")
WARNINGS=$($PSQL "SELECT count(*) FROM tviews.pg_tviews_health_check() WHERE severity = 'warning'")
REBUILD=$($PSQL "SELECT count(*) FROM tviews.pg_tviews_replication_status() WHERE needs_rebuild")

if [ "$ERRORS" -gt 0 ] || [ "$REBUILD" -gt 0 ]; then
    echo "CRITICAL: pg_tviews $VERSION - $ERRORS errors, $REBUILD TVIEWs need rebuild"
    exit 2
elif [ "$WARNINGS" -gt 0 ]; then
    echo "WARNING: pg_tviews $VERSION - $WARNINGS warnings"
    exit 1
fi

echo "OK: pg_tviews $VERSION healthy"
exit 0
```

### Prometheus

With a SQL exporter (for example `postgres_exporter` custom queries), export:

```sql
-- pgtviews_health_problems{severity}
SELECT severity, count(*) AS problems
FROM tviews.pg_tviews_health_check()
GROUP BY severity;

-- pgtviews_tview_bytes{entity}, pgtviews_dead_tuples{entity}, pgtviews_hot_ratio{entity}
SELECT entity, heap_bytes + index_bytes + toast_bytes AS bytes,
       n_dead_tup AS dead_tuples, coalesce(hot_ratio, 1) AS hot_ratio
FROM tviews.pg_tviews_profile();

-- pgtviews_view_recomputes_total{entity}, pgtviews_refresh_ms_total{entity}, ...
SELECT entity, view_recomputes, noop_skipped, rows_written, rows_deleted,
       full_refreshes, refresh_ms
FROM tviews.stats
WHERE NOT untracked;

-- pgtviews_needs_rebuild{entity}
SELECT entity, needs_rebuild::int AS needs_rebuild
FROM tviews.pg_tviews_replication_status();
```

### Grafana panels

1. Health check problems by severity
2. Refresh time and rows recomputed per TVIEW (rates from `tviews.stats`)
3. TVIEW sizes and dead tuples
4. HOT ratio per TVIEW
5. Mean time of writes to base tables (from `pg_stat_statements`)
6. TVIEWs needing a rebuild

## Logging

```sql
ALTER SYSTEM SET log_line_prefix = '%t [%p]: user=%u,db=%d,app=%a ';
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- slow writes, refresh included
SELECT pg_reload_conf();
```

A failed refresh is an ERROR on the writing statement, so it appears in the server log
and to the client.

### Audit log

With `pg_tviews.audit_enabled = on` (a superuser's setting), pg_tviews records
TVIEW operations such as `CREATE` and `REFRESH` in `tviews.pg_tview_audit_log`.
It grows without bound: prune it yourself.

```sql
SELECT performed_at, operation, entity, performed_by, rows_affected, details
FROM tviews.pg_tview_audit_log
ORDER BY performed_at DESC
LIMIT 20;
```

## Troubleshooting with Monitoring

### Health check reports a problem

Follow the message; most trigger, catalog and re-registration problems are fixed by
`SELECT * FROM tviews.pg_tviews_reregister_all();`. See [Debugging](debugging.md).

### Writes got slower

Find the TVIEW whose `refresh_ms` grows fastest in `tviews.stats`. Check `warnings`, `missing_propagation_indexes` and `fanout` in
`tviews.pg_tviews_profile()`, then `EXPLAIN ANALYZE` a single-row query on the view
(`SELECT * FROM tviews.public__tv_user WHERE pk_user = 1`). See
[Performance Monitoring](runbooks/01-health-monitoring/performance-monitoring.md).

### TVIEW differs from its view

`SELECT tviews.pg_tviews_refresh('user');`, then find the cause with the
[Debugging](debugging.md) flowchart.

## Alert Thresholds

- **Critical**: any health check row with severity `error`; any `needs_rebuild`;
  TVIEW content differing from its view
- **Warning**: health check rows with severity `warning`; non-empty `warnings` from
  `pg_tviews_profile()`; mean write time above your baseline

## See Also

- [Operator Guide](../user-guides/operators.md) - Production deployment
- [Troubleshooting Guide](troubleshooting.md) - Issue resolution
- [Performance Tuning](performance-tuning.md) - Optimization strategies
