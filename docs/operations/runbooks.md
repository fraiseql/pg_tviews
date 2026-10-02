# Operational Runbooks

Short procedures for common pg_tviews problems: symptoms, diagnosis, resolution.
The detailed runbooks are in [runbooks/](runbooks/README.md).

Examples use the entity `user` (`tb_user`, `v_user`, `tv_user`); substitute your own.

## Runbook 1: TVIEW Not Updating

**Symptom**: changes to base tables do not show in a `tv_*` table.

**Diagnosis**:

```sql
-- 1. Health check: triggers, catalog revision, re-registration
SELECT status, component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';
```

```sql
-- 2. Is the table registered, and do writes to it reach the TVIEW?
SELECT schema, name, base_tables, cascade_kinds, uncascaded_tables, uncascaded_policy,
       needs_reregister
FROM tviews.registry
WHERE entity = 'user';
```

A base table missing from `cascade_kinds`, or listed in `uncascaded_tables` with policy
`warn`, is a table whose writes do not refresh the TVIEW by key.

```sql
-- 3. pg_tviews triggers on the base table
SELECT tgname, tgrelid::regclass, tgfoid::regproc, tgenabled
FROM pg_trigger
WHERE tgrelid = 'tb_user'::regclass AND NOT tgisinternal;
```

Expect a row trigger (`tviews.pg_tview_trigger_handler`), a statement trigger
(`tviews.pg_tview_flush_trigger`) and a truncate trigger, with `tgenabled = 'O'`.

```sql
-- 4. Is refresh suspended in this session/transaction?
SELECT tviews.pg_tviews_is_suspended(), current_setting('pg_tviews.suspend_triggers');
```

**Resolution**:

1. Missing or disabled triggers, or `needs_reregister`:
   ```sql
   SELECT * FROM tviews.pg_tviews_reregister_all();
   ```
2. Bring the TVIEW up to date now:
   ```sql
   SELECT tviews.pg_tviews_refresh('user');
   ```
3. Tables in `uncascaded_tables`: see `docs/reference/ddl.md` (tables no cascade reaches);
   either rewrite the view so keys can be mapped, or recreate the TVIEW with
   `uncascaded_policy` `full_refresh` or `error`.

---

## Runbook 2: Slow Writes to Base Tables

**Symptom**: `INSERT`/`UPDATE`/`DELETE` on tables feeding TVIEWs got slow. Refresh runs
inside the writing statement, so its cost shows up there.

**Diagnosis**:

```sql
-- 1. jsonb_delta installed?
SELECT tviews.pg_tviews_check_jsonb_delta();
```

```sql
-- 2. Physical health, fan-out and missing indexes per TVIEW
SELECT entity, hot_ratio, n_dead_tup, missing_propagation_indexes, fanout, warnings
FROM tviews.pg_tviews_profile();
```

```sql
-- 3. Dependency chain
SELECT * FROM tviews.pg_tviews_show_cascade_path('post');
```

```sql
-- 4. Cost of recomputing one row
EXPLAIN (ANALYZE, BUFFERS) SELECT * FROM v_user WHERE pk_user = 1;
```

**Resolution**:

1. Install jsonb_delta if missing: `CREATE EXTENSION jsonb_delta;`
2. Create the indexes cascades need:
   ```sql
   SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
   SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes();
   ```
3. Index the join and foreign-key columns the view uses in its base tables.
4. Reduce fan-out or nesting depth in the view definitions.

See [Performance Monitoring](runbooks/01-health-monitoring/performance-monitoring.md).

---

## Runbook 3: Out of Memory or Queue Limit During a Bulk Write

**Symptom**: "out of memory", or
`refresh queue backpressure: queue size (...) would exceed max_queue_size (...)`.

**Diagnosis**:

```sql
SHOW work_mem;
SHOW pg_tviews.max_queue_size;
SELECT entity, rows_estimate, pg_size_pretty(heap_bytes) AS heap, fanout
FROM tviews.pg_tviews_profile()
ORDER BY heap_bytes DESC;
```

**Resolution**:

1. Write in smaller transactions (for example by key range).
2. Suspend refresh for the load and refresh once at the end:
   ```sql
   BEGIN;
   SELECT tviews.pg_tviews_suspend_triggers();
   -- bulk writes
   SELECT tviews.pg_tviews_resume_triggers();
   COMMIT;
   ```
3. Raise limits for the session if the load is legitimate:
   `SET work_mem = '256MB'; SET pg_tviews.max_queue_size = 100000;`

---

## Runbook 4: Extension Upgrade Failed

**Symptom**: `ALTER EXTENSION pg_tviews UPDATE` fails, or the health check reports a
catalog revision mismatch.

**Diagnosis**:

```sql
SELECT extversion FROM pg_extension WHERE extname = 'pg_tviews';
SELECT tviews.pg_tviews_version(), tviews.pg_tviews_catalog_revision();
SELECT component, severity, message FROM tviews.pg_tviews_health_check()
WHERE component IN ('extension', 'catalog', 'reregister');
```

**Resolution**:

1. `ALTER EXTENSION pg_tviews UPDATE` runs in one transaction: a failure leaves the old
   version in place. Fix the reported error and run it again.
2. After a successful update:
   ```sql
   SELECT * FROM tviews.pg_tviews_reregister_all();
   ```
3. Databases created with 0.1.0 use `scripts/migrate-from-0.1.0.sql`.
4. Scripts: `docs/operations/upgrade/scripts/pre-upgrade-checks.sh` and
   `post-upgrade-validation.sql`. If the problem persists, open an issue with the full
   error, the PostgreSQL version and both pg_tviews versions.

---

## Runbook 5: Orphaned Triggers After TVIEW Drop

**Symptom**: pg_tviews triggers remain on base tables after a TVIEW was dropped.

**Diagnosis**:

```sql
SELECT status, message
FROM tviews.pg_tviews_health_check()
WHERE component = 'triggers';
-- WARNING | 1 orphaned trigger found: trg_tview_row_post_on_app_tb_user on app.tb_user
```

**Resolution**:

```sql
-- An orphaned trigger: drop it
DROP TRIGGER trg_tview_row_post_on_app_tb_user ON app.tb_user;

-- Missing triggers, or triggers without an entity: re-install them
SELECT * FROM tviews.pg_tviews_reregister_all();
```

Drop TVIEWs with `SELECT tviews.pg_tviews_drop('tv_post');` (or `DROP TABLE tv_post`) so
their triggers are removed with them.

---

## Runbook 6: TVIEW Content Differs From Its View

**Symptom**: a `tv_*` row differs from what `v_*` returns.

**Diagnosis**:

```sql
-- Rows that differ, in either direction
(SELECT pk_user, data FROM v_user EXCEPT SELECT pk_user, data FROM tv_user)
UNION ALL
(SELECT pk_user, data FROM tv_user EXCEPT SELECT pk_user, data FROM v_user);
```

Then check Runbook 1 (registration, triggers, uncascaded tables).

**Resolution**:

```sql
SELECT tviews.pg_tviews_refresh('user');       -- rebuild one TVIEW from its view
SELECT tviews.pg_tviews_refresh_all();         -- all TVIEWs, dependencies first
```

A difference with healthy triggers and no uncascaded tables is a bug: please report it
with the view definition.

---

## Runbook 7: High CPU During Writes

**Symptom**: CPU spikes while base tables are written.

**Diagnosis**:

```sql
SELECT pid, state, wait_event_type, wait_event, now() - query_start AS running,
       left(query, 80) AS query
FROM pg_stat_activity
WHERE datname = current_database() AND state = 'active'
ORDER BY query_start;
```

```sql
-- Refresh counters of this session: run before and after one write and compare
SELECT tviews.pg_tviews_queue_stats();
```

A large increase in `view_recomputes` relative to `direct_patches_applied` means rows
are being rebuilt from the view; `fanout` in `tviews.pg_tviews_profile()` shows how many
rows each parent change touches.

**Resolution**:

1. Index the columns the view joins on; create propagation indexes (Runbook 2).
2. Write in bulk statements rather than many single-row statements: each statement
   refreshes each affected key once, at its end.
3. For large loads, suspend refresh and resume once (Runbook 3).

---

## Emergency Procedures

### Rebuild every TVIEW

```sql
SELECT tviews.pg_tviews_refresh_all();
```

### Rebuild TVIEWs emptied by a crash or promotion (UNLOGGED)

```sql
SELECT * FROM tviews.pg_tviews_replication_status() WHERE needs_rebuild;
SELECT * FROM tviews.pg_tviews_rebuild_all();
```

### Stop refreshing from one session

See `runbooks/scripts/emergency-disable.sql` and
[Emergency Procedures](runbooks/04-incident-response/emergency-procedures.md).

## Monitoring Integration

```bash
#!/bin/bash
# Exit non-zero when the health check reports a warning or an error.
problems=$(psql -X -At -d "$DB_NAME" -c \
  "SELECT component || ': ' || message FROM tviews.pg_tviews_health_check() WHERE severity <> 'info'")
if [ -n "$problems" ]; then
    echo "$problems"
    exit 1
fi
```

What to alert on:
- Any `pg_tviews_health_check()` row with severity `error` (critical) or `warning`
- `needs_rebuild` in `pg_tviews_replication_status()` after a failover
- Writes failing with pg_tviews errors in the server log

## See Also

- [Monitoring Guide](monitoring.md)
- [Troubleshooting Guide](troubleshooting.md)
- [Performance Tuning](performance-tuning.md)
