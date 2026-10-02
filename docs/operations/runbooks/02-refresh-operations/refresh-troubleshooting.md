# Refresh Troubleshooting Runbook

## Purpose
Diagnose writes that fail because of TVIEW refresh, TVIEWs whose rows are stale,
and refreshes that are slow.

## How refresh works (what can go wrong)
Triggers on the base tables queue TVIEW keys in memory during the writing
transaction; the queue is flushed at the end of each statement and on `COMMIT`.
There is no queue table, no background job and no record of refresh errors. So:

- A refresh **error** surfaces as an error of the writing statement, which rolls
  back. Nothing is left half-applied, and nothing records the error except the
  client and the PostgreSQL log.
- A TVIEW can only be **stale** if a write was not mapped to its keys: refresh
  suspended in that session, a table no cascade reaches, triggers missing after an
  upgrade, or a write outside the triggers.

## Initial Assessment (5 minutes)

```sql
-- Problems pg_tviews can detect (triggers, catalog, re-registration, jsonb_delta)
SELECT component, severity, message
FROM tviews.pg_tviews_health_check()
WHERE severity <> 'info';

-- How writes to each base table reach each TVIEW
SELECT name, base_tables, cascade_kinds, uncascaded_tables, uncascaded_policy,
       needs_reregister
FROM tviews.registry
ORDER BY name;
```

For the full picture run
[refresh-status.sql](../scripts/refresh-status.sql).

## Issue 1: A write fails with a refresh error

**Symptoms**: an `INSERT`/`UPDATE`/`DELETE` on a base table, or a `COMMIT`, fails
with an error whose `CONTEXT` is a statement on `v_<entity>` / `tv_<entity>`, or with
`TVIEW refresh failed before COMMIT: ...`.

**Diagnosis**: the error is the view query failing on the new data (for example
`division by zero`, a cast error, a uniqueness violation on the TVIEW key), or a
limit:

| Message | Cause | Action |
|---------|-------|--------|
| error from the view's expressions | The definition fails on the new rows | Fix the data, or the definition with `pg_tviews_create_or_replace` |
| `refresh queue backpressure: queue size (...) would exceed max_queue_size (...)` | One statement queued more keys than `pg_tviews.max_queue_size` | Split the statement, or raise the GUC for the session ([Batch Refresh](batch-refresh.md)) |
| `permission denied ...` | The TVIEW owner cannot read a table the view reads | Grant the owner access |
| lock timeout / deadlock on `tv_*` | Concurrent writers refreshing the same TVIEW rows | Retry; keep transactions short |

Reproduce outside the write by querying the view for the affected key:

```sql
SELECT * FROM public.v_user WHERE pk_user = 1;
```

To unblock writers while the cause is fixed, a session can suspend refresh
([emergency-disable.sql](../scripts/emergency-disable.sql)); refresh the TVIEWs
afterwards.

## Issue 2: Stale rows

**Symptoms**: a TVIEW row does not match its view.

**Diagnosis**: compare the TVIEW with its view. For every TVIEW, run step 4 of
[post-upgrade-validation.sql](../../upgrade/scripts/post-upgrade-validation.sql);
for one TVIEW:

```sql
(SELECT 'only in tv' AS side, pk_user, data FROM public.tv_user
 EXCEPT SELECT 'only in tv', pk_user, data FROM public.v_user)
UNION ALL
(SELECT 'only in view', pk_user, data FROM public.v_user
 EXCEPT SELECT 'only in view', pk_user, data FROM public.tv_user);
```

Then find why the write was not mapped:

1. **Tables no cascade reaches**: `uncascaded_tables` lists base tables whose writes
   cannot be mapped to TVIEW keys. Under `uncascaded_policy = 'warn'` writes to them
   leave the TVIEW stale. Rewrite the definition so the table joins on a column
   pg_tviews can trace, or recreate the TVIEW with
   `pg_tviews.uncascaded_policy = 'full_refresh'` (see
   [DDL reference](../../../reference/ddl.md#tables-no-cascade-reaches)).
   ```sql
   SELECT name, uncascaded_tables, uncascaded_policy
   FROM tviews.registry WHERE cardinality(uncascaded_tables) > 0;
   ```
2. **Re-registration pending**: after an extension upgrade, TVIEWs with
   `needs_reregister` may lack the current triggers.
   ```sql
   SELECT schema, name FROM tviews.registry WHERE needs_reregister;
   SELECT * FROM tviews.pg_tviews_reregister_all();
   ```
3. **Suspended writes**: a session that wrote with `pg_tviews.suspend_triggers = on`
   (records nothing), or committed implicitly while suspended (logs a `WARNING`
   naming the stale TVIEWs).
4. **How a table's writes map to keys**: `pg_tviews_mapping_query` returns the query
   that turns a statement's changed rows of a `mapped` table into TVIEW keys (empty
   when the key is read off the row). Run its plan to see whether it finds the keys
   you expect:
   ```sql
   SELECT tviews.pg_tviews_mapping_query('tv_post', 'tb_user'::regclass);
   ```
5. **Triggers disabled** on a base table (`ALTER TABLE ... DISABLE TRIGGER`, or
   `session_replication_role = replica` during a load):
   ```sql
   SELECT tgrelid::regclass AS base_table, tgname, tgenabled
   FROM pg_trigger WHERE tgname LIKE 'trg_tview_%' AND tgenabled = 'D';
   ```

**Fix the rows**: `SELECT tviews.pg_tviews_refresh('user');`, then the TVIEWs that
embed it, or `SELECT tviews.pg_tviews_refresh_all();` ([Manual Refresh](manual-refresh.md)).

## Issue 3: Slow writes (slow refresh)

**Symptoms**: writes to base tables take longer since TVIEWs were added.

**Diagnosis**:

```sql
-- Counters of this session (compare before/after one write, in the same session)
SELECT tviews.pg_tviews_queue_stats();

-- Plan of one key's refresh
EXPLAIN ANALYZE SELECT * FROM public.v_post WHERE pk_post = 1;

-- Physical state: HOT ratio, dead tuples, unused/missing indexes, fan-out, warnings
SELECT entity, hot_ratio, n_dead_tup, unused_indexes, missing_propagation_indexes,
       fanout, warnings
FROM tviews.pg_tviews_profile('post');

-- Indexes the propagation from embedded TVIEWs needs
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);
```

With `pg_stat_statements`, the refresh statements on `v_<entity>` / `tv_<entity>`
show their mean time and calls.

**Solutions**: add the indexes the view's joins and the mapping queries need
(`pg_tviews_ensure_propagation_indexes()` without `dry_run` creates the propagation
ones); `VACUUM ANALYZE` the TVIEW if dead tuples are high; reduce fan-out (one base
row embedded in very many TVIEW rows).

## Issue 4: TVIEW not found

**Symptoms**: `TVIEW metadata not found for entity '...'`.

Functions take the entity without the `tv_` prefix (`pg_tviews_refresh('user')`),
except `pg_tviews_reregister`, `pg_tviews_drop` and `pg_tviews_mapping_query`,
which take the TVIEW name. List the names:

```sql
SELECT schema, name, entity FROM tviews.registry ORDER BY schema, name;
```

## Logging
Log slow statements to catch slow refreshes in context:

```sql
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- ms
SELECT pg_reload_conf();
```

## Related Runbooks
- [TVIEW Health Check](../01-health-monitoring/tview-health-check.md) - Overall health
- [Manual Refresh](manual-refresh.md) - Refresh one TVIEW
- [Batch Refresh](batch-refresh.md) - Bulk writes and suspension
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Refresh cost
- [Incident Checklist](../04-incident-response/incident-checklist.md) - Incident response
