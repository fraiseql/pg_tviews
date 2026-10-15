# Operator guide

Running pg_tviews in production: server setup, the settings operators tune, monitoring,
recovery, maintenance and the operator role.

The SQL examples on this page run in order in a fresh database; they create a small
schema to work on.

## Requirements

- PostgreSQL 16, 17 or 18. `CREATE EXTENSION` refuses older versions.
- The library preloaded in every server that runs pg_tviews (see below).
- Optional: the `jsonb_delta` extension, which speeds up patching documents in place.
  `tviews.pg_tviews_check_jsonb_delta()` reports whether it is installed.

## Server setup

pg_tviews must be preloaded: its hooks intercept `CREATE TABLE tv_* AS`, `COMMIT`,
`DROP` and `REFRESH MATERIALIZED VIEW`, and its settings and background worker are
registered at server start. Without the preload, a `CREATE TABLE tv_* AS` the hook did
not see fails, naming the fix. Set it in `postgresql.conf` and restart:

```ini
shared_preload_libraries = 'pg_tviews'          # add to any existing list
# Databases whose emptied UNLOGGED TVIEWs are rebuilt when recovery ends
# ('*', the default: every database; '': none; restart to change)
#pg_tviews.auto_rebuild_databases = '*'
```

Then, in each database:

```sql
CREATE EXTENSION jsonb_delta;   -- optional
CREATE EXTENSION pg_tviews;     -- objects go to schema tviews
SHOW shared_preload_libraries;
SELECT tviews.pg_tviews_version();
```

To call the functions unqualified, add `tviews` to the database's `search_path`
(`ALTER DATABASE app SET search_path = "$user", public, tviews;`). This page qualifies
them.

The example schema used below:

```sql
CREATE TABLE tb_user (
    pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id      uuid NOT NULL DEFAULT gen_random_uuid(),
    name    text NOT NULL
);
INSERT INTO tb_user (name) VALUES ('Alice'), ('Bob');
CREATE TABLE tv_user AS
SELECT pk_user, id, jsonb_build_object('id', id, 'name', name) AS data FROM tb_user;
```

## Settings operators tune

Settings tune the server and sessions; what a TVIEW is (LOGGED or UNLOGGED, fillfactor,
uncascaded policy, …) comes only from its definition and its options. Every setting,
with its type and default, is in the
[README's configuration table](../../README.md#configuration); the
[API reference](../reference/api.md#configuration) says when each is read. The ones
that matter in operations:

| Setting | Default | Who sets it | When to change it |
|---|---|---|---|
| `pg_tviews.auto_rebuild_databases` | `*` | postmaster | `*` refills the reset UNLOGGED TVIEWs of every database after a crash restart or promotion; a list restricts it to those databases; empty disables it (restart required) |
| `pg_tviews.max_queue_size` | `10000` | superuser | raise for transactions that queue more refreshes (else `54000`) |
| `pg_tviews.max_propagation_depth` | `100` | superuser | raise for very deep embed chains (else `54001`) |
| `pg_tviews.max_dependency_depth` | `10` | superuser | raise for deeper view-on-view hierarchies |
| `pg_tviews.lock_escalation_threshold` | `64` | superuser | value locks per relation before a transaction locks the relation instead |
| `pg_tviews.audit_enabled` | `off` | superuser | record creates, drops and refreshes in `tviews.pg_tview_audit_log` |
| `pg_tviews.batch_size` | `1000` | user | keys per bulk-refresh statement |
| `pg_tviews.cache_size` | `10000` | user | entries per in-memory metadata cache, per backend |
| `pg_tviews.report_max_tracked` | `10000` | user | changed rows journaled per transaction for `pg_tviews_flush_and_report()` |

Superuser settings decide whether a write or a creation succeeds, so they are the same
for every session unless a superuser changes them: in `postgresql.conf`, with
`ALTER SYSTEM`, or per role or database with `ALTER ROLE … SET` /
`ALTER DATABASE … SET`. User settings can be set by any session:

```sql
SET pg_tviews.batch_size = 5000;
RESET pg_tviews.batch_size;
```

Diagnostics are `DEBUG1` messages: `SET client_min_messages = debug1` shows them in a
session, `log_min_messages = debug1` in the server log.

## Connection poolers

pg_tviews keeps its refresh queue per transaction, so PgBouncer and Pgpool-II work in
transaction pooling mode. Keep a bulk load that suspends refresh
(`pg_tviews_suspend_triggers()` … `pg_tviews_resume_triggers()`) inside one transaction.
`pg_tviews_queue_stats()` reports the counters of the backend it runs on: behind a
pooler, read it in the same transaction as the writes. `tviews.stats` holds the
per-TVIEW counters of every session.

## Monitoring

```sql
-- Anything not OK: catalog, plans, triggers, TVIEWs to re-register
SELECT component, status, message FROM tviews.pg_tviews_health_check() WHERE status <> 'OK';

-- Refresh work per TVIEW, from any session, cumulative since server start or reset
SELECT entity, view_recomputes, noop_skipped, rows_written, full_refreshes, refresh_ms
FROM tviews.stats ORDER BY refresh_ms DESC NULLS LAST;

-- Physical health: size, HOT ratio, bloat, missing propagation indexes, warnings
SELECT entity, persistence, heap_bytes, hot_ratio, n_dead_tup,
       missing_propagation_indexes, warnings
FROM tviews.pg_tviews_profile();

-- TVIEWs a standby cannot serve, or that need a rebuild
SELECT * FROM tviews.pg_tviews_replication_status() WHERE needs_rebuild OR NOT replica_readable;
```

Alert on any health row whose `status` is not `OK`, on `needs_rebuild`, and on
`tviews.registry.needs_reregister`. `tviews.stats` needs
`shared_preload_libraries = 'pg_tviews'`; a TVIEW whose `untracked` is true found the
shared table (4096 TVIEWs per cluster) full and has NULL counters.
`tviews.pg_tviews_stats_reset(tview)` zeroes one TVIEW's counters, or every TVIEW's
without an argument (an operator function, see below). See [Monitoring](../operations/monitoring.md) for
thresholds and exporters.

## Backup, replication and recovery

- `pg_dump` dumps the TVIEW registrations with the extension, and the backing views
  after the tables they read; see [Upgrades](../operations/upgrades.md).
- TVIEWs are LOGGED by default and replicate like any table. A TVIEW declared
  `logged: false` (or created with `CREATE UNLOGGED TABLE tv_x AS`) is not written to
  WAL: a standby cannot read it, and a crash restart, a promotion or a physical
  restore leaves it empty. Switch one back with the `logged` option of
  `pg_tviews_create_or_replace()` or `ALTER TABLE tv_x SET LOGGED`; both fill a reset
  TVIEW first.
- After a crash restart or a promotion, a launcher worker refills reset UNLOGGED
  TVIEWs in every database (`pg_tviews.auto_rebuild_databases`). Refill them by hand
  after a restore, or where the worker is disabled:

```sql
SELECT * FROM tviews.pg_tviews_rebuild_all();                     -- emptied UNLOGGED TVIEWs
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => false);  -- every TVIEW
```

Details: [Replication](../operations/replication.md),
[Disaster recovery](../operations/disaster-recovery.md).

## Upgrades

Install the new package, restart, then in each database run
`ALTER EXTENSION pg_tviews UPDATE;` and `SELECT * FROM tviews.pg_tviews_reregister_all();`.
Until the `UPDATE` runs, writes to base tables fail rather than be served by a
mismatched library. The full procedure is in [Upgrades](../operations/upgrades.md).

## Maintenance

```sql
-- Registered TVIEWs: tables, backing views, and whether they need re-registering
SELECT entity, schema, name, view, needs_reregister FROM tviews.registry ORDER BY entity;

-- pg_tviews triggers on base tables, with the entity each one serves
SELECT t.tgrelid::regclass AS base_table, t.tgname, p.proname
FROM pg_trigger t JOIN pg_proc p ON p.oid = t.tgfoid
WHERE p.pronamespace = 'tviews'::regnamespace AND NOT t.tgisinternal
ORDER BY 1, 2;

-- Propagation indexes a cascade needs, reported without being built
SELECT * FROM tviews.pg_tviews_ensure_propagation_indexes(dry_run => true);

-- Autovacuum for a frequently refreshed TVIEW
ALTER TABLE tv_user SET (autovacuum_vacuum_scale_factor = 0.05);
ANALYZE tv_user;
```

`pg_tviews_ensure_propagation_indexes()` without `dry_run` builds what is missing; on
large tables run the reported statements with `CREATE INDEX CONCURRENTLY` instead.
`REINDEX … CONCURRENTLY` and `VACUUM` work on `tv_*` tables as on any table.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| A TVIEW differs from its view | a change the triggers did not see (`session_replication_role = replica`, disabled triggers) | `SELECT tviews.pg_tviews_refresh('<entity>');` |
| A write fails naming a TVIEW, hint `pg_tviews_reregister` | its stored plan does not decode or no longer matches the tables | `SELECT tviews.pg_tviews_reregister('<entity>');` |
| `COMMIT` fails with `55000`, refresh work still queued | a missing or disabled flush trigger | `pg_tviews_reregister`, then check `pg_tviews_health_check()` |
| `54000` refresh queue full | one transaction queued more than `pg_tviews.max_queue_size` | raise it, or write in smaller transactions |
| `42501` must be owner of TVIEW | the role does not own the TVIEW | run as its owner (see below) |
| An UNLOGGED TVIEW is empty | crash restart, promotion or restore | `pg_tviews_rebuild_all()` |

```sql
SELECT tviews.pg_tviews_refresh('user');
```

More in [Troubleshooting](../operations/troubleshooting.md).

## Security

Readers need only `SELECT` on the `tv_*` tables; writers need privileges on the base
tables only, since refreshes run as each TVIEW's owner. `pg_tviews_health_check()` and
the other read-only functions are executable by everyone.

```sql
CREATE ROLE ops_doc_reader;
GRANT SELECT ON tv_user TO ops_doc_reader;

-- An application role owning the TVIEW; refreshes now run as it
CREATE ROLE ops_doc_app;
GRANT SELECT ON tb_user TO ops_doc_app;
ALTER TABLE tv_user OWNER TO ops_doc_app;
```

### Operator role

The functions that act on every TVIEW are not executable by `PUBLIC`: only superusers,
the extension's owner and the roles granted them may run them.

| Function | What it does |
|---|---|
| `pg_tviews_refresh_all()` | Rebuild every TVIEW |
| `pg_tviews_rebuild_all(only_empty)` | Rebuild every (emptied) TVIEW, e.g. after a restore |
| `pg_tviews_reregister_all(strict)` | Re-derive every TVIEW's plan and triggers |
| `pg_tviews_ensure_propagation_indexes(tview, dry_run)` | Create missing propagation indexes |
| `pg_tviews_stats_reset(tview)` | Zero the counters in `tviews.stats` |
| `pg_tviews_invalidate_caches(relid)` | Internal: invalidate cached metadata |

A deploy or restore tool that runs as a non-superuser role gets them with a grant (the
extension lives in schema `tviews`). Bulk rebuilds run each TVIEW's backing view as that
TVIEW's owner, never as the caller, so the grant alone is enough for
`pg_tviews_refresh_all()`.
`pg_tviews_rebuild_all()` also reads each TVIEW as the caller, to find the empty ones
and count the rows it filled, so the role needs `SELECT` on the `tv_*` tables too.

Every function acting on one TVIEW (`pg_tviews_refresh`, `pg_tviews_reregister`,
`pg_tviews_drop`, `pg_tviews_create_or_replace`, …) requires owning it, or being a member of its owner
or of the extension's owner, whoever may execute it; anyone else gets SQLSTATE `42501`.
`pg_tviews_reregister_all()` and
`pg_tviews_ensure_propagation_indexes()` check this for each TVIEW too. A deploy role
that runs them is simplest made a member of the role owning the TVIEWs.

```sql
CREATE ROLE ops_doc_deploy;
GRANT EXECUTE ON FUNCTION
    tviews.pg_tviews_refresh_all(),
    tviews.pg_tviews_rebuild_all(boolean),
    tviews.pg_tviews_reregister_all(boolean),
    tviews.pg_tviews_ensure_propagation_indexes(text, boolean),
    tviews.pg_tviews_stats_reset(text)
TO ops_doc_deploy;

SET ROLE ops_doc_deploy;
SELECT tviews.pg_tviews_refresh_all();            -- allowed: runs as each owner
SELECT * FROM tviews.pg_tviews_reregister_all();  -- status: must be owner of TVIEW tv_user
RESET ROLE;

GRANT ops_doc_app TO ops_doc_deploy;              -- member of the role owning the TVIEWs
SET ROLE ops_doc_deploy;
SELECT * FROM tviews.pg_tviews_reregister_all();  -- status: reregistered
SELECT * FROM tviews.pg_tviews_rebuild_all(only_empty => false);
RESET ROLE;
```

### Audit log

With `pg_tviews.audit_enabled = on`, creates, drops and refreshes are recorded in
`tviews.pg_tview_audit_log` (operation, entity, role, time, transaction, client).

```sql
SELECT operation, entity, performed_by, performed_at
FROM tviews.pg_tview_audit_log ORDER BY performed_at DESC LIMIT 20;
```

The roles created on this page are cluster-wide; drop them:

```sql
DROP TABLE tv_user;
DROP OWNED BY ops_doc_deploy, ops_doc_reader, ops_doc_app;
DROP ROLE ops_doc_deploy, ops_doc_reader, ops_doc_app;
```

## See also

- [Installation](../getting-started/installation.md)
- [API reference](../reference/api.md)
- [Monitoring](../operations/monitoring.md)
- [Replication](../operations/replication.md)
- [Troubleshooting](../operations/troubleshooting.md)
- [Performance tuning](../operations/performance-tuning.md)
