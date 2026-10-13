# Architect Guide

Design patterns and trade-offs for building CQRS systems with pg_tviews and FraiseQL.

## Overview

pg_tviews is the read-model side of a CQRS design that stays inside one PostgreSQL
database. The application writes normalized tables; pg_tviews keeps denormalized `tv_*`
tables (TVIEWs) up to date from them, in the writing transaction.

```text
Command side (write)              Query side (read)
+------------------+              +------------------+
| FraiseQL         |              | GraphQL          |
| mutations        |              | queries          |
+--------+---------+              +--------+---------+
         |                                 |
         v                                 v
+------------------+   triggers   +------------------+
| write tables     | -----------> | tv_* tables      |
| (normalized)     |  same txn    | (denormalized)   |
+------------------+              +------------------+
```

What this gives the design:

- **Read-your-writes**: a TVIEW reflects a write at the end of the statement that made it,
  in that transaction. Other transactions see both when it commits.
- **Incremental cost**: a write refreshes the TVIEW rows it reaches, not the whole TVIEW.
  [Benchmarks](../benchmarks/overview.md) compare this with `REFRESH MATERIALIZED VIEW`.
- **No refresh code**: no cache invalidation or projection code in the application.

## How a TVIEW is maintained

`CREATE TABLE tv_<entity> AS SELECT …` (or `tviews.pg_tviews_create()`) stores the SELECT
as a backing view, `tviews.<schema>__tv_<entity>`, fills the table from it, and analyses the
definition's query tree into one stored **propagation plan**. The plan records, for each
table the definition reads, how a write to it maps to TVIEW keys, and which other TVIEWs
the TVIEW embeds. A trigger on each of those tables reads only the plan:

- A write to a table whose columns are copied into `data` unchanged is **patched in place**.
- Any other write **recomputes** the affected rows from the backing view, filtered on their
  keys.
- A refreshed TVIEW row refreshes the rows of the TVIEWs that embed it (**propagation**).

Refresh work is queued by row triggers and run by a statement trigger at the end of each
statement. A commit with work still queued fails with SQLSTATE 55000 rather than committing
stale TVIEWs.

No naming convention is involved: a write table and its columns may have any names. The one
fixed name is the TVIEW's row identity, `pk_<entity>` for `tv_<entity>` (or its
`DISTINCT ON` key).

## Design patterns

The examples below run in order in one database.

```sql
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

### Identity

FraiseQL's trinity pattern gives each entity three identifiers, which a TVIEW projects:

- **`pk_<entity>`**: an integer key, the TVIEW's row identity, used to join and refresh.
- **`id`**: a UUID, the public GraphQL ID.
- **`identifier`**: an optional slug for URLs.

The write tables need not follow it. Here the tables are `account`, `article` and `review`,
and the foreign keys are `author_ref` and `article`:

```sql
CREATE TABLE account (
    account_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name TEXT NOT NULL
);

CREATE TABLE article (
    article_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    author_ref BIGINT NOT NULL REFERENCES account (account_id)
);

CREATE TABLE review (
    review_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    article BIGINT NOT NULL REFERENCES article (article_id),
    rating INT NOT NULL CHECK (rating BETWEEN 1 AND 5)
);

INSERT INTO account (name) VALUES ('Alice'), ('Bob');
INSERT INTO article (identifier, title, author_ref) VALUES
    ('intro', 'Introduction', 1), ('design', 'Design notes', 2);
INSERT INTO review (article, rating) VALUES (1, 5), (1, 3);
```

### Composition: TVIEWs embedding TVIEWs

Model each GraphQL type once and embed it where it is used. `tv_author` is the `Author`
type; `tv_article` embeds its `data` by joining on its key:

```sql
CREATE TABLE tv_author AS
SELECT a.account_id AS pk_author,
       a.id,
       jsonb_build_object('id', a.id, 'name', a.name) AS data
FROM account a;

CREATE TABLE tv_article AS
SELECT ar.article_id AS pk_article,
       ar.id,
       ar.identifier,
       ar.author_ref AS author_pk,
       au.id AS author_id,
       jsonb_build_object(
           'id', ar.id,
           'title', ar.title,
           'author', au.data,
           'reviewCount', (SELECT count(*) FROM review r WHERE r.article = ar.article_id),
           'rating', (SELECT round(avg(r.rating), 2) FROM review r
                      WHERE r.article = ar.article_id)
       ) AS data
FROM article ar
JOIN tv_author au ON au.pk_author = ar.author_ref;
```

Renaming an account patches `tv_author`, which patches the `author` of every article
embedding it. A new review recomputes its article's counts:

```sql
UPDATE account SET name = 'Alicia' WHERE account_id = 1;
INSERT INTO review (article, rating) VALUES (1, 1);

SELECT identifier, data->'author'->>'name' AS author, data->>'reviewCount' AS reviews
FROM tv_article ORDER BY pk_article;
```

The registry shows what the plan derived: each table read, and how a write to it reaches
the TVIEW (`local`: the rows read it directly; `mapped`: through a key mapping;
`propagated`: through an embedded TVIEW).

```sql
SELECT entity, base_tables, cascade_kinds FROM tviews.registry ORDER BY entity;

SELECT * FROM tviews.pg_tviews_show_cascade_path('author');
```

Create a TVIEW before the TVIEWs that embed it.

### Aggregates

Per-entity aggregates (counts, averages, the last N items) fit in the entity's own TVIEW as
correlated subqueries, as `reviewCount` above. A read model whose rows are groups (one row
per customer per month) is an [aggregate TVIEW](aggregate-tviews.md), created with
`tviews.pg_tviews_create()` and its `group_keys` option, and refreshed group by group.

### What a definition cannot do

A TVIEW is refused when it is created, rather than going stale later, when a write could
change its rows without pg_tviews knowing which ones:

- a table read in a way no key traces back to the TVIEW's rows (the `uncascaded_policy`
  option decides: `error` by default, `full_refresh`, or `warn`);
- the current time (`now()`, `CURRENT_DATE`) unless it declares `"time_refresh": "external"`
  and something calls `tviews.pg_tviews_refresh_time_dependent()`;
- a non-immutable function that reads tables, unless the `function_reads` option names them;
- TVIEWs reading each other in a cycle (42P17).

Options are passed to `tviews.pg_tviews_create()` or
`tviews.pg_tviews_create_or_replace()`, and are the whole declaration: what a TVIEW is
comes only from its definition and its options, never from a setting. See the
[API reference](../reference/api.md).

## Cascade design

Each write costs the rows it reaches. Two dimensions drive it:

- **Fan-out**: a row embedded in many rows (a category in every product) refreshes all of
  them on every write to it ([below](#fan-out)). `tviews.pg_tviews_profile()` reports the
  fan-out of each lookup column, projects its cost per write and warns above a threshold.
- **Depth**: each level of TVIEW embedding TVIEW adds a round of refreshes. Keep chains
  short; `pg_tviews.max_propagation_depth` (default 100) stops a runaway chain.

`pg_tviews.max_queue_size` (default 1,000,000 keys) bounds the work one transaction can queue:
beyond it the write fails with SQLSTATE 54000. Split a write that reaches more rows into
several transactions, or suspend triggers for it (below).

```sql
SELECT entity, rows_estimate, warnings FROM tviews.pg_tviews_profile();
```

### Fan-out

A write to a column that N TVIEW rows embed rewrites those N documents in the writer's
statement, and their rows stay locked until its transaction ends. pg_tviews patches them in
place (`jsonb_set` at the embedded path) when it can, and recomputes them from their view
otherwise. What it costs ([ADR 0233](../adr/0233-fanout-cost-and-guard.md), stock
PostgreSQL 18.4 release package, one client):

| documents | path | per write | per embedding row | WAL per row |
|---|---|---:|---:|---:|
| ~1 KB, inline | patch | ~0.15 ms | ~2.8-3.7 µs | ~0.7-0.9 KB |
| ~1 KB, inline | recompute | ~0.2 ms | ~8 µs | ~0.8-1 KB |
| ~4 KB, TOASTed | patch or recompute | ~0.25 ms | ~21-30 µs | ~5-6 KB |

A TOASTed document is rewritten whole, so the patch saves nothing there. The patch reaches
an embed one join away (`tb_post.fk_author → tb_user`) or two (`tb_comment → tb_post →
tb_user`, found through the table in between); arrays of embedded rows (`jsonb_agg`) and
embeds three joins away are recomputed. With ~1 KB documents:

| fan-out N | per write | WAL per write | at 10 writes/s | at 100 writes/s |
|---:|---:|---:|---|---|
| 10 | 0.18 ms | 7 KB | 0.2 % of a core, 70 KB/s | 2 % of a core, 0.7 MB/s |
| 100 | 0.43 ms | 69 KB | 0.4 % of a core, 0.7 MB/s | 4 % of a core, 6.9 MB/s |
| 1,000 | 3.1 ms | 0.69 MB | 3 % of a core, 6.9 MB/s | 31 % of a core, 69 MB/s |
| 10,000 | 28 ms | 6.9 MB | 28 % of a core, 69 MB/s | 2.8 cores, 690 MB/s |
| 1,000,000 | ~2.8 s | ~690 MB | | |

With ~4 KB TOASTed documents, multiply both by about 7.

**Rule.** Above ~1,000 embedding rows per write at a sustained write rate, or ~100,000 at
all, don't embed. Reference the id instead and resolve it at read time, or make the change
in an offline batch. `tviews.pg_tviews_profile()` shows each lookup column's p99 fan-out
with its projected `p99_ms_per_write` and `p99_wal_bytes_per_write`, from the same model and
the TVIEW's bytes per row.

**Remedies** for the writes that remain large:

- `pg_tviews.max_refresh_rows` (superuser, `0` = off) caps the rows one statement's refresh
  writes: above it the statement fails with `54000` before writing them, naming the TVIEW,
  the lookup column, the count and the limit. Set it database-wide so that no single write
  can hold a million rows' locks by accident.
- A bulk rename: `tviews.pg_tviews_suspend_triggers()`, the change, then
  `pg_tviews_resume_triggers()` or the commit rebuilds each TVIEW it changed once. These
  rebuilds are not charged against `max_refresh_rows`.
- There is no deferred (asynchronous) refresh: a committed write is visible in every TVIEW
  when the commit returns, and pg_tviews keeps that contract.

To re-measure on your hardware: `test/bench/write_path/fanout.sh [seconds] [doc_bytes]`
on a release build.

## Performance architecture

### Read side

A TVIEW is a table: index it for the queries the API runs, on projected columns or
expressions over `data`.

```sql
CREATE INDEX idx_tv_article_author ON tv_article (author_id);
CREATE INDEX idx_tv_article_title ON tv_article ((data->>'title'));
CREATE INDEX idx_tv_article_fts
    ON tv_article USING gin (to_tsvector('english', data->>'title'));
```

Keep indexes on `data` few: nearly every refresh rewrites `data`, and every index on it makes
the refresh a non-HOT update. TVIEWs get a fillfactor of 85 (the `fillfactor` option) to
leave room for HOT updates.

### Write side

- A statement refreshes each affected TVIEW row once at its end, however many base rows it
  wrote: prefer set-based statements to row-by-row loops.
- For bulk loads, `tviews.pg_tviews_suspend_triggers()` defers refreshes, and
  `tviews.pg_tviews_resume_triggers()` rebuilds the TVIEWs written meanwhile.
- `tviews.pg_tviews_ensure_propagation_indexes()` creates the indexes the refresh lookups
  need.

### Read replicas

TVIEWs are LOGGED by default: a hot standby reads them like any table. A TVIEW declared
`logged: false` (or created with `CREATE UNLOGGED TABLE tv_x AS`) is cheaper to write, but
a standby cannot read it, and promotion or a crash restart empties it until a worker
refills it. `ALTER TABLE tv_x SET LOGGED` (or the `logged` option) switches it back:

```sql
SELECT entity, persistence, replica_readable FROM tviews.pg_tviews_replication_status();
```

A replica never refreshes a TVIEW: it replays the primary's. See
[Replication](../operations/replication.md).

## Consistency model

- **Transactional**: the write and its refreshes commit or roll back together.
- **Read-your-writes**: within the writing transaction, the next statement reads fresh
  TVIEWs.
- **Isolation**: other sessions see the refreshed rows when the writer commits, under
  PostgreSQL's usual rules. Concurrent writes reaching the same TVIEW row are serialized by
  the row locks the refresh takes.
- **Rendering**: refreshes render values under fixed settings (`TimeZone` UTC, `DateStyle`
  ISO), so a TVIEW's text does not depend on the writer's session.

```sql
BEGIN;
UPDATE article SET title = 'Draft' WHERE identifier = 'design';
SELECT data->>'title' AS title FROM tv_article WHERE identifier = 'design';  -- Draft
ROLLBACK;
```

## Security model

- A TVIEW is owned by its creator. Functions acting on one TVIEW (`pg_tviews_refresh`,
  `pg_tviews_reregister`, `pg_tviews_drop`, …) require owning it or the extension
  (42501 otherwise).
- Functions acting on every TVIEW (`pg_tviews_refresh_all()`, `pg_tviews_rebuild_all()`,
  `pg_tviews_reregister_all()`, …) are not executable by `PUBLIC`; grant them to the roles
  that run maintenance.
- Every rebuild runs as the TVIEW's owner, like `REFRESH MATERIALIZED VIEW`.

See [Operator Guide](operators.md) and [Security](../operations/security.md).

## Monitoring

```sql
SELECT * FROM tviews.pg_tviews_health_check() WHERE status <> 'OK';
SELECT entity, needs_reregister, uncascaded_tables FROM tviews.registry;
SELECT entity, view_recomputes, rows_written, refresh_ms FROM tviews.stats;
SELECT entity, persistence, heap_bytes, hot_ratio, warnings FROM tviews.pg_tviews_profile();
```

- `pg_tviews_health_check()`: catalog, plans, triggers, `jsonb_delta`; an empty result above
  is healthy.
- `tviews.registry`: one row per TVIEW, the stable read contract for tools
  ([read contract](../reference/read-contract.md)).
- `tviews.stats`: refresh work per TVIEW, cumulative, readable from any session.
- `pg_tviews_profile()`: sizes, dead tuples, fan-out, with warnings.
- `pg_tviews_queue_stats()`: the current session's refresh work, for debugging.

## Trade-offs

### TVIEW or materialized view

| Aspect | TVIEW | Materialized view |
|--------|-------|-------------------|
| Freshness | In the writing transaction | Until the next `REFRESH` |
| Refresh cost | Rows a write reaches | The whole view |
| Write cost | Each write pays its refresh | None until `REFRESH` |
| Definitions | Refused if a write cannot be traced | Any query |

A materialized view suits data refreshed on a schedule and read stale; a TVIEW suits data
read right after it is written, where writes touch a small share of the rows.

### JSONB documents or normalized read tables

| Aspect | JSONB `data` | Normalized columns |
|--------|--------------|--------------------|
| GraphQL fit | One read returns the type | Joins at read time |
| Storage | Repeats embedded values | Stores each value once |
| Indexing | Expression and GIN indexes | B-tree on columns |
| Schema changes | Change the definition | Migrate tables |

A TVIEW can carry both: project the filter columns, and keep the document in `data`.

## Best practices

1. Model one TVIEW per GraphQL type, embed it where it is used.
2. Project the columns the API filters on; index those, not every field.
3. Keep embedding chains short and fan-out bounded; check `pg_tviews_profile()`.
4. Make TVIEWs read from replicas LOGGED.
5. Grant maintenance functions to the roles that run them; keep TVIEW ownership with the
   schema owner.

## See also

- [Developer Guide](developers.md)
- [Aggregate TVIEWs](aggregate-tviews.md)
- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
- [Performance Benchmarks](../benchmarks/overview.md)
- [Performance Tuning](../operations/performance-tuning.md)
- [Troubleshooting Guide](../operations/troubleshooting.md)
