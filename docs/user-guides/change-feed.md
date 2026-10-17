# The change feed: caching TVIEW rows across processes

A TVIEW row is fresh when its writing transaction commits. A cache in another
process, or in front of a read replica, holds copies it cannot know are stale.
With publishing on, every committed transaction that changed TVIEW rows says which,
on the PostgreSQL channel `tviews_changes`. A cache LISTENs and evicts what changed.
The decision and its trade-offs are [ADR 0231](../adr/0231-publish-changes.md).

Inside the writing transaction, a mutation reports what it changed with
[`pg_tviews_flush_and_report()`](graphql-cascade.md), collection keys included (its
`collections`, from the same capture as `c`). The feed is for everyone else, after the
commit.

## Turning it on

A superuser setting: a writing session cannot turn it off.

```text
ALTER DATABASE app SET pg_tviews.publish_changes = 'rows';  -- or 'types', or 'off' (the default)
```

| value | sends |
|---|---|
| `off` | nothing, and keeps nothing: no cost |
| `types` | the GraphQL types that changed: `{"v":2,"t":["Post","User"]}` |
| `rows` | the rows that changed, by type and id, the collections they left or joined, and types past the cap |

What `rows` costs, measured server-side on stock PostgreSQL 18.4 (`test/bench/write_path`,
one write per transaction, `synchronous_commit = off`, no client round trip), against
the same build with publishing `off`:

| write | `rows`, without collection keys (0.1.0-beta.29) | `rows` |
|---|---:|---:|
| `username`: a rename fanned out to 60 rows of three TVIEWs with references | +14 % | +30 % |
| `comment_insert`: one `tv_comment` row with two references | +14 % | +36 % |

The collection keys cost an index probe per reference of each recomputed row (its
parent's id), and the parents' ids in the payload. End to end, with client round trips
(pgbench), `rows` cost 2-3 % on the rename before collection keys existed. `types` and
`off` cost nothing measurable. Publishing internal keys instead of ids would save 6–9
points of it, but would put them on a channel any connected role can read
([ADR 0231](../adr/0231-publish-changes.md) §8).

## Payload, version 2

One JSON object per message, under 8,000 bytes:

```json
{"v":2,"u":{"User":["8f6a…"],"Comment":["1c2b…","77d0…"]},"d":{"Tag":["e5f1…"]},
 "c":{"Comment.fk_author":["8f6a…"],"Comment.fk_post":["9a03…"]},"t":["Audit"]}
```

| key | meaning |
|---|---|
| `v` | payload version: `2`. Ignore messages of a version you don't know, and evict every type |
| `u` | rows inserted or updated, by type: `{"Type": [id, …]}` |
| `d` | rows deleted, by type |
| `c` | collections that changed, by collection key: `{"Child.column": [parent id, …]}` ([below](#collection-keys)) |
| `t` | types that changed whose rows or collections are not listed: evict the whole type and its collections |

- **Types** are GraphQL type names, as `pg_tviews_flush_and_report()` reports them: the
  TVIEW's `typename` option, else its entity in Pascal case (`blog_post` → `BlogPost`).
- **Ids** are the rows' public `id` column, as JSON: a uuid is a string, an integer id a
  number.
- **Net changes**: a row inserted and deleted in the same transaction is not listed;
  deleted and inserted again, it is in `u`; written twice, it is listed once.
- **`t`** lists a type when its ids did not fit (see below), when its TVIEW has no `id`
  column, or when the parents of one of its collections cannot be listed. A type in `t`
  may also have some of its ids in `u` or `d`: evicting the type covers them.
- Every key but `v` may be absent.

A transaction sends at most 8 messages with ids (about 1,500 uuids). The remaining types
go to `t` of the last message. A large transaction is therefore always fully described.
Past a point, it is described by type instead of by id.

Version 1, before collection keys, had no `c`.

## Collection keys

A GraphQL layer that composes a list at read time ([Embed or
compose](embed-or-compose.md)), the comments of a post read from `tv_comment`, caches a
result that no row tag keeps fresh: adding a comment to post P changes no `tv_post` row,
and the new comment's id was never in P's result. Tag such a result with a **collection
key** for each parent it composes:

- the parent's row tag, as for any result: `Post:<P's id>`;
- the collection: `Comment.fk_post:<P's id>`, the child type (`tviews.registry.typename`
  of `tv_comment`), a dot, the reference column (its `tviews.registry.references`), a
  colon and the parent's id. Both come from the registry: nothing to spell by hand.

A page of 20 posts carries both tags for each of its 20 posts.

Every published row of a TVIEW with references names, in `c`, the parents of each of its
references, before and after the write, each once per transaction:

| event | feed | evicts |
|---|---|---|
| comment added to P | `u:{Comment:[c]}`, `c:{Comment.fk_post:[P]}` | P's composed results |
| comment moved from P to Q | `u:{Comment:[c]}`, `c:{Comment.fk_post:[Q,P]}` | both |
| comment edited | `u:{Comment:[c]}`, `c:{Comment.fk_post:[P]}` | P's |
| author renamed (50 comments on 10 posts) | `u:{User:[a], Comment:[…]}`, `c:{Comment.fk_post:[…10 posts], Comment.fk_author:[a]}` | those posts', and the user's comment lists |
| comment deleted | `d:{Comment:[c]}`, `c:{Comment.fk_post:[P]}` | P's |
| post deleted | `d:{Post:[P]}` | P's, through its row tag |

Eviction:

- `u` and `d` evict row tags; `c` evicts collection tags;
- a type in `t` evicts its rows and every collection tag of that type (`Comment.*`): keep
  an index from child type to its collection tags;
- the reconnect rule (evict everything) is unchanged.

A parent the transaction deleted names no collection: its own row tag evicts its results.
A new row in a list query (a new post on page 1) follows the cache's rule for new rows of a
type, unchanged.

**Before payload version 2** (a cache that does not tag collections), the rule is coarse:
any `Comment` row in `u` or `d` evicts every result that composes `Comment` children.

## Delivery

- Only committed transactions publish, in commit order. A rolled-back transaction, or a
  savepoint rolled back, publishes nothing for its rows.
- All the messages of one transaction are delivered together.
- The writing session receives them too, if it listens.
- A write made inside a function (a FraiseQL mutation called by `SELECT`) publishes like
  any other.
- A TVIEW rebuilt whole (`pg_tviews_refresh()`, the catch-up of a suspended transaction,
  the refill of a reset UNLOGGED TVIEW, `pg_tviews_rebuild_all()`) is published by type
  in `t`: its rows are not listed one by one.

## The reconnect rule

**Notifications sent while a listener is not connected are lost.** On every connection
(the first one, and after any error on the listening connection), a cache must:

1. `LISTEN tviews_changes`;
2. evict everything it holds (every type);
3. then evict as messages arrive.

In this order, a change committed between steps 1 and 2 is covered by step 2. One
committed before step 1 is never seen, which step 2 covers too.

### Gaps while connected

A listener can also miss transactions without losing its connection: when the NOTIFY
queue is over 90 % full (another listener stopped reading), transactions commit
unpublished ([Operating it](#operating-it)). `tviews.pg_tviews_feed_gaps()` counts them
for the whole cluster since the server started, and any session can read it:

1. at connect, after `LISTEN`, read it and keep the value;
2. on each heartbeat (a few seconds), read it again; when it moved, evict everything and
   keep the new value.

A gap is counted once its transaction has committed, so a listener that evicts when the
counter moves reads the missed transaction's rows fresh. A server restart
starts the counter over, and also drops every listening connection, which the reconnect
rule already covers. The counter needs `shared_preload_libraries = 'pg_tviews'`;
without it the function fails (`55000`) rather than report no gaps.

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
                    if change.get("v") != 2:
                        cache.evict_all()
                        continue
                    for section in ("u", "d"):
                        for typename, ids in change.get(section, {}).items():
                            cache.evict_rows(typename, ids)
                    for key, parent_ids in change.get("c", {}).items():
                        cache.evict_collections(key, parent_ids)
                    for typename in change.get("t", []):
                        cache.evict_type(typename)  # its rows and its collections
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
        if change["v"] != 2 {
            cache.evict_all();
            continue;
        }
        for section in ["u", "d"] {
            for (typename, ids) in change[section].as_object().into_iter().flatten() {
                cache.evict_rows(typename, ids.as_array().into_iter().flatten());
            }
        }
        for (key, parent_ids) in change["c"].as_object().into_iter().flatten() {
            cache.evict_collections(key, parent_ids.as_array().into_iter().flatten());
        }
        for typename in change["t"].as_array().into_iter().flatten() {
            // Its rows and its collections.
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
  stuck listener fail every write. The writer gets a WARNING naming the cause (it also
  goes to the server log, where monitoring sees it), and `pg_tviews_feed_gaps()` counts
  these transactions for the whole cluster. Listeners missed them: a connected
  listener learns it from the counter ([gaps](#gaps-while-connected)), a reconnecting
  one evicts everything anyway.
  The check is made just before the commit; another session can fill the last pages in
  between, and PostgreSQL then fails that commit (`too many notifications in the NOTIFY
  queue`).
- **PREPARE TRANSACTION is refused** (`25000`) while the transaction has changes to
  publish: PostgreSQL cannot carry a notification through PREPARE. Commit it without
  two-phase commit, or turn publishing off for the role that prepares
  (`ALTER ROLE batch SET pg_tviews.publish_changes = off`, as a superuser).

## Who can read it

LISTEN has no privilege. Any role that can connect to the database receives every
message: the type names and ids of changed rows, the ids of their parents in `c`, and when
writes happen, including rows the role cannot SELECT (other tenants' rows, under
row-level security). Where ids are sensitive, publish `types` only. Publishing asks
nothing more of the writer than the write does
([Privileges](../reference/api.md#privileges)).
