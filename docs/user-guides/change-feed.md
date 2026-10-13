# The change feed: caching TVIEW rows across processes

A TVIEW row is fresh when its writing transaction commits. A cache in another
process, or in front of a read replica, holds copies it cannot know are stale.
With publishing on, every committed transaction that changed TVIEW rows says which,
on the PostgreSQL channel `tviews_changes`. A cache LISTENs and evicts what changed.
The decision and its trade-offs are [ADR 0231](../adr/0231-publish-changes.md).

Inside the writing transaction, a mutation reports what it changed with
[`pg_tviews_flush_and_report()`](graphql-cascade.md). The feed is for everyone else,
after the commit.

## Turning it on

A superuser setting: a writing session cannot turn it off.

```sql
ALTER DATABASE app SET pg_tviews.publish_changes = 'rows';  -- or 'types', or 'off' (the default)
```

| value | sends |
|---|---|
| `off` | nothing, and keeps nothing: no cost |
| `types` | the GraphQL types that changed: `{"v":1,"t":["Post","User"]}` |
| `rows` | the rows that changed, by type and id, and types past the cap |

`rows` costs about 2-3 % on a write that changes 60 TVIEW rows (stock PostgreSQL 18.4,
`test/bench/write_path`, `username`); `types` and `off` cost nothing measurable.

## Payload, version 1

One JSON object per message, under 8,000 bytes:

```json
{"v":1,"u":{"User":["8f6a…"],"Post":["1c2b…","77d0…"]},"d":{"Comment":["e5f1…"]},"t":["Tag"]}
```

| key | meaning |
|---|---|
| `v` | payload version: `1`. Ignore messages of a version you don't know, and evict every type |
| `u` | rows inserted or updated, by type: `{"Type": [id, …]}` |
| `d` | rows deleted, by type |
| `t` | types that changed whose rows are not listed: evict the whole type |

- **Types** are GraphQL type names, as `pg_tviews_flush_and_report()` reports them: the
  TVIEW's `typename` option, else its entity in Pascal case (`blog_post` → `BlogPost`).
- **Ids** are the rows' public `id` column, as JSON: a uuid is a string, an integer id a
  number.
- **Net changes**: a row inserted and deleted in the same transaction is not listed;
  deleted and inserted again, it is in `u`; written twice, it is listed once.
- **`t`** lists a type when its ids did not fit (see below), or when its TVIEW has no `id`
  column. A type in `t` may also have some of its ids in `u` or `d`: evicting the type
  covers them.
- Every key but `v` may be absent.

A transaction sends at most 8 messages with ids (about 1,500 uuids). The remaining types
go to `t` of the last message. A large transaction is therefore always fully described.
Past a point, it is described by type instead of by id.

## Delivery

- Only committed transactions publish, in commit order. A rolled-back transaction, or a
  savepoint rolled back, publishes nothing for its rows.
- All the messages of one transaction are delivered together.
- The writing session receives them too, if it listens.
- A write made inside a function (a FraiseQL mutation called by `SELECT`) publishes like
  any other.

## The reconnect rule

**Notifications sent while a listener is not connected are lost.** On every connection
(the first one, and after any error on the listening connection), a cache must:

1. `LISTEN tviews_changes`;
2. evict everything it holds (every type);
3. then evict as messages arrive.

In this order, a change committed between steps 1 and 2 is covered by step 2. One
committed before step 1 is never seen, which step 2 covers too.

### Python (psycopg 3)

```python
import json
import time

import psycopg


def listen(dsn: str, cache) -> None:
    """Keep `cache` fresh; reconnect (and evict everything) after any error."""
    while True:
        try:
            with psycopg.connect(dsn, autocommit=True) as conn:
                conn.execute("LISTEN tviews_changes")
                cache.evict_all()
                for note in conn.notifies():
                    change = json.loads(note.payload)
                    if change.get("v") != 1:
                        cache.evict_all()
                        continue
                    for section in ("u", "d"):
                        for typename, ids in change.get(section, {}).items():
                            cache.evict_rows(typename, ids)
                    for typename in change.get("t", []):
                        cache.evict_type(typename)
        except psycopg.OperationalError:
            time.sleep(1)
```

### Rust (tokio-postgres)

```rust
use futures_util::{StreamExt, stream};
use tokio_postgres::{AsyncMessage, NoTls};

/// Evict from `cache` what each committed transaction changed, until the
/// connection fails; the caller reconnects (and so evicts everything again).
async fn listen(dsn: &str, cache: &Cache) -> Result<(), tokio_postgres::Error> {
    let (client, mut connection) = tokio_postgres::connect(dsn, NoTls).await?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let messages = stream::poll_fn(move |cx| connection.poll_message(cx));
    tokio::spawn(messages.for_each(move |message| {
        if let Ok(AsyncMessage::Notification(note)) = message {
            let _ = tx.send(note);
        }
        async {}
    }));
    client.batch_execute("LISTEN tviews_changes").await?;
    cache.evict_all();
    while let Some(note) = rx.recv().await {
        let change: serde_json::Value =
            serde_json::from_str(note.payload()).unwrap_or_default();
        if change["v"] != 1 {
            cache.evict_all();
            continue;
        }
        for section in ["u", "d"] {
            for (typename, ids) in change[section].as_object().into_iter().flatten() {
                cache.evict_rows(typename, ids.as_array().into_iter().flatten());
            }
        }
        for typename in change["t"].as_array().into_iter().flatten() {
            cache.evict_type(typename.as_str().unwrap_or_default());
        }
    }
    Ok(()) // the connection ended
}
```

## Connection poolers

LISTEN needs a session of its own: through pgbouncer, connect the listener with a
`session` pool (or directly to PostgreSQL), never a `transaction` pool. A transaction
pool can hand the LISTEN to another client and lose the notifications. Writers can use
any pool mode.

## Operating it

- **A listener that stops reading holds the queue.** PostgreSQL keeps every notification
  until every listening session has read it. A listener idle in a transaction reads
  nothing. Watch `pg_notification_queue_usage()` and, in `pg_stat_activity`, the
  sessions `idle in transaction` that LISTEN.
- **Above 90 % queue usage, transactions commit unpublished.** pg_tviews does not let a
  stuck listener fail every write. The writer gets a WARNING naming the cause, and
  `pg_tviews_queue_stats()->'publish_skipped'` counts these transactions in the session.
  Listeners missed them: that is why a cache evicts everything when it reconnects.
  The check is made just before the commit; another session can fill the last pages in
  between, and PostgreSQL then fails that commit (`too many notifications in the NOTIFY
  queue`).
- **PREPARE TRANSACTION is refused** (`25000`) while the transaction has changes to
  publish: PostgreSQL cannot carry a notification through PREPARE. Commit it without
  two-phase commit, or turn publishing off for the role that prepares
  (`ALTER ROLE batch SET pg_tviews.publish_changes = off`, as a superuser).

## Who can read it

LISTEN has no privilege. Any role that can connect to the database receives every
message: the type names and ids of changed rows, and when writes happen, including rows
the role cannot SELECT (other tenants' rows, under row-level security). Where ids are
sensitive, publish `types` only.
