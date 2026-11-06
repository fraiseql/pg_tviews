# ADR 0207: Concurrent maintenance with value locks

- Status: Accepted (implemented in 0.1.0-beta.27; amended in 0.1.0-beta.34, #263, and
  0.1.0-beta.35, #277; writers at READ COMMITTED only since 0.1.0-beta.35). The Decision
  describes the protocol as it is; [History](#history) records what changed and why.
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

### Writers run at READ COMMITTED

A transaction at REPEATABLE READ or SERIALIZABLE that writes to a table a TVIEW reads (its
row, delta or truncate trigger fires), flushes refresh work, or refreshes or rebuilds TVIEWs
is refused with 0A000 before it writes. Its snapshot cannot see what it would wait for on
these locks, so it could only be failed after the fact. Reads are not concerned. Every
statement below runs at READ COMMITTED, where a statement run after a wait sees what it
waited for.

### What is locked

- A **writer** takes exclusive value locks on the join values of the rows it changed, old and
  new images, on every read-set column of the table (the columns some TVIEW's mapping joins
  on), before the query that finds the TVIEW rows by those values runs for the last time
  (see [the capture](#the-capture-one-sorted-pass)).
- A write to a TVIEW's rows that other TVIEWs embed takes exclusive value locks on those
  rows' keys before it looks up the parents. This also serializes two transactions changing
  the same child row's links.
- A **refresh** takes shared value locks on the join values its rows read (their read set)
  and on the keys of the embedded rows they read, before it computes them, **only in a
  transaction that changed a link**: inserted or deleted a row of a table a TVIEW reads, or
  updated a column of one that some TVIEW's rows are linked by. TRUNCATE, partitions
  attached, detached or dropped, full refreshes and suspended writes count as link changes;
  `ROLLBACK TO SAVEPOINT` forgets a link changed after the savepoint, as the lock manager
  releases its locks. A transaction that changed no link computes rows linked by committed
  rows only: a writer of a value they read finds them by its own query, and the row locks
  order the two refreshes.
- Every refresh takes a weak **intent lock** on the TVIEW. A write that refreshes a whole
  TVIEW (a `full_refresh` table, `TRUNCATE`, a partition attached or detached, a
  materialized view refreshed, a suspension caught up) takes an exclusive lock on it, and
  locks every relation it reads whole.

The read set comes from the plan (ADR 0203): for each `mapped` table, a query from the
TVIEW's keys to the values of the columns its mapping query joins on, the same columns the
writer locks. For an embedded TVIEW, the value is the parent's lookup column.

**Link columns** come from the definition (per table, in the plan): a column read in a join
or filter condition, a GROUP BY (not the definition's own, grouped by its identity), a
DISTINCT, a window's PARTITION BY or ORDER BY, the ORDER BY of a level under LIMIT/OFFSET,
an aggregate's FILTER, a function in FROM, a subquery compared in one of those, and the key.
A read the walk cannot follow to columns (a set operation that removes duplicates, an
aggregate compared above its level, a whole-row reference) makes every column a link.

### The capture: one sorted pass

The lookup that finds the rows a write reaches runs at capture, in the statement-level delta
trigger, because it reads the transition tables; it must run after the writer's locks, or it
misses a row a concurrent transaction is creating. The capture takes the locks of both sides
together:

1. **Look up without locks.** For each TVIEW whose delta trigger fires for the statement,
   find the rows the changed rows reach (`K0`). When the transaction changed a link, also
   read what those rows read: their read sets and embedded keys.
2. **One sorted pass.** Take the writer's exclusive locks on the changed rows' values and,
   after a link change, the refresh's shared locks on the read set of `K0`, all at once,
   ordered by relation, then by space, relation locks (intent, escalated) before value
   locks, then by hash. An exclusive lock covers a shared request on the same value.
   Escalation is decided per relation, space and side over the whole pass, so an escalated
   lock takes its relation's place in the order. A writer's intent and a refresh's
   escalated lock on one relation are one `ShareRowExclusiveLock`. Transactions that each
   take one such pass can't wait for each other in a cycle.
3. **Look up again**, in a new snapshot, after the pass: this is the lookup the refresh
   uses. A row found now but not in step 1 was created or re-pointed by a transaction that
   committed in between, or written by this transaction's own triggers; the flush locks its
   read set, as for every row it computes, before computing it.

**Several TVIEWs on one table.** Each TVIEW has its own delta trigger, and they fire one
after the other. The first delta trigger of a statement takes steps 1 and 2 for every
pg_tviews delta trigger that fires for the same table and event, in the order PostgreSQL
fires them; the later ones find their locks held and run step 3. A statement is named by its
executor frame and its transition tables; a trigger that finds no pass recorded for it (a
`COPY`, which runs no executor) takes its own pass. The partitioned-table paths (a row
trigger per changed row, rows entering with a partition) take the same pass, one TVIEW at a
time.

The flush reads the read set of the rows it computes again, after the capture read it.
Checking instead what the compute itself read, against the locks taken before it, was
rejected (#281): a row re-linked by a transaction that committed while the capture's pass
waited is then found only after the refresh holds the row's lock, and a writer holding the
new value needs that row: a deadlock where the flush now waits.

### Rows being created

A refresh that creates a TVIEW row (its key has no row yet) can race another refresh creating
the same row with no join value between them: a child that carries the key in its own row (a
comment naming post 9, with no foreign key) written while post 9 is inserted, or two first
rows of one group of an aggregate TVIEW. A refresh therefore locks, exclusively, each key it
is about to create, in the TVIEW's *key space*: a hash space and a relation lock of its own
(`field3` 1 on the relation tag), so that escalating these locks only stops other
transactions creating rows of that TVIEW.

The order is: row locks on the rows found (`SELECT … FOR UPDATE`, in key order), then key
locks on the missing keys, then row locks on the rows of those keys another transaction
created and committed in between, in a new statement, before the refresh computes. A row is
only ever created by a refresh that holds its key lock: the refresh checks the rows its
upsert inserted against the key locks it holds, and a row created without its lock is an
internal error. Specs: `local-child-vs-new-parent`, `aggregate-new-group-vs-write`,
`new-row-vs-row-lock`.

### Patches

- **One refresh of an entity per flush, its rows locked in key order.** The rows a flush's
  own writes queue through another TVIEW's triggers join the flush after each entity, and a
  refresh that patches some rows and recomputes others locks them all in key order first.
- **A patch derived for a parent holds only while the parent still embeds the child.**
  Before patching, the refresh locks the rows in key order (the latest committed version)
  and recomputes a parent whose lookup column no longer holds the child's key.
- **A two-hop fan-out patch locks the rows in between.** The lookup takes a shared value
  lock on each row in between it reached (by the column the patched rows join on, a read set
  every write of such a row locks exclusively), whether or not the transaction changed a
  link, then reads again in a new snapshot until it reaches no row it hasn't locked. Past
  `pg_tviews.lock_escalation_threshold` rows it locks nothing and the rows are mapped and
  recomputed, which reads the link itself. The order is fixed: the writer's exclusive locks,
  the lookup, the shared locks on what it reached; a shared lock taken before the lookup
  deadlocks with a row being moved to the renamed value (`two-hop-rename-vs-move-in`). The
  lookup reads as the TVIEW's owner and locks no row, so the writer needs no privilege on the
  table in between and its policies hide nothing (#268); no value it reads reaches the
  writer's errors (`two-hop-hidden-repoint`).

### Lock tags and modes

The tag is `LOCKTAG_ADVISORY` with a field PostgreSQL's own advisory functions never use:

- `field1` = database, `field2` = relation OID, `field3` = SipHash-2-4 of (attnum, the
  value's lock form), keyed with a 128-bit secret the postmaster draws from
  `pg_strong_random` into shared memory at start and truncated to 32 bits: `pg_locks`
  shows the tags to every role, and an unkeyed hash would let any of them test which
  value is locked. Without `shared_preload_libraries` there is no key, and pg_tviews
  refuses to run at all (`55000`); a superuser computes a tag with
  `pg_tviews_value_lock_tag`. The lock form is the hash the column type's equality uses
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
(20,000 values gave `out of shared memory`). Past `pg_tviews.lock_escalation_threshold` values
on one relation (default 64, per transaction, space and side), a transaction locks the
relation instead. Values already held stay held.

A refresh's escalated lock blocks every writer of the relation, so a refresh locks a relation
whole only when the role that wrote may write that relation itself (`INSERT` or `UPDATE`,
checked once per relation and transaction, for the role that called into `pg_tviews`, not
the TVIEW's owner it refreshes as). A role that could not block those writers with plain
PostgreSQL keeps locking values, one per value, up to 16 times the threshold; past that the
statement fails with 54000 ("too many rows linked in one statement"), naming no relation. A
writer's escalation is not gated: it locks values of the rows it changed, which it may write.

### Fresh snapshots

The queries that find TVIEW rows (mapping queries, the embed lookup) and that compute them run
as read-write SPI statements: each takes a new snapshot, so one run after a lock wait sees
what it waited for. A read-only SPI statement reuses the snapshot of the statement that fired
the trigger, taken before the wait.

### What can still deadlock

Transactions that each take one pass per statement don't wait for each other in a cycle. A
deadlock remains possible, reported by PostgreSQL with 40P01, which is retryable:

- **Multi-statement transactions.** A transaction's earlier statements' locks are held when a
  later one takes its pass, as with row locks (`value-lock-deadlock`).
- **Rows found by the second lookup only.** Their read set is locked at the flush, after the
  pass, so a row created and committed between the two lookups can bring in a value another
  transaction holds exclusively (`created-row-vs-lock-pass`).
- **The flush.** It locks the read set of the rows it computes again: values that changed
  since the capture, and rows queued by the flush itself (the parents of an embedded TVIEW).

## Consequences

- A write can wait for a concurrent transaction writing related rows: an insert of a post
  waits for an open rename of its author, and the reverse. Deadlocks between such writes are
  reported with 40P01. A transaction left open holds its locks until it ends, as with row
  locks: set `idle_in_transaction_session_timeout` for writer roles.
- A role without `INSERT` or `UPDATE` on a table a TVIEW reads links at most 16 times
  `pg_tviews.lock_escalation_threshold` of its rows in one transaction (54000 past that).
- Writers at REPEATABLE READ or SERIALIZABLE are refused (0A000); fraiseql's mutations run at
  READ COMMITTED and keep rules spanning rows with constraints and `SELECT … FOR UPDATE`.
- After a link change the capture runs the lookup twice and the read-set query once more,
  per TVIEW (about 1.4 ms per such statement on the benchmark's schema,
  `test/concurrency/run.sh embed`); lookups by up to 128 keys, read sets among them, are
  planned once per backend (#281). A write that changes no link takes its writer locks in one
  pass. A schema can avoid the fan-out of a link change by composing a parent's aggregate at
  read time ([embed or compose](../user-guides/embed-or-compose.md#a-parents-aggregate-on-its-children)).
- The plan carries, per `mapped` table, its read sets (`reads`: the joined column and the
  read-set query) and its link columns; `pg_tviews_read_set_queries()` shows the read sets.

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

## History

**0.1.0-beta.27 (2026-10-10, #207, #214): the first protocol.** A refresh locked the read set
of every row it recomputed, before computing it; the writer locked its values before any
query found TVIEW rows by them, one read-set column after another, in the delta trigger.
REPEATABLE READ never waited: a lock not granted at once raised 40001, and every refresh and
discovery was cross-checked against the latest snapshot, as PostgreSQL's foreign-key checks
are. SERIALIZABLE took no value locks, on the ground that SSI detects the anomaly.

**0.1.0-beta.34 (#263): shared locks only after a link changed.** The first protocol
deadlocked two single-row writes that changed no relationship (two users each editing their
own `bio`, each having commented on the other's post: the trigger took the exclusive lock on
its own user and the flush a shared lock on the other one; velocitybench saw 846 deadlocks,
writes falling from 589 to about 8 per second). "Deadlocks between value locks taken in
different statements" was wrong: one statement was enough. The refresh's shared locks became
conditional on a link change, link columns entered the plan (version 3), the writer's locks on
embedded keys were described, and one refresh per entity per flush locked its rows in key
order. The concurrent fuzzer then found, stale on beta.33 too: SERIALIZABLE needed the locks
and checks of REPEATABLE READ (SSI orders serializable transactions among themselves only;
`serializable-vs-read-committed`); a patch derived for a parent had to re-check that the
parent still embeds the child (`embed-patch-vs-repoint`); a full refresh under REPEATABLE
READ had to be cross-checked too (`rr-full-refresh-vs-insert`); and a two-hop fan-out patch
had to lock the rows in between (`two-hop-patch-vs-repoint`, `two-hop-rename-vs-move-in`,
`two-hop-hidden-repoint`, #268). Rows created between a refresh's lookup and its key lock
were then row-locked too (`new-row-vs-row-lock`, #272), and a REPEATABLE READ refresh that
would recreate a row deleted after its snapshot failed with 40001 (`rr-recreated-row`, #273).

**0.1.0-beta.35 (#277): one sorted pass at capture.** One statement could still deadlock when
it changed a link: two transactions soft-deleting items of one group in two places each held
their own place exclusively, then one waited for the group and the other, at flush, for a
shared lock on the first one's place (`embed-count-soft-deletes`; 40P01 on about half of the
link changes at 16 clients on beta.33 and beta.34). Moving the writer's locks to the flush,
sorted with the refresh's, made four specs stale (the lookup must run after the writer's
locks), so the capture takes both sides' locks in one sorted pass between two lookups. #275
(ordering key locks across a flush) was superseded.

**0.1.0-beta.35 (2026-11 architecture wave, decision D1): writers at READ COMMITTED.** The
REPEATABLE READ and SERIALIZABLE handling (fail fast on a lock, cross-check every refresh,
discovery and reconcile against the latest snapshot, about a fifth of such a writer's time)
served no workload pg_tviews is built for: fraiseql's mutations run at READ COMMITTED. Such
writers are now refused with 0A000 before they write; the fail-fast policy, the cross-check
(`concurrency/crosscheck.rs`) and their 40001 are gone. The REPEATABLE READ and SERIALIZABLE
specs were rewritten to expect the refusal.

**0.1.0-beta.36: keyed tags and gated escalation.** `pg_locks` shows every role the tags of
every lock, so the 32-bit FNV-1a hash of a value let any role test which value another
transaction held. The tag is now SipHash-2-4 keyed with a secret the postmaster draws at
start ([Lock tags and modes](#lock-tags-and-modes)); a superuser matches it with
`pg_tviews_value_lock_tag(relation regclass, column_name text, value text)`, which converts
`value` to the column's type as the lock form does (a NULL `column_name` names a TVIEW key
being created). A refresh's escalation took a whole-relation lock on any relation a TVIEW
reads, whichever role wrote: it now needs `INSERT` or `UPDATE` on the relation for the role
that called into pg_tviews, otherwise the refresh keeps value locks up to 16 times the
threshold and fails with 54000 past it ([Escalation](#escalation)). The registration lock
serializing DDL on one entity moved from `pg_advisory_xact_lock(1953917285, …)`, which any
role could take, to an advisory tag of pg_tviews' own (`field4` 21624).

