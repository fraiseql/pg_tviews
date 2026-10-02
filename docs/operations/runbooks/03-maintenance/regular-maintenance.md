# Regular Maintenance Runbook

## Purpose
Routine checks and upkeep for a database that uses pg_tviews.

pg_tviews keeps no queue table, refresh log or history that grows over time: the
refresh queue lives in memory inside each writing transaction. Regular maintenance is
therefore ordinary PostgreSQL upkeep of the `tv_*` tables plus a few pg_tviews checks.

## When to Use
- Weekly, during a low-usage window
- After bulk loads or schema changes
- When TVIEW reads or base-table writes slow down gradually

## Prerequisites
- Owner of the TVIEW tables (or superuser) for `VACUUM` / `ANALYZE` / `REINDEX`
- A recent backup before `VACUUM FULL` or `REINDEX`

## Weekly Maintenance

### Step 1: Health check
```bash
psql -X -v ON_ERROR_STOP=1 -d mydb -f docs/operations/runbooks/scripts/health-check.sql
```
Or only the component status:
```sql
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
```

### Step 2: Dead tuples and HOT updates on TVIEW tables
TVIEW tables are update-heavy: every refresh that changes a row writes a new row
version. `pg_tviews_profile()` reports per TVIEW the HOT ratio, dead tuples, last
(auto)vacuum and warnings.
```sql
SELECT entity, tview, persistence, rows_estimate,
       pg_size_pretty(heap_bytes) AS heap, pg_size_pretty(index_bytes) AS indexes,
       fillfactor, round(hot_ratio::numeric, 2) AS hot_ratio,
       n_dead_tup, last_vacuum, last_autovacuum, warnings
FROM tviews.pg_tviews_profile()
ORDER BY n_dead_tup DESC;
```
A low HOT ratio usually means an index on a column that changes, or a fillfactor of
100; new TVIEWs get `pg_tviews.fillfactor` (default 85).

### Step 3: Vacuum and analyze TVIEW tables
Autovacuum normally handles this. To run it by hand for every registered TVIEW:
```sql
SELECT format('VACUUM (ANALYZE) %I.%I', schema, name)
FROM tviews.registry
ORDER BY schema, name
\gexec
```
For a single busy TVIEW, per-table autovacuum settings are often better than manual runs:
```sql
ALTER TABLE public.tv_post SET (autovacuum_vacuum_scale_factor = 0.05,
                                autovacuum_analyze_scale_factor = 0.05);
```

### Step 4: Indexes
Unused indexes and missing propagation indexes (indexes on the foreign-key columns
cascades look up) are listed by `pg_tviews_profile()`:
```sql
SELECT entity, unused_indexes, missing_propagation_indexes
FROM tviews.pg_tviews_profile()
WHERE cardinality(unused_indexes) > 0 OR cardinality(missing_propagation_indexes) > 0;
```
Show the indexes that would be created, then create them:
```sql
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes();
```

### Step 5: Registrations that need refreshing
After an extension update some TVIEWs may need re-registration:
```sql
SELECT schema, name FROM tviews.registry WHERE needs_reregister;
SELECT * FROM tviews.pg_tviews_reregister_all();
```

## Monthly Maintenance

### Bloat
If a TVIEW table stays bloated after vacuum, rewrite it. `VACUUM FULL` takes an
`ACCESS EXCLUSIVE` lock: writers to its base tables and readers of the TVIEW block
until it finishes.
```sql
VACUUM (FULL, ANALYZE) public.tv_post;
```
Alternatively rebuild the TVIEW contents from its view (this rewrites rows, it does not
shrink the file by itself):
```sql
SELECT tviews.pg_tviews_refresh('post');
```

### Reindex
```sql
REINDEX TABLE CONCURRENTLY public.tv_post;
```

### Consistency spot check
A TVIEW should equal its backing view. For one TVIEW (`data` compared as jsonb):
```sql
SELECT count(*) AS differing_rows
FROM (
    (SELECT pk_post, data FROM public.v_post EXCEPT SELECT pk_post, data FROM public.tv_post)
    UNION ALL
    (SELECT pk_post, data FROM public.tv_post EXCEPT SELECT pk_post, data FROM public.v_post)
) d;
```
A non-zero count means the TVIEW drifted (for example writes made while triggers were
suspended and never resumed). Repair with `SELECT tviews.pg_tviews_refresh('post');`
and see [Refresh Troubleshooting](../02-refresh-operations/refresh-troubleshooting.md).

### Settings review
```sql
SELECT name, setting, context
FROM pg_settings
WHERE name LIKE 'pg_tviews.%'
ORDER BY name;
```
The settings are described in the README (GUC table).

## Troubleshooting

### Vacuum does not remove dead tuples
A long-running or idle-in-transaction session holds back the xmin horizon. Find it with
the queries in [Connection Management](connection-management.md).

### Writes to base tables slow down
Check fan-out and missing propagation indexes (Step 4, and the `fanout` column of
`pg_tviews_profile()`). See [Performance Monitoring](../01-health-monitoring/performance-monitoring.md).

## Related Runbooks
- [Table Analysis](table-analysis.md)
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md)
- [Connection Management](connection-management.md)
- [Emergency Procedures](../04-incident-response/emergency-procedures.md)
