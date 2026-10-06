# Manual Refresh Runbook

## Purpose
Bring one TVIEW back in line with its backing view by hand.

## When to Use
- **Stale rows**: a TVIEW differs from its backing view `tviews.<schema>__tv_<entity>`
- **After suspended writes**: a session wrote with `pg_tviews.suspend_triggers = on`
- **After a definition change** outside pg_tviews, or a restore
- **Empty UNLOGGED TVIEW** after a crash or on a promoted standby

In normal operation no manual refresh is needed: triggers refresh TVIEW rows in the
transaction that writes the base tables.

## Prerequisites
- `psql` access as the TVIEW owner or a superuser
- The entity name: the TVIEW name without `tv_` (`tv_user` -> `user`)

## Single TVIEW Refresh (5 minutes)

### Step 1: Identify the TVIEW
```sql
SELECT schema, name, entity, view, base_tables, needs_reregister
FROM tviews.registry
WHERE name LIKE '%user%';
```

If `needs_reregister` is true, re-register first:
`SELECT tviews.pg_tviews_reregister('tv_user');`

### Step 2: Check how far it differs
```sql
SELECT count(*) AS rows_differing FROM (
    (SELECT pk_user, data FROM public.tv_user EXCEPT SELECT pk_user, data FROM public.v_user)
    UNION ALL
    (SELECT pk_user, data FROM public.v_user EXCEPT SELECT pk_user, data FROM public.tv_user)
) d;
```

### Step 3: Refresh
```sql
SELECT tviews.pg_tviews_refresh('user');
```

The refresh rebuilds `tv_user` from its view (`TRUNCATE`, then `INSERT … SELECT`),
then every TVIEW whose view reads it, directly or through others (`tv_post` embedding
the user, `tv_feed` embedding the post), dependencies first. Each rebuilt TVIEW is
locked ACCESS EXCLUSIVE until the transaction ends, so readers wait. It returns
nothing; an error rolls the whole refresh back.

To see what it rebuilds:

```sql
SELECT depth, entity_name FROM tviews.pg_tviews_show_cascade_path('user') ORDER BY depth;
```

To rebuild everything, dependencies first: `SELECT tviews.pg_tviews_refresh_all();`

### Step 4: Verify
Rerun Step 2 (expect 0), then check when the rows last changed:

```sql
SELECT max(updated_at) AS last_change FROM public.tv_user;
```

## Empty TVIEWs after a crash
UNLOGGED TVIEWs (the default) are emptied by a crash restart and are empty on a
standby.

```sql
SELECT * FROM tviews.pg_tviews_replication_status();           -- needs_rebuild
SELECT tviews.pg_tviews_recover_after_crash('user');           -- true if it rebuilt
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => true); -- all empty ones
```

## Large TVIEWs
A refresh recomputes the whole view in the calling transaction and holds row locks
on the rows it writes until commit. For large TVIEWs, run it at low traffic and check
the view's plan first:

```sql
EXPLAIN SELECT * FROM public.v_user;
```

## Error Handling

| Error | Cause | Action |
|-------|-------|--------|
| `TVIEW metadata not found for entity 'tv_user'` | Name passed with `tv_` | Pass the entity: `'user'` |
| `Cannot refresh: triggers are suspended` (`pg_tviews_refresh_all`) | Session is suspended | `SELECT tviews.pg_tviews_resume_triggers();` or `RESET pg_tviews.suspend_triggers;` |
| `permission denied ...` | Caller cannot write the TVIEW or read the view's tables | Run as the TVIEW owner |
| `canceling statement due to lock timeout` | Another transaction holds TVIEW rows | Find it in `pg_stat_activity` / `pg_locks`, retry later |
| Any error from the view (e.g. `division by zero`) | The view query itself fails on current data | Fix the data or the definition (`pg_tviews_create_or_replace`) |

## Related Runbooks
- [Batch Refresh](batch-refresh.md) - Bulk writes and refreshing many TVIEWs
- [Refresh Troubleshooting](refresh-troubleshooting.md) - Debug refresh issues
- [Performance Monitoring](../01-health-monitoring/performance-monitoring.md) - Refresh cost
- [Emergency Procedures](../04-incident-response/emergency-procedures.md) - Crisis actions
