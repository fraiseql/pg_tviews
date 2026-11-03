# ADR 0286: Refresh once per top-level statement, or at the first read of a TVIEW

- Status: Accepted (0.1.0-beta.36)
- Related: [ADR 0207](0207-concurrent-maintenance.md) (value locks), [ADR 0220](0220-settings.md)
  (`max_refresh_rows`), [concurrency](../concurrency.md), #286, fraiseql/fraiseql#1897

## Context

A fraiseql mutation is one statement, `SELECT app.fn(…)`, whose PL/pgSQL function writes in
several statements and usually reads its result back from `tv_*` before it returns.
pg_tviews refreshed at the end of every statement that wrote a base table, unless an
enclosing statement also wrote one: a `SELECT` that only calls the function deferred
nothing. A function writing one row in 13 statements refreshed that row, and every TVIEW
row embedding it, 13 times.

Measured with `test/bench/write_path/mutations.sh` (stock PostgreSQL 18, one client,
printoptim-shaped model; per-statement form vs the same change written once):

| mutation | per statement | written once |
|---|---|---|
| `update_location` (13 single-field `UPDATE`s, embedded in 10 rows) | 7.4–9.4 ms, 142 TVIEW rows written | 0.56–0.78 ms, 11 rows |
| `log_events` (10 child `INSERT`s) | 9.2–11.0 ms, 110 rows | 0.81–1.49 ms, 11 rows |
| `create_location` (`INSERT`, then `UPDATE` of its identifier) | 0.78–1.34 ms, 2 rows | 0.22–0.32 ms, 1 row |

Asking every mutation to "write each row once" (fraiseql/fraiseql#1897) puts a storage
engine's cost model into application code. A study of printoptim's mutations found
`update_location` refreshing one row about 17 times, every create twice, and 71 of 90
mutations reading `v_`/`tv_` after writing.

## Decision

The TVIEWs are refreshed **once per top-level statement**: at its end, or earlier, at the
first query inside it that reads a TVIEW's table, whichever comes first. One mode, no
setting.

- **Frames** (`src/executor.rs`): `ExecutorRun` and `ExecutorFinish` push a frame for their
  duration. The statement-level flush trigger leaves the queue alone while any frame
  encloses the statement it fires for.
- **End of the top-level statement**: one that writes a table carrying a flush trigger
  refreshes in `ExecutorFinish`, after its `AFTER` triggers; one that only reads
  (`SELECT app.fn(…)`) when its `ExecutorRun` returns. A portal finishes a `SELECT` after
  popping its snapshot, where SPI cannot run.
- **A read refreshes first** (`ExecutorStart`): a query whose range table holds a TVIEW's
  table (a view over one included: the rewriter expands it there) while work is queued
  flushes it, then runs on a **snapshot taken after the flush**, as the next statement of a
  function would under `READ COMMITTED`. The query's descriptor registers the new snapshot
  in place of the one it was given, and the hooks make it active around each executor call
  on the query (`standard_ExecutorStart` and `standard_ExecutorRun` assert the query's
  snapshot is active); the given one is left alone. Advancing the given snapshot's command
  id instead was tried and is wrong twice: under concurrency it sees a row both as a
  transaction that committed meanwhile left it and as the flush rewrote it (the fuzzer's
  in-mutation read found duplicate keys), and a STABLE function's queries share their
  caller's snapshot, whose running scan would then meet rows the flush wrote.
- **A snapshot older than the last refresh**: a query reading a TVIEW on a snapshot taken
  before the transaction's last refresh (a STABLE function's query, on its caller's
  snapshot, after an earlier read in the same statement refreshed) also reads on a
  snapshot taken now.
- `pg_tviews`' own queries (the flush, catalog reads, a refresh API) never flush; nor do
  `EXPLAIN` without `ANALYZE`, parallel mode or a suspended session.
- **End of the top-level statement** also refreshes what a trigger of a write to a table
  no TVIEW reads queued (that trigger's write to a table a TVIEW does read).
- `CALL` and `DO` push no frame: each statement they run is top-level, as before.
- A subquery of a query already running (`SELECT app.create(x), (SELECT … FROM tv_post)`)
  was started with it, before the call wrote: it reads what that query's snapshot sees.
- A user `AFTER … FOR EACH ROW` trigger reading a TVIEW during the very statement whose
  write it fires for reads it as it was before that statement: that statement's work is
  its own to refresh when it ends, as before this change.
- A subtransaction that rolls back undoes a refresh it ran with its writes: the work is
  deferred again and refreshed by the statement, as if the read had not happened.

## Consequences

- A mutation costs what its rows cost, however its statements are written. Same bench,
  stock PostgreSQL 18.6 release packages, two rounds alternating builds (load 39-67, so
  the times are noisy; the rows written are exact):

  | mutation | before, per statement | after, per statement | after, written once |
  |---|---|---|---|
  | `update_location` | 9.8–10.2 ms, 141 rows | 0.93–1.14 ms, 11 rows | 0.88–1.47 ms, 11 rows |
  | `create_location` | 1.1–1.3 ms, 2 rows | 0.36–0.49 ms, 1 row | 0.34–0.58 ms, 1 row |
  | `log_events` | 7.8–9.5 ms, 110 rows | 1.5–2.4 ms, 11 rows | 1.3–1.9 ms, 11 rows |
- A refresh error surfaces at the read, or at the end of the top-level statement, outside
  the function's exception handlers. A handler that turns errors into a mutation result
  catches refresh errors only around a read of a TVIEW.
- `pg_tviews.max_refresh_rows` counts one deferred refresh: the rows a whole mutation
  reaches, not one of its statements.
- Value locks (ADR 0207) are taken at capture and held to the end of the transaction, as
  before: deferring the refresh does not hold them longer. A rolled-back subtransaction
  inside the mutation restores the queued work, the value locks and the frames
  (`src/flush/savepoint.rs`), so its writes are never refreshed.
- A query that refreshed, or whose snapshot predated a refresh, reads on a newer snapshot
  than the statement that called it: a STABLE function then sees transactions its caller's
  snapshot does not, as a VOLATILE one does under `READ COMMITTED`. A cursor fetched
  after a refresh in its transaction reads its TVIEW-reading functions' rows on such a
  snapshot too, the rest of its rows on its own.
- A read between two writes of the same rows refreshes them twice; read the result back
  once, after the last write.
- Every query pays one thread-local check in `ExecutorStart` and `ExecutorRun`; the range
  table is read only while work is queued, and the set of TVIEW tables is a backend memo.

## Alternatives

- **Per transaction** (refresh at `COMMIT`): reads inside the transaction see stale
  TVIEWs, and fraiseql mutations read `tv_*` before returning. Rejected (D7, 2026-11-02).
- **Flush before any read inside a mutation**: simpler, but mutations read `tb_`/`v_`
  between writes; most of the gain is lost.
- **A setting to keep per-statement refresh**: two modes to test and document forever.
