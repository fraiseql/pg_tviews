# pg_tviews Debugging Guide

Systematic troubleshooting for common pg_tviews problems. Examples use the entities
`user` and `post` (`tb_user`, `v_user`, `tv_user`, ...); substitute your own.

## How refresh works (what to debug)

Row triggers on base tables record affected TVIEW keys in an in-memory queue of the
writing transaction. A statement trigger flushes the queue at the end of each statement,
and COMMIT flushes anything left. Each affected TVIEW row is either patched directly or
recomputed from the backing view `v_<entity>`. There is no queue table and no background
worker: if a refresh fails, the writing statement fails.

## Quick Diagnosis

```sql
-- Overall status: severity info | warning | error per component
SELECT * FROM tviews.pg_tviews_health_check();

-- Registered TVIEWs and how writes reach them
SELECT schema, name, base_tables, cascade_kinds, uncascaded_tables, uncascaded_policy,
       needs_reregister
FROM tviews.registry;

-- Physical health and warnings per TVIEW
SELECT entity, hot_ratio, n_dead_tup, missing_propagation_indexes, fanout, warnings
FROM tviews.pg_tviews_profile();
```

### Common Symptoms and Solutions

| Symptom | Likely Cause | Quick Fix |
|---------|-------------|-----------|
| TVIEW not refreshing | Missing triggers, or needs re-registration | `SELECT * FROM tviews.pg_tviews_reregister_all();` |
| TVIEW not refreshing for one base table | Table in `registry.uncascaded_tables` | See `docs/reference/ddl.md` (tables no cascade reaches) |
| Slow writes | No jsonb_delta, missing indexes, high fan-out | `CREATE EXTENSION jsonb_delta;` and `tviews.pg_tviews_ensure_propagation_indexes()` |
| Write fails with queue backpressure | Bulk write over `pg_tviews.max_queue_size` | Smaller transactions, or suspend/resume |
| Writes blocked | Long or prepared transactions holding locks | `pg_stat_activity`, `pg_prepared_xacts` |
| Memory errors | Large rebuilds or bulk writes | Increase `work_mem` for the session |

## Troubleshooting Flowcharts

### TVIEW Not Refreshing

```
Start: data changed in a base table, the TVIEW does not show it
    ↓
Was the writing transaction committed? (other sessions see TVIEW changes only after COMMIT)
    ├─ NO  → commit it (check pg_prepared_xacts too)
    └─ YES → continue
        ↓
Health check errors or warnings?
SELECT * FROM tviews.pg_tviews_health_check() WHERE severity <> 'info';
    ├─ triggers / reregister / catalog → SELECT * FROM tviews.pg_tviews_reregister_all();
    └─ none → continue
        ↓
Does the write reach the TVIEW?
SELECT cascade_kinds, uncascaded_tables, uncascaded_policy FROM tviews.registry WHERE entity = 'post';
    ├─ base table in uncascaded_tables → change the view, or the policy
    └─ base table in cascade_kinds → continue
        ↓
Was refresh suspended for that write?
SELECT tviews.pg_tviews_is_suspended(), current_setting('pg_tviews.suspend_triggers');
    ├─ yes → SELECT tviews.pg_tviews_refresh('post');
    └─ no  → continue
        ↓
Rebuild and compare with the view
SELECT tviews.pg_tviews_refresh('post');
    └─ if the difference comes back after later writes, report a bug with the view definition
```

### Slow Writes

```
Start: writes to base tables got slow
    ↓
jsonb_delta installed?
SELECT tviews.pg_tviews_check_jsonb_delta();
    ├─ false → CREATE EXTENSION jsonb_delta;
    └─ true  → continue
        ↓
Missing propagation indexes, high fan-out, low HOT ratio?
SELECT entity, missing_propagation_indexes, fanout, hot_ratio, warnings FROM tviews.pg_tviews_profile();
    ├─ missing indexes → SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes();
    ├─ high fan-out → one parent change rewrites many rows; reconsider the nesting
    └─ fine → continue
        ↓
Is recomputing one row expensive?
EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM v_post WHERE pk_post = 1;
    ├─ seq scans → index the join / foreign-key columns of the base tables
    └─ fast → continue
        ↓
Recomputes vs direct patches for one write (same session, before and after)
SELECT tviews.pg_tviews_queue_stats();
    └─ compare view_recomputes, direct_patches_applied, direct_patch_fallbacks
```

## Debugging Tools

### Queue

```sql
-- Keys queued in the current transaction (normally [] between statements)
SELECT tviews.pg_tviews_debug_queue();

-- Refresh counters of this session
SELECT tviews.pg_tviews_queue_stats();

-- TVIEW rows the current transaction changed, per entity
SELECT tviews.pg_tviews_flush_and_report();
```

### Triggers

```sql
-- pg_tviews triggers and the tables they are on
SELECT tgname, tgrelid::regclass AS table_name, tgfoid::regproc AS function, tgenabled
FROM pg_trigger
WHERE tgfoid::regproc::text IN ('tviews.pg_tview_trigger_handler',
                                'tviews.pg_tview_flush_trigger',
                                'tviews.pg_tview_truncate_trigger')
ORDER BY tgrelid::regclass::text, tgname;

-- Test one write end to end
BEGIN;
UPDATE tb_user SET name = name || '' WHERE pk_user = 1;
SELECT updated_at, data FROM tv_user WHERE pk_user = 1;
ROLLBACK;
```

### Dependencies

```sql
-- Chain of TVIEWs an entity depends on
SELECT * FROM tviews.pg_tviews_show_cascade_path('post');

-- Relations each TVIEW reads
SELECT entity, relid::regclass FROM tviews.pg_tview_reads ORDER BY entity;

-- The query used to map writes on a base table to TVIEW keys
SELECT tviews.pg_tviews_mapping_query('tv_post', 'tb_user'::regclass);

-- Check a SELECT before creating a TVIEW from it
SELECT tviews.pg_tviews_analyze_select('SELECT pk_user, id, jsonb_build_object(''name'', name) AS data FROM tb_user');
```

## Common Issues and Solutions

### Issue: TVIEW Creation Fails

**Debug steps**:
1. Check the SELECT on its own: `EXPLAIN SELECT ...;`
2. Required columns: `pk_<entity>` (bigint), `id` (uuid), `data` (jsonb)
3. Check permissions on the base tables: `\dp tb_post`
4. Run `SELECT tviews.pg_tviews_analyze_select('SELECT ...');`

**Create it**:
```sql
SELECT tviews.pg_tviews_create_or_replace('tv_post',
  'SELECT p.pk_post, p.id, jsonb_build_object(''title'', p.title) AS data FROM tb_post p');
```

### Issue: Automatic Refresh Not Working

Follow the "TVIEW Not Refreshing" flowchart. Repairs:

```sql
SELECT * FROM tviews.pg_tviews_reregister_all();   -- re-install triggers
SELECT tviews.pg_tviews_refresh('post');           -- bring one TVIEW up to date
```

### Issue: Slow TVIEW Reads

```sql
EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM tv_post WHERE data->>'title' = 'x';

-- Expression indexes for frequent filters
CREATE INDEX idx_tv_post_title ON tv_post ((data->>'title'));
```

`pg_tviews.data_gin_index` creates a GIN index on `data` for new TVIEWs. Every index on
`data` makes refresh updates non-HOT; check `hot_ratio` in `tviews.pg_tviews_profile()`.

### Issue: Memory Errors

**Debug steps**:
1. `SHOW work_mem;`
2. `SELECT pid, state, now() - xact_start, left(query, 80) FROM pg_stat_activity WHERE state <> 'idle';`
3. Size of the TVIEW being written: `SELECT entity, rows_estimate, heap_bytes, fanout FROM tviews.pg_tviews_profile();`

**Solutions**: raise `work_mem` for the session doing the rebuild or the bulk write;
split bulk writes into smaller transactions; for large loads suspend refresh and resume
once (see [Failure Modes](FAILURE_MODES.md)).

### Issue: Connection Poolers

The refresh queue lives in the writing transaction and is flushed before COMMIT
completes, so transaction pooling is safe. Session state that does matter:
`pg_tviews_suspend_triggers()` (ends with the transaction) and `SET pg_tviews.*`
(session GUCs; reset by `DISCARD ALL`).

### Issue: TVIEW Content Differs From Its View

```sql
(SELECT pk_post, data FROM v_post EXCEPT SELECT pk_post, data FROM tv_post)
UNION ALL
(SELECT pk_post, data FROM tv_post EXCEPT SELECT pk_post, data FROM v_post);
```

Repair with `SELECT tviews.pg_tviews_refresh('post');`. Do not `TRUNCATE` or write
`tv_*` tables directly.

## Advanced Debugging

### PostgreSQL Log

```sql
ALTER SYSTEM SET log_line_prefix = '%t [%p]: user=%u,db=%d,app=%a ';
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- slow writes, refresh included
SELECT pg_reload_conf();
```

```bash
grep -iE "pg_tviews|tview" /var/log/postgresql/postgresql-*.log
```

`SET pg_tviews.log_level = 'debug';` makes pg_tviews log more detail in one session.

### Lock Analysis

```sql
SELECT pid, pg_blocking_pids(pid) AS blocked_by, wait_event_type, wait_event,
       now() - xact_start AS xact_age, left(query, 80) AS query
FROM pg_stat_activity
WHERE cardinality(pg_blocking_pids(pid)) > 0;
```

## Emergency Procedures

### Rebuild all TVIEWs

```sql
SELECT tviews.pg_tviews_refresh_all();
```

### Drop and recreate a TVIEW

```sql
SELECT query FROM tviews.registry WHERE entity = 'post';   -- keep the definition
SELECT tviews.pg_tviews_drop('tv_post', if_exists => true);
SELECT tviews.pg_tviews_create_or_replace('tv_post', '<the saved query>');
```

`pg_tviews_drop(..., cascade => true)` also drops objects that depend on the TVIEW or its view.

## Proactive Monitoring

```bash
#!/bin/bash
psql -X -At -d mydb -c "
    SELECT component || ' ' || severity || ': ' || message
    FROM tviews.pg_tviews_health_check()
    WHERE severity <> 'info'
" > health_check.log

if [ -s health_check.log ]; then
    mail -s 'pg_tviews health check' admin@example.com < health_check.log
fi
```

## See Also

- [Error Reference](../error-reference.md) - Complete error documentation
- [API Reference](../reference/api.md) - Function documentation
- [Monitoring Guide](monitoring.md) - Health checking and metrics
- [Troubleshooting Guide](troubleshooting.md) - Production procedures
