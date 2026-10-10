# Replication and UNLOGGED TVIEWs

TVIEW tables are UNLOGGED by default (`pg_tviews.unlogged_by_default = on`). They
write no WAL, which makes refreshes cheaper, but it also means the table is not
replicated. This page sets out what that means for standbys, failover and crash
restarts, and how to choose per TVIEW.

## Contract

| Situation | LOGGED TVIEW | UNLOGGED TVIEW |
|-----------|--------------|----------------|
| Primary | read / write | read / write |
| Hot standby (physical replica) | readable, up to date with replay | **read fails**: `cannot access temporary or unlogged relations during recovery` |
| Standby after promotion | intact | **empty** (init fork) until rebuilt |
| Primary after a crash restart | intact | **empty** until rebuilt |
| Logical replication | published like any table | not published |

A standby never runs the rebuild itself: it is read-only, and the TVIEW's
contents are the primary's to maintain. The rebuild happens where writes happen,
once recovery ends.

## Choosing LOGGED or UNLOGGED

Make a TVIEW LOGGED when it is read from replicas, or when it must be complete
immediately after a failover. Keep it UNLOGGED when it is only read on the
primary and its write rate makes the WAL matter (see
`docs/benchmarks/physical-baseline-beta17.md` for measured WAL volumes).

```sql
-- New TVIEWs, for a session or a transaction
SET pg_tviews.unlogged_by_default = off;

-- An existing TVIEW. ALTER TABLE ... SET [UN]LOGGED rewrites the whole table
-- under an ACCESS EXCLUSIVE lock; the rows are kept.
SELECT pg_tviews_set_logged('post', true);
SELECT pg_tviews_set_logged('post', false);
```

## Checking what a standby can serve

```sql
SELECT pg_tviews_is_replica_readable('post');   -- true / false / NULL (unknown entity)

SELECT * FROM pg_tviews_replication_status();
--  entity | persistence | replica_readable | is_empty | needs_rebuild
```

Both functions are read-only and work on a standby. There, `is_empty` is NULL
for UNLOGGED TVIEWs (their table cannot be read) and `needs_rebuild` is NULL for
all of them. A client that routes reads to replicas (FraiseQL's
`read_replica_urls`) can call `pg_tviews_replication_status()` once at startup
and keep every type backed by a `replica_readable = false` TVIEW on the primary.

## Rebuilding after promotion, a crash or a restore

PostgreSQL leaves no trace of the reset beyond the emptied tables, so pg_tviews
keeps one: `tviews.pg_tview_valid`, itself UNLOGGED, holds a row per UNLOGGED
TVIEW whose rows can be trusted, and the reset empties it together with the
TVIEWs. A TVIEW missing from it is reported with `needs_rebuild = true`. A TVIEW
that is merely empty (its base tables have no rows, or someone ran `TRUNCATE`)
keeps its row and is never refilled behind your back. The rows are not dumped:
after a restore, each UNLOGGED TVIEW is refilled once.

Without further setup, a reset UNLOGGED TVIEW is filled on the first write that
touches it, after the TVIEWs it reads. Concurrent first writers wait for the one
that fills it (under `REPEATABLE READ` they fail with `40001` instead, and can be
retried). Until then, readers see an empty table. Two ways to close that window:

**Automatically.** List the databases in `postgresql.conf` and restart:

```ini
shared_preload_libraries = 'pg_tviews'
pg_tviews.auto_rebuild_databases = 'app, reporting'
```

For each listed database a background worker starts when the server leaves
recovery: at startup, after a crash restart, and when a standby is promoted. It
runs `pg_tviews_rebuild_all()` in that database and logs each rebuilt TVIEW,
then stays idle so that a later crash restart runs it again. A database without
the extension is skipped. The worker uses the extension's schema and `public`
as its `search_path`.

**By hand**, from deploy tooling after a failover or a restore:

```sql
SELECT * FROM pg_tviews_rebuild_all();                    -- only reset UNLOGGED TVIEWs
SELECT * FROM pg_tviews_rebuild_all(only_empty => false); -- every TVIEW
```

It returns each rebuilt entity with its row count, in rebuild order: a TVIEW
whose backing view reads another TVIEW comes after it. It refuses to run during
recovery.

## Testing

`test/replication/promote_rebuild.sh` runs the contract above against a real
`pg_basebackup` standby on a spare port: reads on the standby, promotion, and an
immediate stop and restart of the promoted node. CI runs it on every pull request.
