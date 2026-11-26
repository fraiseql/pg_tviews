# A model of the flush and locking protocol (skeleton)

The skeleton of the model [theorems.md §2](theorems.md#2-a-model-of-the-flush-and-locking-protocol)
describes, in [Quint](https://quint-lang.org): its types, state, actions and invariants,
with the actions' bodies left as comments. It has never been run. It is the starting point
for checking T12 (Freshness under concurrency) and I2–I4 at a small scale: 2–3
transactions, 2 base tables, 1 TVIEW embedding 1 other, 2–3 keys and values. The protocol it
models is [ADR 0207](../adr/0207-concurrent-maintenance.md); each action names the section it
follows.

The decisions of `src/decide/` (T1–T11) enter as one uninterpreted function, `affects`,
assumed sound: the model checks the protocol, and the decisions are proved apart.

```quint
module tviews_protocol {
  // ── Sizes ───────────────────────────────────────────────────────────────
  pure val TXNS   = Set("t1", "t2", "t3")
  pure val TABLES = Set("tb_child", "tb_parent")
  pure val TVIEWS = Set("tv_child", "tv_parent")   // tv_parent embeds tv_child
  pure val KEYS   = Set(1, 2, 3)
  pure val VALUES = Set(10, 20)                    // join values (links)

  // ── Types ───────────────────────────────────────────────────────────────
  type Txn = str
  type Row = { key: int, link: int, payload: int }
  // A row version, MVCC-style: visible to a snapshot that sees xmin and not xmax.
  type Version = { row: Row, xmin: Txn, xmax: Txn }   // "" = none
  type Snapshot = Set[Txn]                         // the committed transactions it sees
  type LockMode = Share | Exclusive | RowShare | RowExclusive
  type LockTag =
    | Value({ rel: str, value: int })              // field4 0x5476
    | KeySpace({ rel: str, key: int })             // rows being created (field3 1)
    | Relation({ rel: str })                       // intent and escalated, field4 0x5477
    | RowLock({ rel: str, key: int })              // SELECT … FOR UPDATE
  type Pending = { keys: Set[(str, int)], fanout: Set[(str, int)], linksChanged: bool }

  // ── State ───────────────────────────────────────────────────────────────
  var heap: str -> Set[Version]                    // base and TVIEW tables
  var committed: Set[Txn]
  var aborted: Set[Txn]
  var snapshot: Txn -> Snapshot                    // READ COMMITTED: renewed per statement
  var pending: Txn -> Pending                      // the queue a flush drains
  var locks: Set[(Txn, LockTag, LockMode)]         // held to end of transaction (PG8)
  var waiting: Txn -> Set[(LockTag, LockMode)]     // at most one request each
  var savepoints: Txn -> List[{ locks: int, pending: Pending }]

  // The decisions, uninterpreted and sound (T1–T11): which TVIEW keys a change of
  // base table `rel` from `old` to `new` can affect.
  pure def affects(rel: str, old: Row, new: Row): Set[(str, int)] = Set()  // assumed

  // ⟦D_V⟧ over the rows `snap` sees, in the owner frame.
  pure def evaluate(v: str, heap: str -> Set[Version], snap: Snapshot): Set[Row] = Set() // assumed

  // ── Actions ─────────────────────────────────────────────────────────────
  // Statement start: a new snapshot of the committed transactions (PG5).
  action statementStart(t: Txn): bool = all {
    // snapshot' = snapshot.set(t, committed)
    true,
  }

  // A row write: the row trigger records the keys or the patch it reaches (PG2).
  // ADR 0207 "What is locked": inserts, deletes and link updates set linksChanged.
  action rowWrite(t: Txn, rel: str, old: Row, new: Row): bool = all {
    // heap' adds new's version (xmin t), sets old's xmax to t
    // pending'[t].keys += affects(rel, old, new)
    true,
  }

  // The capture at statement end (ADR 0207 "The capture: one sorted pass"):
  // 1. look up K0 without locks; 2. one sorted pass of every lock (writer's
  // exclusive value locks, and after a link change the refresh's shared locks on
  // the read set of K0); 3. look up again in a new snapshot.
  action capture(t: Txn): bool = all {
    // requests' = sorted(writerLocks ∪ (linksChanged ? readSetLocks(K0) : {}))
    // blocks in `waiting` while a conflicting lock is held by another transaction
    true,
  }

  // The flush (ADR 0207 "Rows being created", "Patches", "Fresh snapshots"): row
  // locks in key order, key locks on missing keys, compute in a fresh snapshot,
  // upsert or delete, record each change, cascade to the embedding TVIEW.
  action flush(t: Txn): bool = all {
    // for (v, k) in pending[t].keys, in key order:
    //   tv row ← evaluate(v, heap, committed ∪ {t})[k]  (or delete when ⊥)
    // pending'[t] = empty
    true,
  }

  // Escalation past the threshold (ADR 0207 "Escalation"): a relation lock in
  // place of its value locks.
  action escalate(t: Txn, rel: str): bool = true

  action savepoint(t: Txn): bool = true
  // Releases the locks and forgets the queue entries taken after the savepoint.
  action rollbackToSavepoint(t: Txn): bool = true

  // COMMIT: the hook's flush, then the PRE_COMMIT check: pending work fails it.
  action commit(t: Txn): bool = all {
    // pending[t] empty, else the transaction aborts (55000)
    // committed' = committed ∪ {t}; locks' drops t's locks
    true,
  }

  action rollback(t: Txn): bool = true

  // Deadlock detection: in a cycle of `waiting`, one waiter aborts with 40P01.
  action detectDeadlock: bool = true

  action step = any {
    nondet t = oneOf(TXNS)
    any { statementStart(t), capture(t), flush(t), commit(t), rollback(t),
          savepoint(t), rollbackToSavepoint(t) },
    detectDeadlock,
  }

  // ── Invariants ──────────────────────────────────────────────────────────
  // I1 (Freshness, T12): every TVIEW table, read at the committed state, equals its
  // definition evaluated at that state.
  val freshness = TVIEWS.forall(v => true /* visibleRows(v, committed) == evaluate(v, heap, committed) */)

  // I2: no transaction commits with pending work (keys or fan-out patches).
  val noCommitWithPending = committed.forall(t => true /* pending[t] was empty at commit */)

  // I3: a TVIEW row is created only by a transaction holding its key lock.
  val createdUnderKeyLock = true /* every tv version with no predecessor: (xmin, KeySpace(rel, key), Exclusive) ∈ locks at creation */

  // I4: every deadlock is in one of ADR 0207's classes ("What can still deadlock"):
  // multi-statement, rows found by the second lookup only, the flush.
  val deadlocksClassified = true
}
```

What the fuzzer already checks of these at scale is in
[`test/concurrency/fuzz/README.md`](../../test/concurrency/fuzz/README.md).
