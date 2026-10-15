# Replication and UNLOGGED TVIEWs

TVIEW tables are LOGGED by default: they write WAL, survive a crash and are
replicated like any table. A TVIEW declared `logged: false` is UNLOGGED: it writes
no WAL, which makes refreshes cheaper, but it is not replicated and a crash or a
promotion empties it. This page sets out what that means for standbys, failover and
crash restarts, and how to choose per TVIEW.

## Contract

| Situation | LOGGED TVIEW (default) | UNLOGGED TVIEW |
|-----------|------------------------|----------------|
| Primary | read / write | read / write |
| Hot standby (physical replica) | readable, up to date with replay | **read fails**: `cannot access temporary or unlogged relations during recovery` |
| Standby after promotion | intact | **empty** (init fork) until refilled |
| Primary after a crash restart | intact | **empty** until refilled |
| Logical replication | published like any table | not published |

A standby never refills a TVIEW itself: it is read-only, and the TVIEW's contents
are the primary's to maintain. The refill happens where writes happen, once
recovery ends.

## Choosing LOGGED or UNLOGGED

Keep a TVIEW LOGGED when it is read from replicas, or when it must be complete
immediately after a failover. Make it UNLOGGED when it is only read on the primary,
its rows can be recomputed, and its write rate makes the WAL matter (see
`docs/benchmarks/physical-baseline-beta17.md` for measured WAL volumes).

The `logged` option declares it, at creation or later:

```sql
-- A new UNLOGGED TVIEW
SELECT pg_tviews_create('post', 'SELECT ... FROM v_post', '{"logged": false}');

-- The same, by DDL: CREATE UNLOGGED TABLE ... AS makes an UNLOGGED TVIEW,
-- plain CREATE TABLE ... AS a LOGGED one
CREATE UNLOGGED TABLE tv_post AS SELECT ... FROM v_post;

-- An existing TVIEW. The options passed are the whole declaration, so start
-- from the ones it has.
SELECT pg_tviews_create_or_replace(format('%I.%I', schema, name), query,
                                   options || '{"logged": false}')
FROM tviews.registry WHERE entity = 'post';

-- Or by DDL
ALTER TABLE tv_post SET LOGGED;
ALTER TABLE tv_post SET UNLOGGED;
```

Either way of switching rewrites the whole table under an `ACCESS EXCLUSIVE` lock;
the rows are kept. A reset UNLOGGED TVIEW switched to LOGGED is filled first,
whichever way it is switched: a LOGGED table is never checked again.

## Checking what a standby can serve

```sql
SELECT * FROM pg_tviews_replication_status();
--  entity | persistence | replica_readable | is_empty | needs_rebuild

SELECT entity, options->'logged' AS logged FROM tviews.registry;
```

Both are read-only and work on a standby. There, `is_empty` is NULL for UNLOGGED
TVIEWs (their table cannot be read) and `needs_rebuild` is NULL for all of them.
A client that routes reads to replicas (FraiseQL's `read_replica_urls`) can call
`pg_tviews_replication_status()` once at startup and keep every type backed by a
`replica_readable = false` TVIEW on the primary.

## Refilling after promotion, a crash or a restore

PostgreSQL leaves no trace of the reset beyond the emptied tables, so pg_tviews
keeps one: `tviews.pg_tview_valid`, itself UNLOGGED, holds a row per UNLOGGED
TVIEW whose rows can be trusted, and the reset empties it together with the
TVIEWs. A TVIEW missing from it is reported with `needs_rebuild = true`. A TVIEW
that is merely empty (its base tables have no rows, or someone ran `TRUNCATE`)
keeps its row and is never refilled behind your back. The rows are not dumped:
after a restore, each UNLOGGED TVIEW is refilled once.

### The launcher

With `shared_preload_libraries = 'pg_tviews'`, a launcher worker starts when the
server leaves recovery: at startup, after a crash restart, and when a standby is
promoted. It starts one worker per database, one after the other. Each worker runs
`pg_tviews_rebuild_all()` in its database, logs each refilled TVIEW and exits; the
launcher then idles, so that a later crash restart runs it again. A database
without the extension is skipped quietly. The worker uses the extension's schema
and `public` as its `search_path`.

`pg_tviews.auto_rebuild_databases` chooses the databases. It is read at server
start; changing it needs a restart.

```ini
shared_preload_libraries = 'pg_tviews'
pg_tviews.auto_rebuild_databases = '*'              # the default: every database that accepts connections
# pg_tviews.auto_rebuild_databases = 'app, reporting'  # only these
# pg_tviews.auto_rebuild_databases = ''                # none: no launcher
```

### Without the launcher

A reset UNLOGGED TVIEW is filled on the first write that touches it, after the
TVIEWs it reads. Concurrent first writers wait for the one that fills it (under
`REPEATABLE READ` they fail with `40001` instead, and can be retried). Until then,
readers see an empty table.

A transaction that filled one cannot be prepared: `PREPARE TRANSACTION` fails with
`25000`, because its claim would make every writer of the TVIEW wait for
`COMMIT PREPARED`. Fill it in a transaction of its own first.

From deploy tooling after a failover or a restore:

```sql
SELECT * FROM pg_tviews_rebuild_all();                    -- only reset UNLOGGED TVIEWs
SELECT * FROM pg_tviews_rebuild_all(only_empty => false); -- every TVIEW
```

It returns each rebuilt entity with its row count, in rebuild order: a TVIEW whose
backing view reads another TVIEW comes after it. It refuses to run during recovery.
Its `EXECUTE` is revoked from `PUBLIC`.

### Cost of the first write after a reset

Without the launcher, the first write pays for the refill. Measured on a debug build
of PostgreSQL 18 with assertions, so these are upper bounds; `tv_post` embeds
`tv_user`, both UNLOGGED, and one write to a post refills both:

| Posts / users | First write | Next write |
|---------------|-------------|------------|
| 10k / 1k | 186 ms | ~1-2 ms |
| 100k / 10k | 1.8 s | ~1-2 ms |
| 1M / 100k | 35.9 s | ~1-2 ms |

The work scales with the size of the TVIEW and of the TVIEWs it reads, which are
filled first.

## Testing

`test/replication/promote_rebuild.sh` runs the contract above against a real
`pg_basebackup` standby on a spare port: reads on the standby, promotion, and an
immediate stop and restart of the promoted node. CI runs it on every pull request.
