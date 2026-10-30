# ADR 0207: Concurrent maintenance with value locks

- Status: Accepted (implemented in 0.1.0-beta.27; amended in 0.1.0-beta.34, #263)
- Fixes: #207 (concurrent writes leave TVIEW rows stale), #214 (concurrent first writes
  into a reset UNLOGGED TVIEW)
- Builds on: [ADR 0157](0157-cascade-key-mapping.md) (mapping queries),
  [ADR 0203](0203-propagation-plan.md) (the propagation plan)

## Context

A write to a base table refreshes the TVIEW rows it affects inside the writer's transaction. It
finds them in one of two ways:

- by the changed row itself (a `local` table: the key is a column of the row), or
- by a query: the mapping query of a `mapped` table, the fan-out `UPDATE … WHERE lookup = key`,
  the embed lookup that finds the parents of a refreshed child, or a full reconcile under the
  `full_refresh` policy.

A query can't see a row that another open transaction just inserted or re-pointed. That other
transaction, meanwhile, computes the row from a snapshot without the first writer's change.
Neither waits for the other, so the row stays stale after both commit. Row locks on the TVIEW
(`SELECT … FOR UPDATE` before a recompute) serialize two writers of an *existing* row; they
can't help with a row that doesn't exist yet in the other's snapshot.

### Reproduced (2026-10-10)

Each shape below is an isolation spec in `test/isolation/specs/`. All are stale at READ
COMMITTED in both orders a waiting protocol can run:

| Spec | Shape |
|---|---|
| `new-parent-vs-write` | a parent inserted while the child TVIEW it embeds is renamed |
| `join-parent-vs-write` | the same with a direct base-table join |
| `fanout-vs-new-parent` | the same when the rename is written by a fan-out patch |
| `repoint-vs-write` | an existing parent re-pointed (`UPDATE tb_post SET fk_user`) to a child being renamed |
| `multihop-vs-write` | a post inserted while its author's organisation, two hops away, is renamed |
| `phantom-child-vs-parent` | an outer join: a post naming a user that doesn't exist yet, while that user is inserted |
| `full-refresh-vs-new-parent` | a post inserted while a table under `full_refresh` changes |
| `rr-new-parent-vs-write` | REPEATABLE READ: stale in every order, even when the rename committed before the insert ran |

SERIALIZABLE is already correct: SSI raises 40001 at the second commit
(`serializable-new-parent-vs-write`).

Under load (pgbench, 16 clients, 80% post inserts / 20% user renames), 10–15 TVIEW rows were
stale after every 20-second run.

## Alternatives

Measured with the same pgbench workload (tps):

| Protocol | 1,000 users | 10 hot users | Stale |
|---|---|---|---|
| none (before this ADR) | 3,300–3,800 | 890 | 10–15 |
| value locks (this ADR, measured with triggers) | 3,800–4,100 | 925 | 0 |
| one lock per relationship, inserts vs updates (`S`/`RX`) | ~720 | 400 | 0 |
| one lock per TVIEW (pg_ivm's `ExclusiveLock`) | ~200 | 230 | 0 |

Rejected:

- **Row locks on what a refresh reads.** They miss phantoms (an outer join, a child inserted
  later, a join on a non-key column) and lock users' own base rows: multixact churn, and
  unrelated updates of those rows blocked.
- **One lock per relationship.** About 5x slower: every insert of a post serializes with every
  rename of any user.
- **One lock per TVIEW.** About 20x slower. This is what pg_ivm does.
- **A link table with conflicting upserts.** Correct, but a heap write and its WAL per link and
  per change.
- **Asynchronous, commit-ordered maintenance.** It gives up read-your-writes and
  `pg_tviews_flush_and_report`. It may come later as an opt-in mode.

Precedents: SQL Server maintains multi-table indexed views under internal serializable
key-range locks; pg_ivm takes relation locks.

## Decision

A lock names a **join value**: `(relation, column, value)`. Locks live in PostgreSQL's lock
manager and are held to the end of the transaction.

### The protocol

- A **writer** takes an exclusive value lock on the join values of the rows it changed, old and
  new images, before any query that finds TVIEW rows by those values.
- A **refresh** takes shared value locks on every join value its rows read, before it computes
  them. The computation runs in a later statement, so under READ COMMITTED it sees whatever it
  waited for.
- A refresh locks the read set of **every** row it recomputes, not only new or re-pointed rows.
  This is simple, and correct for links changed at any depth (a user moved to another
  organisation re-links that user's posts).
- A write that refreshes a whole TVIEW (a `full_refresh` table, `TRUNCATE`, a partition
  attached or detached, a materialized view refreshed, a suspension caught up) takes an
  exclusive lock on the TVIEW itself. Every refresh takes a weak intent lock on it, so the two
  conflict.

The read set comes from the plan (ADR 0203): for each `mapped` table, a query from the TVIEW's
keys to the values of the columns its mapping query joins on, the same columns the writer
locks. For an embedded TVIEW, the value is the parent's lookup column, read from the backing
view before computing.

### Rows being created

A refresh that creates a TVIEW row (its key has no row yet) can race another
refresh creating the same row with no join value between them: a child that
carries the key in its own row (a comment naming post 9, with no foreign key)
written while post 9 is inserted, or two first rows of one group of an aggregate
TVIEW. A refresh therefore also locks, exclusively, each key it is about to
create, in the TVIEW's *key space*: a hash space and a relation lock of its own
(`field3` 1 on the relation tag), so that escalating these locks only stops other
transactions creating rows of that TVIEW. Existing rows keep their row locks
(`SELECT … FOR UPDATE`). Specs: `local-child-vs-new-parent`,
`aggregate-new-group-vs-write`.

The key lock only stops other creations of the row. A row another transaction
created and committed after the refresh looked for it, and that a third one may
since have locked through the row itself, is locked after the key lock, in a new
statement, before the refresh computes: the order is row locks of the rows found,
then key locks, then row locks of the rows created in between (amended in
0.1.0-beta.34; `new-row-vs-row-lock`). Before, the refresh's upsert waited for
that row inside `ON CONFLICT DO UPDATE` and wrote what it computed before the
wait.

A row is only ever created by a refresh that holds its key lock: the refresh
checks the rows its upsert inserted against the key locks it holds. Under
REPEATABLE READ and SERIALIZABLE a row the snapshot still shows may have been
deleted since by a committed transaction; the refresh took no key lock for it, and
its upsert would create it anew. It fails with 40001 instead (`rr-recreated-row`,
#273). Under READ COMMITTED the rows found are locked, so a row created without
its key lock is an internal error.

### Lock tags and modes

The tag is `LOCKTAG_ADVISORY` with a field PostgreSQL's own advisory functions never use:

- `field1` = database, `field2` = relation OID, `field3` = a 32-bit FNV-1a hash of
  (attnum, the value's lock form). The lock form is the hash the column type's equality uses
  (its default hash operator class), in the written column's type and collation, as text, so
  equal values with different text forms (`5` and `5.00`, `Ann` and `ann` under a
  case-insensitive collation) take the same lock on both sides;
- `field4` = `0x5476` for value locks, `0x5477` for relation (intent and escalated) locks,
  whose `field3` is 0 for the values of the relation and 1 for the keys of a TVIEW's rows
  being created.

`pg_advisory_lock()` uses `field4` 1 and 2, so a user can neither take nor block these locks. A
hash collision gives an extra wait, never a missed conflict.

| Lock | Refresh | Writer |
|---|---|---|
| value | `ShareLock` | `ExclusiveLock` |
| relation intent | `RowShareLock` | `RowExclusiveLock` |
| escalated (whole relation) | `ShareLock` | `ExclusiveLock` |

PostgreSQL's conflict table then gives what's needed: shared and exclusive value locks
conflict, two shared ones don't; an escalated `ShareLock` conflicts with writers'
`RowExclusiveLock` intent, and an escalated `ExclusiveLock` conflicts with both intents.

### Escalation

One lock per value exhausts the shared lock table at the default `max_locks_per_transaction`
(20,000 values gave `out of shared memory`). Past `pg_tviews.lock_escalation_threshold` values on
one relation (default 64, per transaction and per side), a transaction locks the relation
instead. Values already held stay held.

### Isolation levels

- **READ COMMITTED** waits.
- **REPEATABLE READ** never waits: the transaction snapshot is fixed, so waiting can't make a
  computation see the other side's change. A lock that isn't granted at once raises 40001. Every
  refresh and every discovery is then cross-checked against the latest snapshot, as
  PostgreSQL's foreign-key checks are: a difference raises 40001.
- **SERIALIZABLE** takes no value locks: SSI already detects the anomaly.

Deadlocks between value locks taken in different statements are possible, as with row locks;
PostgreSQL reports them with 40P01. Both 40001 and 40P01 are retryable.

### Fresh snapshots

The queries that find TVIEW rows (mapping queries, the embed lookup) and that compute them run
as read-write SPI statements: under READ COMMITTED each takes a new snapshot, so one run after a
lock wait sees what it waited for. A read-only SPI statement reuses the snapshot of the statement
that fired the trigger, taken before the wait.

## Consequences

- A write can now wait for a concurrent transaction writing related rows: an insert of a post
  waits for an open rename of its author, and the reverse. Deadlocks between such writes are
  reported with 40P01.
- REPEATABLE READ transactions can fail with 40001 where they used to write stale rows.
- Each refresh runs one more statement, the read sets of all its mapped tables and embeds
  together, from a generic plan kept for the backend, and takes one lock per distinct join
  value up to the escalation threshold.
- The plan gains, per `mapped` table, its read sets (`reads`: the joined column and the
  read-set query); `pg_tviews_read_set_queries()` shows them.

## Results

Measured on a shared 24-core host (its load moved ±10% between runs), against the same branch
before the protocol, runs interleaved:

| Workload (pgbench, 16 clients) | Before | After |
|---|---|---|
| 1,000 users, 80% post inserts / 20% renames | 2,988 tps, 39–59 stale rows | 3,193 tps, 0 stale |
| 10 hot users | 651 tps, 12–19 stale | 683 tps, 0 stale |
| one transaction renaming 20,000 users during inserts | ~5.0 s, 4–9 stale | ~5.0 s, 0 stale |
| one client (latency) | 4.06 ms | 4.30 ms |

A single-row statement of the product benchmark (`test/sql/real_benchmark`) costs about 4% more
at the median (about 0.05 ms). A 10-minute mixed run (inserts, renames of users and of
organisations two hops away, users moving between organisations, a rename of every user every
30 s) ends with no stale row.

## Amendment: shared locks only after a link changed (0.1.0-beta.34, #263)

The protocol above deadlocked two single-row writes that changed no relationship. Two
users each edit their own `bio`; each commented on the other's post, so each write's
refresh of `tv_post` reads both users. Within one autocommit statement, the trigger took
the exclusive lock on its own user and the flush then a shared lock on the other one:
the two statements waited for each other (velocitybench: 846 deadlocks, writes from 589
to about 8 per second between 0.1.0-beta.11 and 0.1.0-beta.33). "Deadlocks between
value locks taken in different statements" above was wrong: one statement is enough.

The shared locks protect a refresh whose rows are linked to a value by a row only its
own transaction can see: a post it inserted, a comment it re-pointed, a user it moved to
another organisation. A transaction that changed no link computes rows linked by
committed rows only; a writer of a value they read finds them by its own query, and the
row locks order the two refreshes, the second computing after the first commits.

- **A refresh takes its shared value locks (read sets and embedded keys) only in a
  transaction that changed a link**: inserted or deleted a row of a table a TVIEW reads,
  or updated a column of one that some TVIEW's rows are linked by. TRUNCATE, partitions
  attached, detached or dropped, full refreshes and suspended writes count as link
  changes. The intent lock, the writer's exclusive locks and the key locks of rows
  being created are unchanged. `ROLLBACK TO SAVEPOINT` forgets a link changed after the
  savepoint, as the lock manager releases its locks.
- **The link columns come from the definition** (plan version 3, per table): a column
  read in a join or filter condition, a GROUP BY (not the definition's own, grouped by
  its identity), a DISTINCT, a window's PARTITION BY or ORDER BY, the ORDER BY of a
  level under LIMIT/OFFSET, an aggregate's FILTER, a function in FROM, a subquery
  compared in one of those, and the key. A read the walk cannot follow to columns (a
  set operation that removes duplicates, an aggregate compared above its level, a
  whole-row reference) makes every column a link; a plan of an earlier version reads
  the same way until the update re-derives it.
- **The writer's exclusive locks on embedded keys** (taken before the parents of a
  refreshed child are looked up) were not described above; they also serialize two
  transactions changing the same child row's links, which the shared locks no longer do
  for value-only writers.
- **One refresh of an entity per flush, its rows locked in key order.** The rows a
  flush's own writes queue through another TVIEW's triggers join the flush after each
  entity, not after the last one, and a refresh that patches some rows and recomputes
  others locks them all in key order first, at every isolation level. Before, the same `tv_post` rows could be
  locked in two statements in a different order in each transaction.

Found by the concurrent differential fuzzer (`test/concurrency/fuzz/`), both stale on
0.1.0-beta.33 too:

- **SERIALIZABLE takes the locks and checks of REPEATABLE READ.** "SERIALIZABLE takes
  no value locks: SSI already detects the anomaly" above holds only when every
  concurrent writer is serializable: SSI orders serializable transactions among
  themselves, and a READ COMMITTED rename is invisible to it. A post inserted under
  SERIALIZABLE while its author was renamed under READ COMMITTED stayed stale in both
  orders (`serializable-vs-read-committed`). SERIALIZABLE now fails fast on a value
  lock and cross-checks against the latest snapshot, as REPEATABLE READ does.
- **A patch derived for a parent holds only while the parent still embeds the child.**
  A rename patched into the posts embedding the user found post 10 under its
  snapshot, waited for a concurrent re-point's row lock, then wrote the old user's
  name into a post that embedded another user (`embed-patch-vs-repoint`). Before
  patching, the refresh locks the rows in key order (the latest committed version
  under READ COMMITTED) and recomputes a parent whose lookup column no longer holds
  the child's key.

- **A full refresh is cross-checked too.** Under REPEATABLE READ the reconcile of a
  `full_refresh` table compared the TVIEW with its view under the transaction's
  snapshot: a post inserted and committed since then, written with the old name by its
  own refresh, was invisible to it and stayed stale (`rr-full-refresh-vs-insert`). The
  whole view is now compared with the latest snapshot after the reconcile, as a keyed
  refresh's rows are; a difference raises 40001.

- **A two-hop fan-out patch locks the rows in between.** A rename written into the
  lines of the renamed customer's orders found order 10 under its snapshot, waited for
  a concurrent re-point of that order, then wrote the old customer's name into lines
  that now show another one (`two-hop-patch-vs-repoint`): the link lives in `tb_order`,
  which the patch's own condition cannot re-check. The lookup now takes a shared value
  lock on each order it reached, by the column the lines join on (`pk_order`, a read
  set of `tb_order`, which every write of an order locks exclusively, old and new
  images), whether or not the transaction changed a link. Then, under READ COMMITTED,
  it reads again in a new snapshot until it reaches no order it hasn't locked: an order
  re-pointed while it waited, or between its read and its lock, is dropped. Under
  REPEATABLE READ a lock held elsewhere raises 40001. Past
  `pg_tviews.lock_escalation_threshold` orders it doesn't lock the table instead, which
  would wait for every writer of `tb_order`: the lines are mapped and recomputed, which
  reads the link itself and needs no lock in between.

  The order is fixed: the writer's exclusive locks, then the lookup, then the shared
  locks on what it reached. A shared lock taken before the lookup (on the customer's
  orders as a whole) waits for an order being moved to the renamed customer, which
  waits for the rename's customer to recompute its line: a deadlock in one statement
  each (`two-hop-rename-vs-move-in`), and so would an escalated lock. The lookup reads as the TVIEW's owner and locks
  no row: a `FOR SHARE` took UPDATE on the table in between, which its writer, and
  often its owner, lacks, and as the writer it missed the rows its policies hide
  (#268). No value it reads reaches the writer's errors, notices or results
  (`two-hop-hidden-repoint`).

Specs: `crossed-value-writes` (one statement each, gated between the triggers and the
flush), `crossed-value-writes-after-rollback`; the eight specs of the table above pass
unchanged, and `value-lock-deadlock` (two transactions that insert posts, so change
links, then rename crosswise) still deadlocks.
