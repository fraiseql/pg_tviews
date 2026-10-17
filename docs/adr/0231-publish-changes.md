# ADR 0231: Publish changed TVIEW rows at commit

- Status: Accepted
- Related: [ADR 0220](0220-settings.md) (who may set what), [ADR 0221](0221-observability.md),
  #231, #230, fraiseql/fraiseql#1576, #1577
- Amended: payload version 2 adds collection keys (`c`, §8)
- Amended 2026-10-17 (beta.32): `publish_skipped` removed from `pg_tviews_queue_stats()`;
  `pg_tviews_feed_gaps()` is the one count of skipped transactions (§6)

## Context

A write changes TVIEW rows in the writer's transaction. Inside that transaction,
`pg_tviews_flush_and_report()` says which rows changed: a GraphQL server answers its own
mutation with them. Nothing tells anyone else. A cache on another process, or in front of a
replica, holds documents it cannot know are stale: it expires them by time, or never.

The journal the report reads lives in the writer's backend and dies with its transaction. It
is a cursor for the report (reset as rows are reported, capped by `report_max_tracked`, off at
`0`), not a record of what the transaction changed.

Three ways to tell other processes:

- **(A) NOTIFY with type names only.** Small and simple; a cache evicts whole types.
- **(B) A change-log table** polled by position: durable and replayable, but one more heap
  write and its WAL per changed row, on every write, plus retention. Caches don't need
  replay.
- **(C) NOTIFY with ids, chunked**, type names past a cap.

A spike (stock PostgreSQL 18.4, a C module calling `Async_Notify` from the transaction
callback) established:

| Fact | Consequence |
|---|---|
| `Async_Notify` at `XACT_EVENT_PRE_COMMIT` needs no SPI, and is delivered only if the transaction commits, in commit order | one place covers autocommit, `COMMIT`, procedures and function-driven writes |
| an error at PRE_COMMIT (a payload ≥ 8,000 bytes, a full NOTIFY queue) rolls the user's transaction back | publishing must never raise |
| `PREPARE TRANSACTION` refuses a transaction that queued a NOTIFY | a prepared transaction cannot publish |
| several `Async_Notify` calls in one transaction arrive together at commit, in order | a large change set is several messages of one shape |
| identical channel + payload in one transaction are sent once | messages never repeat within a transaction anyway |

## Decision

### 1. Mechanism: NOTIFY at PRE_COMMIT

The transaction callback, at PRE_COMMIT and after the refresh queue is known empty, sends
the transaction's net changes on the channel **`tviews_changes`** (fixed, by convention; a
LISTEN is per database already). The changed rows are kept in a per-transaction publish set
of their own, apart from the report's journal, exact under savepoints. Ids and type names
are captured by the refresh writes themselves (`RETURNING`), while SPI is available: nothing
is looked up at PRE_COMMIT.

### 2. Setting

`pg_tviews.publish_changes = off | types | rows`, default `off`, **superuser**
(`ALTER DATABASE app SET …`, `ALTER SYSTEM`). Listeners rely on it, so a writing session
must not be able to turn it off (ADR 0220 §3). `off` keeps no set and costs one comparison
per journaled row.

### 3. Payload, version 2

```json
{"v":2,"u":{"User":["8f…","1c…"],"Post":["…"]},"d":{"Comment":["…"]},
 "c":{"Comment.fk_post":["9a…"]},"t":["Tag"]}
```

- keys of `u` and `d`: GraphQL type names, as `flush_and_report` emits them (the `typename`
  option, else the PascalCase of the entity);
- `u`: rows inserted or updated, `d`: rows deleted, by public `id` as text, net for the
  transaction (inserted then deleted: absent; deleted then inserted: in `u`);
- `c`: collection keys (§8), each the public ids of the parents whose child rows changed;
- `t`: types that changed and whose ids are not listed: past the cap, a TVIEW without an
  `id` column, or the child type of a collection whose parents are not listed. A type in
  `t` stands for its rows and for its collections;
- every key but `v` may be absent; `types` mode sends `{"v":2,"t":[…]}`;
- each message is under 8,000 bytes; a transaction sends at most **8 messages** (≈ 1,500
  UUIDs). Past that, the remaining types go to `t` of the last message.

Version 1 had no `c`. A listener that does not know a version evicts everything.

The grammar, examples and listener code live in the
[change feed guide](../user-guides/change-feed.md), the one source for consumers.

### 4. The consumer contract

- Delivery happens only on commit, in commit order, all messages of a transaction together.
  The writer's own session receives them too.
- The feed is lossy. Notifications sent while a listener is disconnected are lost, and so are
  those of a transaction that skipped publishing (§6). A cache evicts every type when it
  connects, reconnects, or gets an error on its LISTEN connection, then listens.
- `flush_and_report(types)` filters only the report, never the feed (#230): a cache elsewhere
  needs every change.

### 5. PREPARE TRANSACTION

With publishing on, preparing a transaction that changed TVIEW rows fails with `25000`. The
hint says to commit without two-phase, or to have a superuser turn publishing off for that
role. PostgreSQL cannot carry the notification through PREPARE, and committing unpublished
would leave caches stale with no signal. It shares one error variant, `PrepareRefused
{ reason }`, with the refusal of a prepared refill claim (#215).

### 6. A full NOTIFY queue

Above 90 % queue usage (`pg_notification_queue_usage()`, read without SPI), the transaction
commits unpublished. It emits a WARNING naming the stuck listener's remedy, and adds one to
a cluster-wide counter in shared memory, `pg_tviews_feed_gaps()` (#247; a per-session
`publish_skipped` in `pg_tviews_queue_stats()` counted them too until beta.32): a listener that stays connected (the healthy ones,
while another holds the queue) learns from it that it missed transactions, and evicts
everything. One counter for the cluster, not per database: the queue is the cluster's, a
skip is an incident, and an eviction in another database costs nothing but a refill. The
decision is made at `PRE_COMMIT`, the count at `COMMIT`, once the transaction is visible:
counted earlier, a listener could evict, refill from a snapshot without the transaction,
and keep those rows stale with no further signal. The counter is an atomic in the
`tviews.stats` shared segment: no lock. Otherwise a listener idle in a transaction
would eventually fail every write. The check is a threshold, not a guarantee: another
backend can fill the last pages first, and that commit then fails as PostgreSQL decides.

### 7. Who can read the feed

LISTEN has no ACL. Any role that can connect to the database receives every message: the ids
and type names of changed rows, and the timing of writes, including rows the role cannot
SELECT (other tenants', under RLS). Publishing stays a superuser decision. Use `types` where
ids are sensitive.

### 8. Collection keys

A reader that composes a child list at read time (the comments of a post, read from
`tv_comment` by `fk_post`) caches a result no row tag can keep fresh: adding a comment to
post P, or moving one into it, changes no `tv_post` row, and the new comment's id was never
in P's result. Such a result is tagged with a **collection key**, `<child type>.<reference
column>:<parent id>` (`Comment.fk_post:<P's id>`), one per composed parent, besides the
parent's row tag.

- Every published row of a TVIEW with references (`tviews.registry.references`) names, in
  `c`, the parents of each reference before and after the write: the post a comment was
  inserted into, moved out of and into, edited in, or deleted from. Parents are listed
  once per transaction, new before old.
- The refresh write returns them. A recompute, an insert or a delete returns, per
  reference, the parent's key and its public id, read by an index probe in `RETURNING`.
  The keys the row held before come from the read that locks the rows before the
  recompute (`FOR UPDATE` under READ COMMITTED, so they are what a concurrent writer
  committed); where that read is skipped (REPEATABLE READ without key locks), a subquery
  in `RETURNING` reads them, since the statement's snapshot does not see the row version
  being written. A patch of `data` (direct or fan-out) changes no reference: it returns
  the keys only, over rows that can be many.
- At the end of the flush, while SPI is available, the keys left unresolved are resolved
  from the rows the transaction itself wrote (a fan-out from a renamed author has written
  the posts and the user), the others with one query for every parent TVIEW. A parent no
  row of its TVIEW holds names nothing: the transaction deleted it (its row tag evicts
  it) or it was never one of its rows. A key left unresolved at commit, and a parent
  TVIEW without an `id` column, publish the child type in `t`.
- Every reference is published (no declaration selects them); the parents and the budget
  are those of `u`.
- `pg_tviews_flush_and_report()` reads the same capture before commit (#244): a layer
  that ran the write evicts by collection synchronously, from `collections`, instead of
  every result composing the child type. It lists the parents the rows it reports named,
  under the report's `reset`, `types` and savepoints, resolved as the last flush left
  them; a parent the feed would publish by type puts the child type in
  `invalidated_types`, and a type there stands for all its collections, as in `t`. The
  rows must have been written with `rows`: otherwise nothing was captured and the
  report has no `collections`, whatever the setting when it runs.
- A collection's parent is the TVIEW `tviews.registry.references` names: of two TVIEWs
  named by the same key, the one whose identity column has the referenced column's name,
  else the first by entity. One parent per reference keeps one key per composed list,
  and the rule follows the naming convention, with nothing to declare.
- The cost is accepted. Against `rows` without collection keys (0.1.0-beta.29), on
  TVIEWs with references, a fanned-out rename costs +15–17 % and a comment insert
  +20–21 % server-side (`test/bench/write_path`, stock 18.4; `off` and `types` within
  ±1.5 %). It is paid only where a superuser turned `rows` on, and it buys the one
  eviction a composed list cannot do without. Two cheaper designs were rejected:
  publishing the parents' internal keys (`pk_*`) instead of their ids saves 6–9 points
  but puts internal keys on a channel any connected role can read and makes caches tag
  by them; publishing only the references a declaration marks is configuration a
  generated schema should not need, and a forgotten declaration leaves lists stale
  silently.

## Consequences

- A cache stays fresh across processes and replicas without polling. It evicts by id where
  it can, by collection for results composed at read time, and by type past the cap or after
  a reconnect.
- Off by default: nothing changes for a database that doesn't opt in.
- A write's commit can't fail because of publishing, except in the documented queue race.
- Two-phase commit and publishing exclude each other, loudly.
- A durable feed (CDC) can be added later on the same per-transaction set, as a table.
