# Troubleshooting Guide

Common issues and solutions for pg_tviews deployment and operation. The extension's
objects live in schema `tviews`; examples use the `tb_user` / `tb_post` tables with
TVIEWs `tv_user` and `tv_post`.

## Quick Diagnosis

### Health Check

Run this first for any issue:

```sql
-- Everything pg_tviews checks: extension, jsonb_delta, catalog, metadata,
-- triggers, re-registration
SELECT * FROM tviews.pg_tviews_health_check();

-- Only problems
SELECT component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
```

[health-check.sql](runbooks/scripts/health-check.sql) and
[refresh-status.sql](runbooks/scripts/refresh-status.sql) give a fuller report.

### Refresh Queue

The refresh queue is in memory, per transaction, and is flushed at the end of each
statement and on `COMMIT`. Outside a write it is empty. Inside a transaction:

```sql
SELECT tviews.pg_tviews_debug_queue();   -- keys queued in this transaction
SELECT tviews.pg_tviews_queue_stats();   -- counters of this session
```

## Installation Issues

### Extension Not Found

**Error**: `ERROR: extension "pg_tviews" is not available` (or `could not open
extension control file`)

1. **Check the installed files and version**:
   ```sql
   SELECT name, default_version, installed_version
   FROM pg_available_extensions WHERE name IN ('pg_tviews', 'jsonb_delta');
   ```
2. **Install the build for this server's `pg_config`** (PostgreSQL 16, 17 or 18):
   ```bash
   cargo pgrx install --release --pg-config "$(which pg_config)"
   ```
3. **Preload the library** (needed for the GUCs and the `COMMIT` hook), restart, then
   create the extensions:
   ```sql
   SHOW shared_preload_libraries;           -- must include pg_tviews
   CREATE EXTENSION jsonb_delta;
   CREATE EXTENSION pg_tviews;
   ```

### Permission Denied

**Error**: `ERROR: permission denied for schema tviews` or `for function ...`

```sql
SELECT current_user, rolsuper FROM pg_roles WHERE rolname = current_user;

-- As the extension owner or a superuser
GRANT USAGE ON SCHEMA tviews TO app_user;
GRANT SELECT ON tviews.registry TO app_user;
```

Create TVIEWs as the role that should own them: refreshes run as the TVIEW owner.

## TVIEW Creation Issues

TVIEWs are created with `tviews.pg_tviews_create(name, query)`,
`tviews.pg_tviews_create_or_replace(name, query, options)`, or
`CREATE TABLE tv_<entity> AS SELECT ...`. See the
[DDL reference](../reference/ddl.md).

### Name Does Not Match the Key

**Error**: `TVIEW tv_posts does not match its definition, which is keyed on pk_post`

The TVIEW is named after its entity: `tv_<entity>` with key column `pk_<entity>`
(`tv_post` / `pk_post`). Rename the TVIEW or the key column.

### Key Column Missing or of the Wrong Type

**Error**: `column "pk_note" is of type bigint but expression is of type uuid`

The definition must select `pk_<entity>` (a bigint key from `tb_<entity>`), usually
with `id` (uuid) and `data` (jsonb):

```sql
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id, p.fk_user,
           jsonb_build_object('title', p.title) AS data
    FROM tb_post p
$$);
```

### TVIEW Already Exists

**Error**: `TVIEW tv_post already exists; pg_tviews_create_or_replace() changes an existing TVIEW`

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_post', $$ ... $$);
```

### Unsupported SQL Features

`WITH RECURSIVE` and definitions that cannot be refreshed are rejected at create time.
INTERSECT, EXCEPT and window functions are accepted, but writes to the tables they
read may not reach the TVIEW keys. UNION / UNION ALL, CTEs, subqueries, `LATERAL`
and DISTINCT ON are supported. Full list:
[Supported SQL Features](../reference/ddl.md#supported-sql-features).

### Tables No Cascade Reaches

**Warning**: `writes to public.tb_flag will not refresh public.tv_report (...)`

The definition reads a table with no condition linking its rows to the TVIEW key.
Writes to it leave the TVIEW stale under the default `warn` policy:

```sql
SELECT name, uncascaded_tables, uncascaded_policy
FROM tviews.registry WHERE cardinality(uncascaded_tables) > 0;
```

Rewrite the join, or recreate the TVIEW with
`SET pg_tviews.uncascaded_policy = 'full_refresh'` (see
[Tables no cascade reaches](../reference/ddl.md#tables-no-cascade-reaches)).

### Dependency Cycle

Circular dependencies between TVIEWs are rejected at create time. Replace one
direction with a computed field read from the base table:

```sql
SELECT tviews.pg_tviews_create('tv_post', $$
    SELECT p.pk_post, p.id,
           jsonb_build_object(
               'title', p.title,
               'comment_count', (SELECT count(*) FROM tb_comment c WHERE c.fk_post = p.pk_post)
           ) AS data
    FROM tb_post p
$$);
```

## Runtime Issues

### Writes Fail With a Refresh Error

A refresh error fails the writing statement (or the `COMMIT`), which rolls back.
The `CONTEXT` line names the statement on `v_<entity>` / `tv_<entity>`. Typical
causes: the view's expressions fail on the new data, or one statement queues more
keys than `pg_tviews.max_queue_size`. See
[Refresh Troubleshooting](runbooks/02-refresh-operations/refresh-troubleshooting.md).

### TVIEW Not Updating

**Symptoms**: changes to base tables do not appear in a TVIEW.

1. **Compare the TVIEW with its view**:
   ```sql
   SELECT count(*) AS rows_differing FROM (
       (SELECT pk_post, data FROM public.tv_post EXCEPT SELECT pk_post, data FROM public.v_post)
       UNION ALL
       (SELECT pk_post, data FROM public.v_post EXCEPT SELECT pk_post, data FROM public.tv_post)
   ) d;
   ```
2. **Check the triggers** (named `trg_tview_*` on each base table):
   ```sql
   SELECT tgrelid::regclass AS base_table, tgname, tgenabled
   FROM pg_trigger WHERE tgname LIKE 'trg_tview_%' ORDER BY 1, 2;
   ```
   Re-enable disabled ones (`ALTER TABLE tb_post ENABLE TRIGGER ALL`), and re-install
   missing ones: `SELECT * FROM tviews.pg_tviews_reregister_all();`
3. **Check how writes map to keys**:
   ```sql
   SELECT name, cascade_kinds, uncascaded_tables, needs_reregister
   FROM tviews.registry WHERE entity = 'post';
   SELECT tviews.pg_tviews_mapping_query('tv_post', 'tb_user'::regclass);
   ```
4. **Check that refresh is not suspended** in the writing session:
   ```sql
   SELECT tviews.pg_tviews_is_suspended(),
          current_setting('pg_tviews.suspend_triggers', true);
   ```
5. **Repair**: `SELECT tviews.pg_tviews_refresh('post');` (or
   `SELECT tviews.pg_tviews_refresh_all();`).

### Empty TVIEWs After a Crash or on a Standby

UNLOGGED TVIEWs (the default) are empty after a crash restart and on standbys.

```sql
SELECT * FROM tviews.pg_tviews_replication_status();
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true);
SELECT tviews.pg_tviews_set_logged('post', true);   -- readable on standbys
```

See [Replication](replication.md).

### Slow Writes

**Symptoms**: writes to base tables slowed down by TVIEW refresh.

```sql
-- Sizes, HOT ratio, dead tuples, unused/missing indexes, fan-out, warnings
SELECT * FROM tviews.pg_tviews_profile('post');

-- Missing indexes for propagation from embedded TVIEWs
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes('post', dry_run => true);

-- Plan of one key's refresh
EXPLAIN ANALYZE SELECT * FROM public.v_post WHERE pk_post = 1;

-- Session counters (compare before and after a write, in one session)
SELECT tviews.pg_tviews_queue_stats();
```

Index the columns the view joins on, create the missing propagation indexes
(`dry_run => false`), and keep the fan-out (TVIEW rows per base row) low. See
[Performance Tuning](performance-tuning.md).

### Memory Issues

**Symptoms**: out of memory during large writes or refreshes.

```sql
SHOW work_mem;
SHOW maintenance_work_mem;

-- Largest documents
SELECT pk_post, pg_column_size(data) AS size_bytes
FROM public.tv_post ORDER BY size_bytes DESC LIMIT 10;
```

Lower `pg_tviews.batch_size` (keys recomputed per refresh statement), split very
large write statements, and keep documents small (paginate large arrays).

### Lock Conflicts

**Symptoms**: deadlocks or lock waits on `tv_*` tables.

```sql
SELECT locktype, mode, granted, relation::regclass
FROM pg_locks WHERE relation::regclass::text LIKE '%tv_%';

SELECT pid, wait_event_type, wait_event, left(query, 80)
FROM pg_stat_activity WHERE wait_event_type = 'Lock';
```

Concurrent transactions that change rows embedded in the same TVIEW rows wait on
each other. Keep transactions short, and retry deadlocked transactions in the
application.

## Data Consistency Issues

### Count or Content Mismatches

A TVIEW should equal its view. Compare them (above, or step 4 of
[post-upgrade-validation.sql](upgrade/scripts/post-upgrade-validation.sql) for all
TVIEWs), then refresh:

```sql
SELECT tviews.pg_tviews_refresh('post');
```

Do not write to `tv_*` tables directly: the next refresh overwrites the rows.

## Connection Pooling Issues

Suspension (`pg_tviews_suspend_triggers()`) ends with the transaction, and the
refresh queue is per transaction, so transaction-mode pooling is safe. A session
setting such as `SET pg_tviews.suspend_triggers = on` stays on the pooled
connection: reset it (`RESET pg_tviews.suspend_triggers`) or configure
`server_reset_query = DISCARD ALL` in PgBouncer session mode.

## Advanced Troubleshooting

### Debug Logging

```sql
-- This session: pg_tviews' internal diagnostics as NOTICE
SET pg_tviews.log_level = 'debug';

-- Server: log statements slower than 1 s (refreshes run inside them)
ALTER SYSTEM SET log_min_duration_statement = 1000;
SELECT pg_reload_conf();
```

### Report of a Transaction's Changes

```sql
BEGIN;
UPDATE tb_user SET name = name WHERE pk_user = 1;
SELECT tviews.pg_tviews_flush_and_report();   -- TVIEW rows this transaction changed
ROLLBACK;
```

### Dependencies Between TVIEWs

```sql
SELECT * FROM tviews.pg_tviews_show_cascade_path('user') ORDER BY depth;
SELECT name, base_tables FROM tviews.registry;
```

### Recovery After a Crash

```sql
SELECT * FROM tviews.pg_tviews_health_check();
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true);
-- Prepared transactions carry their TVIEW refreshes: COMMIT PREPARED or
-- ROLLBACK PREPARED them
SELECT gid, prepared FROM pg_prepared_xacts;
```

## Getting Help

When reporting issues, include:

1. **Versions**:
   ```sql
   SELECT tviews.pg_tviews_version(), version(),
          (SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews');
   ```
2. **The full error text**, with `CONTEXT`
3. **The TVIEW definitions and how writes reach them**:
   ```sql
   SELECT name, query, base_tables, cascade_kinds, uncascaded_tables FROM tviews.registry;
   ```
4. **Health check output**: `SELECT * FROM tviews.pg_tviews_health_check();`
5. **Trigger status**:
   ```sql
   SELECT tgrelid::regclass, tgname, tgenabled FROM pg_trigger WHERE tgname LIKE 'trg_tview_%';
   ```

- **GitHub Issues**: [github.com/fraiseql/pg_tviews/issues](https://github.com/fraiseql/pg_tviews/issues)

## See Also

- [Monitoring Guide](monitoring.md) - Health checks and metrics
- [Performance Tuning](performance-tuning.md) - Optimization strategies
- [Runbooks](runbooks/README.md) - Operational procedures
- [Developer Guide](../user-guides/developers.md) - Application integration
