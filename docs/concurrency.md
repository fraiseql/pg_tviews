# Concurrency

How TVIEW refreshes behave when several transactions write at once: when the refresh
runs, what it guarantees under each isolation level, what it locks, and what to retry.
The design is [ADR 0207](adr/0207-concurrent-maintenance.md).

## When a TVIEW is refreshed

A write to a base table is refreshed **inside the writer's transaction**:

- at the end of each statement (a statement-level flush trigger), so an autocommit
  statement is refreshed before it returns. A statement run from a trigger of a write
  to a TVIEW's base table (a trigger that writes its own table, level by level) leaves
  its work to that write, which refreshes once at its end: each TVIEW row is computed
  once, from the final state. A query that only reads defers nothing: in `SELECT f()`,
  each statement of `f` that writes is refreshed before the next one reads;
- before `COMMIT` and before `PREPARE TRANSACTION` (the `ProcessUtility` hook), so the
  refresh writes belong to the transaction.

The refresh writes are ordinary row writes to `tv_<entity>`: other sessions see them
when the writer commits, exactly like its base-table writes, and a rollback discards
both. The refresh runs as the TVIEW's owner with `search_path = pg_catalog, pg_temp`,
whoever the writer is, and renders values under fixed settings (see
[Rendering](reference/ddl.md#rendering)).

Nothing is queued across transactions. A transaction that would commit with refresh
work still queued (a flush trigger dropped or disabled) fails instead, with SQLSTATE
`55000`, naming the TVIEWs; `pg_tviews_health_check()` reports the missing trigger.

## Guarantees

Two transactions can write rows that feed the same TVIEW row at the same time: one
renames a user while another inserts a post whose `tv_post` row embeds that user, or
re-points a post to that user, or inserts the first order of a user into an aggregate
TVIEW. Whatever the order of their statements and commits:

| Isolation level | Outcome |
|---|---|
| `READ COMMITTED` (PostgreSQL's default) | every TVIEW row equals its backing view once both have committed. The second transaction waits for the first, then computes from a snapshot that sees its commit. |
| `REPEATABLE READ` | every TVIEW row equals its backing view, or one transaction fails with `40001` (retry it). A snapshot can't see what committed after it, however long it waits, so such a transaction never waits for these locks, and checks what it refreshed and what it found against the latest snapshot, as PostgreSQL's foreign-key checks do. |
| `SERIALIZABLE` | every TVIEW row equals its backing view, or one transaction fails with `40001` (retry it). PostgreSQL's serializable snapshot isolation detects the conflict; pg_tviews takes none of the locks below. |

`REPEATABLE READ` costs more: each refreshed row is computed a second time under the
latest snapshot, and under contention on the same values many transactions fail and
must be retried (measured: about a fifth fewer transactions per second than `READ
COMMITTED` with writes spread over 1,000 users; on 10 hot users, most renames
conflict). Prefer `READ COMMITTED` for transactions that write TVIEW base tables, and
retry with a backoff.

This holds for every way a definition reads another row: an embedded TVIEW, a direct
join, a fan-out patch, a join several hops away, an outer join to a row not inserted
yet, a table under the `full_refresh` policy, a child carrying the key in its own row,
a new group of an aggregate TVIEW. Each case is an isolation spec in
`test/isolation/specs/`.

## What waits for what

A write waits for an open transaction that **links rows to the same value**, and the
reverse:

- inserting a post of user 1, or pointing a post at user 1, waits for an open rename
  of user 1, and a rename of user 1 waits for an open insert of a post of user 1;
- renaming an organisation waits for an open insert of a post whose author belongs
  to it (two hops), and the reverse;
- two transactions creating the same TVIEW row (the first rows of one aggregate group,
  a comment naming a post being inserted) wait for each other;
- a write refreshing a whole TVIEW (`TRUNCATE` of a base table, a table under the
  `full_refresh` policy, `pg_tviews_refresh()`) waits for every open refresh of that
  TVIEW and every open write to the tables it reads, and they wait for it.

Writes to unrelated values don't wait for each other. Readers are never blocked,
except by `pg_tviews_refresh()` (`ACCESS EXCLUSIVE`).

Like any workload that locks, two transactions taking these locks in opposite orders
can deadlock: PostgreSQL detects it and fails one of them with `deadlock detected`
(SQLSTATE `40P01`). **Retry transactions that fail with `40001` or `40P01`**.

## Locks

| What | Lock | Held until |
|---|---|---|
| A write to a table a TVIEW's mapping joins | exclusive value locks on the join values of the rows it changed, old and new, before it looks up the TVIEW rows they feed | end of transaction |
| A write to a TVIEW's rows that other TVIEWs embed | exclusive value locks on those rows' keys, before it looks up the parents | end of transaction |
| A refresh of some rows (every write) | shared value locks on every join value the rows read (including values no row holds yet), and on the keys of the embedded rows; exclusive locks on the keys of the rows it creates; then row locks on the existing `tv_<entity>` rows (`SELECT … FOR UPDATE`, under `READ COMMITTED`) | end of transaction |
| A full refresh caused by a write (`TRUNCATE` of a base table, a table under the `full_refresh` policy) | the TVIEW and every relation it reads, locked whole; row locks on the rows that differ | end of transaction |
| `pg_tviews_refresh(entity)` | the same, and `ACCESS EXCLUSIVE` on each TVIEW it rebuilds (`TRUNCATE` + `INSERT`) | end of transaction |
| Filling an UNLOGGED TVIEW a crash or a promotion reset (the first write, `pg_tviews_rebuild_all()`, the startup worker) | the TVIEW's row in `tviews.pg_tview_valid`, inserted before the fill: other writers of that TVIEW wait for it (under `REPEATABLE READ` they fail with `40001`); readers are not blocked | end of transaction |
| Creating, replacing or dropping a TVIEW | advisory lock `pg_advisory_xact_lock(1953917285, hashtext(entity))`, so DDL on one entity runs one call at a time | end of transaction |

The value locks live in PostgreSQL's lock manager as advisory locks of pg_tviews'
own: `objsubid` is 21622 for a value (or the key of a row being created) and 21623 for
a relation's intent and escalated locks, `classid` is the relation, `objid` a hash of
the value. `pg_advisory_lock()` uses other `objsubid` values, so it can neither take
nor block them. They are kept by a prepared transaction until `COMMIT PREPARED`, and
released by `ROLLBACK TO SAVEPOINT` when taken after the savepoint.

Past `pg_tviews.lock_escalation_threshold` values of one relation (64 by default), a
transaction locks the relation instead of its values, so a bulk write never exhausts
the shared lock table: one transaction renaming 20,000 users holds a few dozen locks, and
writes linking rows to any of those users wait for it.

```sql
-- Who waits for whom
SELECT l.pid, c.relname AS relation,
       CASE l.objsubid WHEN 21622 THEN 'value' ELSE 'relation' END AS lock,
       l.mode, l.granted, pg_blocking_pids(l.pid) AS blocked_by
FROM pg_locks l
JOIN pg_class c ON c.oid = l.classid
WHERE l.locktype = 'advisory' AND l.objsubid IN (21622, 21623)
  AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY l.granted, l.pid;
```

`pg_tviews_queue_stats()` counts a transaction's `value_locks`,
`value_lock_escalations`, `value_lock_waits` and `value_lock_wait_ms`.

## Suspended refresh

`pg_tviews_suspend_triggers()` defers refreshes until `pg_tviews_resume_triggers()`
or the end of the transaction, whichever comes first: suspension never outlives the
transaction that started it. Resuming rebuilds the TVIEWs that changed and those that
embed them (each with `ACCESS EXCLUSIVE`, as `pg_tviews_refresh` does).

## Settings

The settings are listed in the [README's Configuration table](../README.md#configuration).
The ones that bear on concurrent writers:

- `pg_tviews.lock_escalation_threshold` (superuser): value locks per relation before a transaction
  locks the relation (0 always locks relations; -1 never does, which can exhaust the
  shared lock table on a bulk write);
- `pg_tviews.max_queue_size` (superuser): refreshes one transaction may queue before it fails;
- `pg_tviews.direct_patch_enabled` (superuser, hidden): the direct-patch fast path (on
  by default).

PostgreSQL's settings apply to these locks like to any other:

- `max_locks_per_transaction`: the shared lock table holds about
  `max_locks_per_transaction × (max_connections + max_prepared_transactions)` locks;
  raise it, or lower the escalation threshold, if a workload hits `out of shared
  memory`;
- `lock_timeout`: a wait longer than it fails with `55P03`;
- `deadlock_timeout`: how long a wait lasts before PostgreSQL looks for a deadlock.

A `pg_tviews.*` name that pg_tviews does not define is refused.
