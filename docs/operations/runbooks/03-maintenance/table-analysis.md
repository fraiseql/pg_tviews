# Table Analysis Runbook

## Purpose
Inspect the size, statistics, indexes and query plans of TVIEW tables (`tv_*`), and of
the refresh work that keeps them current.

## When to Use
- Reads from a TVIEW are slow
- Writes to base tables became slow (each write refreshes the affected TVIEW rows)
- Disk usage of TVIEW tables grows unexpectedly
- After bulk loads

## Prerequisites
- Access to `pg_stat_user_tables`, `pg_stat_user_indexes`, `pg_stats`
- Optional: `pg_stat_statements` for statement timings

## Table Statistics Analysis

### Step 1: Inventory
```sql
SELECT schema, name, entity, logged, base_tables
FROM tviews.registry
ORDER BY schema, name;
```

### Step 2: Physical health per TVIEW
```sql
SELECT entity, tview, persistence, rows_estimate,
       pg_size_pretty(heap_bytes)  AS heap,
       pg_size_pretty(index_bytes) AS indexes,
       pg_size_pretty(toast_bytes) AS toast,
       avg_row_width, data_avg_width, fillfactor,
       n_tup_upd, n_tup_hot_upd, round(hot_ratio::numeric, 2) AS hot_ratio,
       n_dead_tup, last_vacuum, last_autovacuum,
       round(all_visible_fraction::numeric, 2) AS all_visible,
       warnings
FROM tviews.pg_tviews_profile()
ORDER BY heap_bytes DESC;
```
`warnings` summarizes what the other columns show (low HOT ratio, many dead tuples,
missing indexes, high fan-out). Pass an entity to inspect one TVIEW:
`SELECT * FROM tviews.pg_tviews_profile('post');`.

### Step 3: Statistics freshness
```sql
SELECT s.schemaname, s.relname, s.n_live_tup, s.n_dead_tup,
       s.n_mod_since_analyze, s.last_analyze, s.last_autoanalyze
FROM pg_stat_user_tables s
JOIN tviews.registry r ON r.schema = s.schemaname AND r.name = s.relname
ORDER BY s.n_mod_since_analyze DESC;
```
Run `ANALYZE` on a TVIEW whose statistics are stale (see
[Regular Maintenance](regular-maintenance.md) for all TVIEWs at once).

### Step 4: Index usage
```sql
SELECT i.schemaname, i.relname, i.indexrelname, i.idx_scan,
       pg_size_pretty(pg_relation_size(i.indexrelid)) AS index_size
FROM pg_stat_user_indexes i
JOIN tviews.registry r ON r.schema = i.schemaname AND r.name = i.relname
ORDER BY i.idx_scan, pg_relation_size(i.indexrelid) DESC;
```
Before dropping an index with `idx_scan = 0`, check that it is not used by refreshes:
indexes on `fk_*` columns serve cascades from parent entities, and statistics reset on
a crash or `pg_stat_reset()`. `pg_tviews_profile()` lists indexes it considers unused in
`unused_indexes`, and missing cascade indexes in `missing_propagation_indexes`:
```sql
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
```

## Query Performance Analysis

### Reads from a TVIEW
```sql
EXPLAIN (ANALYZE, BUFFERS)
SELECT data FROM public.tv_post WHERE fk_user = 1;
```
TVIEW tables are ordinary tables: add indexes for your read patterns with
`CREATE INDEX CONCURRENTLY`. `pg_tviews.data_gin_index` creates a GIN index on `data`
for new TVIEWs.

### Refresh cost
A refresh recomputes affected rows from its backing view (`tviews.<schema>__tv_<entity>`) by key. Its cost
is the cost of that query:
```sql
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM public.v_post WHERE pk_post = 1;
```
Sequential scans here usually mean a missing index on a base-table join or
foreign-key column.

How many TVIEW rows one write touches (fan-out):
```sql
SELECT entity, fanout FROM tviews.pg_tviews_profile();
```

Refresh counters of the current session (compare before and after a write in the same
session):
```sql
SELECT tviews.pg_tviews_queue_stats();
```

Statement timings, if `pg_stat_statements` is installed (PostgreSQL 13+ column names):
```sql
SELECT calls, round(mean_exec_time::numeric, 2) AS mean_ms,
       round(total_exec_time::numeric, 2) AS total_ms, left(query, 100) AS query
FROM pg_stat_statements
WHERE query ILIKE '%tv\_%' OR query ILIKE '%pg_tviews%'
ORDER BY mean_exec_time DESC
LIMIT 10;
```

## Storage

Size breakdown, including TOAST (the `data` jsonb column is usually stored there):
```sql
SELECT format('%I.%I', r.schema, r.name) AS tview,
       pg_size_pretty(pg_table_size(format('%I.%I', r.schema, r.name)))    AS table_with_toast,
       pg_size_pretty(pg_indexes_size(format('%I.%I', r.schema, r.name)))  AS indexes,
       pg_size_pretty(pg_total_relation_size(format('%I.%I', r.schema, r.name))) AS total
FROM tviews.registry r
ORDER BY pg_total_relation_size(format('%I.%I', r.schema, r.name)) DESC;
```

## Troubleshooting

### High bloat
Check that autovacuum keeps up (`last_autovacuum`, `n_dead_tup` above). If the table
stays bloated, `VACUUM FULL public.tv_post;` rewrites it under an `ACCESS EXCLUSIVE`
lock; run it in a maintenance window.

### Low HOT ratio
Updates are not HOT when an indexed column changes or the page has no free space.
Drop indexes on columns that change often if reads do not need them, and keep a
fillfactor below 100 (`ALTER TABLE ... SET (fillfactor = 85)` applies to new pages;
`VACUUM FULL` applies it to the whole table).

## Related Runbooks
- [Regular Maintenance](regular-maintenance.md)
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md)
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md)
- [Connection Management](connection-management.md)
