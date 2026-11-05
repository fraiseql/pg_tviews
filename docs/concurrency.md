# Concurrency

**Writers run at `READ COMMITTED`. After every commit, each TVIEW row equals its
backing view; otherwise the writing transaction fails with `40P01` (a deadlock) and can
be retried.** The exceptions are listed [below](#exceptions); the design is
[ADR 0207](adr/0207-concurrent-maintenance.md).

A transaction at `REPEATABLE READ` or `SERIALIZABLE` that writes to a table a TVIEW
reads, or creates, refreshes or rebuilds TVIEWs, is refused with `0A000` before it writes: its
snapshot cannot see what it would wait for, so pg_tviews could only fail it after the
fact. Reads of TVIEWs work at every level (reports, `pg_dump`). fraiseql's mutation
functions keep rules that span rows with constraints (`UNIQUE`, `CHECK`, exclusion) and
`SELECT … FOR UPDATE`, which need no retry loop.

## When a TVIEW is refreshed

A write to a base table is refreshed **inside the writer's transaction**:

- once per top-level statement, at its end, so an autocommit statement is refreshed
  before it returns. Statements run inside it (by a function it calls, as a fraiseql
  mutation `SELECT app.fn(…)` runs its writes, or by a trigger of its write) leave
  their work to it: each TVIEW row is computed once, from the final state, however many
  statements wrote its sources ([ADR 0286](adr/0286-refresh-once-per-statement.md));
- earlier, when a query inside that statement reads a TVIEW's table (directly or
  through a view, from any function, a cursor or `RETURN QUERY`): what was queued is
  refreshed first, and the query sees it. A mutation that reads its result back before
  it returns reads it fresh. A refresh that fails raises there, or at the end of the
  top-level statement, where a function's own exception handler no longer catches it;
  `pg_tviews.max_refresh_rows` counts the rows of one such refresh. `CALL` and `DO`
  are not statements of their own here: each statement they run is top-level;
- before `COMMIT` and before `PREPARE TRANSACTION` (the `ProcessUtility` hook), so the
  refresh writes belong to the transaction.

The refresh writes are ordinary row writes to `tv_<entity>`: other sessions see them
when the writer commits, exactly like its base-table writes, and a rollback discards
both. The refresh runs as the TVIEW's owner with `search_path = pg_catalog, pg_temp`,
whoever the writer is, and renders values under fixed settings (see
[Rendering](reference/ddl.md#rendering)).

Nothing is queued across transactions. A transaction that would commit with refresh
work still queued (a flush trigger dropped or disabled) fails instead, with SQLSTATE
`55000`, naming the TVIEWs; `pg_tviews_check_health()` reports the missing trigger.

## Guarantees

Two transactions can write rows that feed the same TVIEW row at the same time: one
renames a user while another inserts a post whose `tv_post` row embeds that user, or
re-points a post to that user, or inserts the first order of a user into an aggregate
TVIEW. Whatever the order of their statements and commits, every TVIEW row equals its
backing view once both have committed: the second transaction waits for the first,
then computes from a snapshot that sees its commit.

This holds for every way a definition reads another row: an embedded TVIEW, a direct
join, a fan-out patch, a join several hops away, an outer join to a row not inserted
yet, a table under the `full_refresh` policy, a child carrying the key in its own row,
a new group of an aggregate TVIEW. Each case is an isolation spec in
`test/isolation/specs/`.

## Exceptions

| What | Consequence | See |
|---|---|---|
| A pg_tviews trigger disabled (`ALTER TABLE … DISABLE TRIGGER`, `ENABLE REPLICA`), or a write under `session_replication_role = replica` | the write is not seen: the TVIEW stays as it was until `pg_tviews_refresh()`; `pg_tviews_check_health()` reports disabled triggers | [operators guide](user-guides/operators.md) |
| A table a logical replication subscription writes | refused at create (`0A000`); a table subscribed later is reported by the health check: the apply worker fires no statement trigger | [replication](operations/replication.md) |
| Refresh suspended (`pg_tviews_suspend_triggers()`) | the TVIEWs are rebuilt at resume or at an explicit `COMMIT`; an implicit commit of suspended writes (a procedure's `COMMIT`, several statements in one query string) fails with `55000` | [Suspended refresh](#suspended-refresh) |
| A table under the `full_refresh` policy | each write refreshes the whole TVIEW, locking it and every relation it reads | [DDL reference](reference/ddl.md) |
| A TVIEW that reads the current time | its rows drift from the view between writes until `pg_tviews_refresh_time_dependent()` | [DDL reference](reference/ddl.md) |

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

Writes to unrelated values don't wait for each other. A write that changes **no
link** (it inserts and deletes no row, and updates only columns its TVIEWs show, not
columns they join, filter, group or order by) takes no shared value lock: two such
writes, say two users editing their own `bio`, each refreshing posts that show both
users, only wait for each other's row locks, in key order, as two plain `UPDATE`s of
those rows would.

Readers of a TVIEW wait only while it is rebuilt whole (`TRUNCATE`, which takes
`ACCESS EXCLUSIVE`): by `pg_tviews_refresh()` or a resume
([below](#suspended-refresh)) run with the privileges of its owner, by the operator
functions `pg_tviews_refresh_all()` and `pg_tviews_rebuild_all(only_empty => false)`,
and by DDL on its table. Every other refresh changes rows in place. Creating a TVIEW
takes `SHARE` on the tables its definition reads: their readers don't wait, their
writers do ([Locks](#locks)).

A statement takes its locks in one pass, in a fixed order: its exclusive locks on the
values it changed and, after a link change, the shared locks of the rows it reaches.
The pass covers every TVIEW over the table it writes. Two single-statement writes
therefore don't wait for each other in a cycle, even when each one's rows read the
other's values (two items soft-deleted in one group, each row showing its place's
count). Like any workload that locks, transactions that change links can still
deadlock:
- **Several statements:** the locks of a transaction's earlier statements are already
  held when a later statement takes its pass.
- **Rows linked meanwhile:** another transaction linked rows to the written values and
  committed while the pass waited. Those rows' read sets are locked at the flush, after
  the pass.
- **Rows the refresh reaches itself:** the parents of an embedded TVIEW.

PostgreSQL detects a deadlock and fails one of the transactions with `deadlock detected`
(SQLSTATE `40P01`). **Retry transactions that fail with `40P01`**.

A write that changes a link locks what every row it reaches reads, so the cost of a link
change grows with its fan-out. When each child shows an aggregate of its parent (each
item the count of its place's current items), compose the aggregate at read time instead:
[a parent's aggregate on its children](user-guides/embed-or-compose.md#a-parents-aggregate-on-its-children).

## Locks

| What | Lock | Held until |
|---|---|---|
| A write to a table a TVIEW's mapping joins | exclusive value locks on the join values of the rows it changed, old and new; in a transaction that changed a link, also the shared value locks of the TVIEW rows those values reach (found without locks first). Both are taken in one pass in a fixed order, for every TVIEW over the table, before the TVIEW rows are looked up again | end of transaction |
| A write to a TVIEW's rows that other TVIEWs embed | exclusive value locks on those rows' keys, before it looks up the parents | end of transaction |
| A refresh of some rows (every write) | in a transaction that changed a link (see above): shared value locks on every join value the rows read (including values no row holds yet), and on the keys of the embedded rows; always: exclusive locks on the keys of the rows it creates, then row locks on the existing `tv_<entity>` rows in key order (`SELECT … FOR UPDATE`) | end of transaction |
| A full refresh caused by a write (`TRUNCATE` of a base table, a table under the `full_refresh` policy) | the TVIEW and every relation it reads, locked whole; row locks on the rows that differ | end of transaction |
| `pg_tviews_refresh(entity)` | the same, and `ACCESS EXCLUSIVE` on each TVIEW it rebuilds (`TRUNCATE` + `INSERT`) | end of transaction |
| Filling an UNLOGGED TVIEW a crash or a promotion reset (the first write, `pg_tviews_rebuild_all()`, the startup worker) | the TVIEW's row in `tviews.pg_tview_valid`, inserted before the fill: other writers of that TVIEW wait for it; readers are not blocked | end of transaction |
| Creating or replacing a TVIEW | `SHARE` on every table the definition reads, taken as each table's owner before the fill: the call waits for writes in flight to those tables, and their writers wait for it, so the new TVIEW starts from committed rows and misses no write | end of transaction |
| Creating, replacing or dropping a TVIEW | pg_tviews' registration lock of the entity (an advisory lock with `objsubid` 21624), so DDL on one entity runs one call at a time; a role that may not change the TVIEW is refused before it takes the lock | end of transaction |

The value locks live in PostgreSQL's lock manager as advisory locks of pg_tviews'
own: `objsubid` is 21622 for a value (or the key of a row being created) and 21623 for
a relation's intent and escalated locks, `classid` is the relation, `objid` a hash of
the value keyed with a secret the server makes at start: no role can tell from it which
value is locked, and a superuser computes the tag of a value with
[`pg_tviews_value_lock_tag`](reference/api.md#pg_tviews_value_lock_tag). A registration lock has `objsubid` 21624 and a hash of the entity as
`classid`. `pg_advisory_lock()` uses other `objsubid` values, so it can neither take
nor block any of them. They are kept by a prepared transaction until `COMMIT PREPARED`, and
released by `ROLLBACK TO SAVEPOINT` when taken after the savepoint; a link changed
after the savepoint is forgotten with them.

Which columns link a TVIEW's rows is read from its definition when it is registered:
a column read in a join or filter condition, a `GROUP BY` (unless the definition
groups by its identity), a `DISTINCT`, a window, the `ORDER BY` of a subquery under
`LIMIT`, an aggregate's `FILTER`, a function in `FROM`, or a subquery compared in one
of those. A definition the analysis cannot follow (a `UNION` that removes duplicates,
an aggregate compared above its level) counts every column as a link.

Past `pg_tviews.lock_escalation_threshold` values of one relation (64 by default), a
transaction locks the relation instead of its values, so a bulk write never exhausts
the shared lock table: one transaction renaming 20,000 users holds a few dozen locks, and
writes linking rows to any of those users wait for it.

A refresh escalates the same way only for a relation the role that called into
pg_tviews (the writer, not the TVIEW's owner the refresh runs as) may write itself, with
`INSERT` or `UPDATE`, checked once per relation and transaction: a whole-relation lock
blocks every writer of that relation. For any other role the refresh keeps one lock per
value, up to 16 times `pg_tviews.lock_escalation_threshold`; past that the statement
fails with `54000` ("too many rows linked in one statement"), naming no relation. A
write's own locks on the values it changed escalate without this check: the writer may
write those rows.

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

A superuser finds who holds or waits for the lock on one value, here `pk_user` 1 of
`tb_user`:

```sql fragment
SELECT l.pid, l.mode, l.granted, pg_blocking_pids(l.pid) AS blocked_by
FROM pg_locks l
WHERE l.locktype = 'advisory' AND l.objsubid = 21622
  AND l.classid = 'tb_user'::regclass
  AND l.objid::bigint = tviews.pg_tviews_value_lock_tag('tb_user', 'pk_user', '1');
```

`pg_tviews_show_queue()` counts a transaction's `value_locks`,
`value_lock_escalations`, `value_lock_waits` and `value_lock_wait_ms`.

## Suspended refresh

`pg_tviews_suspend_triggers()` defers refreshes until `pg_tviews_resume_triggers()`
or the end of the transaction, whichever comes first: suspension never outlives the
transaction that started it. Resuming brings the TVIEWs that changed, and those that
embed them, to their views. A caller with the privileges of a TVIEW's owner rebuilds
it (`TRUNCATE`, with `ACCESS EXCLUSIVE`, as `pg_tviews_refresh` does): its readers wait
until the transaction ends. Any other caller changes only the rows that differ, as a
write's full refresh does, and no reader waits.

## Settings

The settings are listed in the [README's Configuration table](../README.md#configuration).
The ones that bear on concurrent writers:

- `pg_tviews.lock_escalation_threshold` (superuser): value locks per relation before a transaction
  locks the relation (minimum 0, which always locks relations);
- `pg_tviews.max_queue_size` (superuser, hidden): refresh keys one transaction may queue
  before it fails;
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
