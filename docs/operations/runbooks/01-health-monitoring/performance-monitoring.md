# Performance Monitoring Runbook

## Purpose
Find out why writes to base tables, or reads of TVIEWs, got slower.

## How refresh cost shows up
pg_tviews refreshes TVIEW rows inside the writing transaction: at the end of each
statement and on COMMIT. A slow refresh is therefore a slow `INSERT`/`UPDATE`/`DELETE`
(or a slow `COMMIT`) on a base table. There is no refresh job and no per-refresh
duration history; measure the writing statements themselves.

## When to Use
- Users report slow writes to tables that feed TVIEWs, or slow reads of `tv_*` tables
- After adding a TVIEW, changing a view definition, or a bulk data change
- Capacity planning

## Prerequisites
- `psql` access to the database
- `pg_stat_statements` (recommended) for statement timings

## Step 1: Physical health of each TVIEW

```sql
SELECT entity, tview, persistence, rows_estimate,
       pg_size_pretty(heap_bytes) AS heap,
       pg_size_pretty(index_bytes) AS indexes,
       round(hot_ratio::numeric, 2) AS hot_ratio,
       n_dead_tup, unused_indexes, missing_propagation_indexes, fanout, warnings
FROM tviews.pg_tviews_profile()
ORDER BY heap_bytes DESC;
```

What to look at:
- `warnings`: the function's own findings (UNLOGGED, low HOT ratio, missing indexes, large fan-out).
- `hot_ratio` well below 1: updates are not HOT; check `fillfactor` and indexes on `data`.
- `missing_propagation_indexes`: cascades from embedded TVIEWs scan instead of using an index.
- `fanout`: how many rows one parent row change touches; a high value makes single-row writes expensive.

One TVIEW, with a lower fan-out threshold:

```sql
SELECT * FROM tviews.pg_tviews_profile('user', fanout_warn => 100);
```

## Step 2: Missing propagation indexes

```sql
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
```

Each row is a `CREATE INDEX` statement that would be run. Run it with `dry_run => false`
(or run the statements yourself) to create them.

## Step 3: Time the writing statements

```sql
SELECT query, calls,
       round(total_exec_time::numeric, 1) AS total_ms,
       round(mean_exec_time::numeric, 2) AS mean_ms,
       rows
FROM pg_stat_statements
WHERE query ILIKE '%tb_%' OR query ILIKE '%tv_%'
ORDER BY mean_exec_time DESC
LIMIT 20;
```

Refresh work is included in the time of the statement that wrote the base table.

## Step 4: Refresh counters of one session

`tviews.pg_tviews_queue_stats()` returns counters for the current session only
(`total_refreshes`, `view_recomputes`, `refresh_noop_skipped`, `direct_patches_applied`,
`direct_patch_fallbacks`, `total_timing_ms`, cache hit rates, ...). Take a reading, run
the slow write in the same session, and compare:

```sql
SELECT tviews.pg_tviews_queue_stats();
UPDATE tb_user SET name = name WHERE pk_user = 1;
SELECT tviews.pg_tviews_queue_stats();
```

A large increase in `view_recomputes` means rows were rebuilt from the view rather than
patched directly; `refresh_noop_skipped` counts recomputes that found nothing to change.

## Step 5: Cost of rebuilding one row

A refresh recomputes affected rows from the backing view. Its cost is roughly the cost
of this query:

```sql
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM tviews.public__tv_user WHERE pk_user = 1;
```

A sequential scan here usually means a missing index on a join or foreign-key column
in the base tables.

## Step 6: Dependencies and fan-out

```sql
SELECT * FROM tviews.pg_tviews_show_cascade_path('post');

SELECT schema, name, base_tables, cascade_kinds, uncascaded_tables, uncascaded_policy
FROM tviews.registry
ORDER BY schema, name;
```

`cascade_kinds` says how writes to each base table reach the TVIEW. Base tables listed
in `uncascaded_tables` with `uncascaded_policy = 'full_refresh'` recompute the whole TVIEW
on every write: expect those writes to be slow on large TVIEWs.

## Step 7: Table maintenance

```sql
SELECT relid::regclass AS tview, n_live_tup, n_dead_tup,
       last_vacuum, last_autovacuum, last_analyze, last_autoanalyze
FROM pg_stat_user_tables
WHERE relid IN (SELECT (quote_ident(schema) || '.' || quote_ident(name))::regclass
                FROM tviews.registry WHERE schema IS NOT NULL)
ORDER BY n_dead_tup DESC;
```

High `n_dead_tup` with old vacuum times: tune autovacuum for that table, or run
`VACUUM ANALYZE` on it.

## Configuration

```sql
SELECT name, setting, short_desc
FROM pg_settings
WHERE name LIKE 'pg_tviews.%'
ORDER BY name;
```

See the GUC table in the README for what each one does.

## Related Runbooks

- [TVIEW Health Check](tview-health-check.md)
- [Refresh Queue](queue-management.md)
- [Table Analysis](../03-maintenance/table-analysis.md)
- [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md)
