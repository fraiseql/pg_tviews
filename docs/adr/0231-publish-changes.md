# ADR 0231: Publish changed TVIEW rows at commit

- Status: Accepted
- Related: [ADR 0220](0220-settings.md) (who may set what), [ADR 0221](0221-observability.md),
  #231, #230, fraiseql/fraiseql#1576, #1577

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

### 3. Payload, version 1

```json
{"v":1,"u":{"User":["8f…","1c…"],"Post":["…"]},"d":{"Comment":["…"]},"t":["Tag"]}
```

- keys of `u` and `d`: GraphQL type names, as `flush_and_report` emits them (the `typename`
  option, else the PascalCase of the entity);
- `u`: rows inserted or updated, `d`: rows deleted, by public `id` as text, net for the
  transaction (inserted then deleted: absent; deleted then inserted: in `u`);
- `t`: types that changed and whose ids are not listed: past the cap, or a TVIEW without an
  `id` column;
- every key but `v` may be absent; `types` mode sends `{"v":1,"t":[…]}`;
- each message is under 8,000 bytes; a transaction sends at most **8 messages** (≈ 1,500
  UUIDs). Past that, the remaining types go to `t` of the last message.

The grammar and examples live in the [change feed guide](../user-guides/change-feed.md).

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
`publish_skipped` in `pg_tviews_queue_stats()`. Otherwise a listener idle in a transaction
would eventually fail every write. The check is a threshold, not a guarantee: another
backend can fill the last pages first, and that commit then fails as PostgreSQL decides.

### 7. Who can read the feed

LISTEN has no ACL. Any role that can connect to the database receives every message: the ids
and type names of changed rows, and the timing of writes, including rows the role cannot
SELECT (other tenants', under RLS). Publishing stays a superuser decision. Use `types` where
ids are sensitive.

## Consequences

- A cache stays fresh across processes and replicas without polling. It evicts by id where
  it can, and by type past the cap or after a reconnect.
- Off by default: nothing changes for a database that doesn't opt in.
- A write's commit can't fail because of publishing, except in the documented queue race.
- Two-phase commit and publishing exclude each other, loudly.
- A durable feed (CDC) can be added later on the same per-transaction set, as a table.
