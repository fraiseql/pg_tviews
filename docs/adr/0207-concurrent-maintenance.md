# ADR 0207: Concurrent maintenance with value locks

- Status: Proposed
- Fixes: #207 (concurrent writes leave TVIEW rows stale)
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
| value locks (this ADR, trigger prototype) | 3,800–4,100 | 925 | 0 |
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

### Lock tags and modes

The tag is `LOCKTAG_ADVISORY` with a field PostgreSQL's own advisory functions never use:

- `field1` = database, `field2` = relation OID, `field3` = a 32-bit FNV-1a hash of
  (attnum, value text);
- `field4` = `0x5476` for value locks, `0x5477` for relation (intent and escalated) locks.

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

## Consequences

- A write can now wait for a concurrent transaction writing related rows: an insert of a post
  waits for an open rename of its author, and the reverse.
- REPEATABLE READ transactions can fail with 40001 where they used to write stale rows.
- Each refresh runs one read-set query per `mapped` table and one lookup query per embed, and
  takes one lock per distinct join value up to the escalation threshold.
- The plan gains, per `mapped` table, its lock columns and a read-set query.
