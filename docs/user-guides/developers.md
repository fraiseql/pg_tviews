# Developer Guide

How to build a GraphQL read model on pg_tviews: design the tables, define TVIEWs that
match the GraphQL types, read them from resolvers, and handle the errors pg_tviews raises.

## Overview

pg_tviews maintains `tv_*` tables (the read models) from the tables an application writes
(the write models). A write refreshes the TVIEW rows it affects at the end of the statement,
in the same transaction:

```text
GraphQL mutation (command)        GraphQL query (read)
        |                                  |
   write tables  --- pg_tviews --->   tv_* tables
  (tb_user, ...)    (triggers)      (tv_post, ...)
```

The workflow:

1. Design the write tables.
2. Create one TVIEW per GraphQL type that needs a read model.
3. Read `tv_*` tables from resolvers; write only to the write tables.
4. Watch refresh cost with `tviews.pg_tviews_queue_stats()` and `tviews.pg_tviews_profile()`.

## Setup

The examples on this page run in order in one database. pg_tviews must be in
`shared_preload_libraries` ([Installation](../getting-started/installation.md)).

```sql
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
```

The functions live in the `tviews` schema. This page qualifies them (`tviews.pg_tviews_*`);
`SET search_path TO public, tviews` lets you drop the prefix.

## Schema design

### Write tables

FraiseQL's trinity pattern gives each entity an integer key (`pk_*`), a public UUID (`id`)
and an optional slug (`identifier`), with integer foreign keys (`fk_*`):

```sql
CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT NOT NULL REFERENCES tb_user (pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tb_comment (
    pk_comment BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    text TEXT NOT NULL,
    fk_post BIGINT NOT NULL REFERENCES tb_post (pk_post),
    fk_user BIGINT NOT NULL REFERENCES tb_user (pk_user),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO tb_user (identifier, name, email) VALUES
    ('alice', 'Alice', 'alice@example.com'),
    ('bob', 'Bob', 'bob@example.com');
INSERT INTO tb_post (identifier, title, content, fk_user) VALUES
    ('hello-world', 'Hello World', 'First post', 1);
INSERT INTO tb_comment (text, fk_post, fk_user) VALUES ('Nice post', 1, 2);
```

These names are a convention, not a requirement. pg_tviews reads the TVIEW's definition
from PostgreSQL's query tree: a write table may have any name, and a foreign key column
may be called `author_pk` or `user_ref`. What it follows is the joins and filters in the
definition.

### TVIEW design

A TVIEW named `tv_<entity>` needs one column naming its rows, `pk_<entity>`, and usually a
`data` JSONB document shaped like the GraphQL type. Add plain columns for the filters
resolvers use (`id`, `user_id`):

```sql
CREATE TABLE tv_post AS
SELECT
    p.pk_post,             -- row identity of tv_post
    p.id,                  -- GraphQL ID
    p.identifier,          -- slug
    u.id AS user_id,       -- filter column
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'createdAt', p.created_at,
        'author', jsonb_build_object('id', u.id, 'name', u.name),
        'comments', COALESCE((
            SELECT jsonb_agg(
                       jsonb_build_object(
                           'id', c.id,
                           'text', c.text,
                           'author', jsonb_build_object('id', cu.id, 'name', cu.name))
                       ORDER BY c.created_at, c.pk_comment)
            FROM tb_comment c
            JOIN tb_user cu ON cu.pk_user = c.fk_user
            WHERE c.fk_post = p.pk_post
        ), '[]'::jsonb)
    ) AS data
FROM tb_post p
JOIN tb_user u ON u.pk_user = p.fk_user;
```

Every table the definition reads gets a trigger: a write to `tb_post`, `tb_user` or
`tb_comment` refreshes the posts it reaches. Renaming Bob refreshes the post he commented
on:

```sql
UPDATE tb_user SET name = 'Robert' WHERE identifier = 'bob';

SELECT data->'comments'->0->'author'->>'name' AS commenter FROM tv_post;
```

A definition pg_tviews cannot keep up to date (a read it cannot trace back to rows of
the TVIEW, a call to the current time) is refused when it is created. See
[`uncascaded_policy`](../reference/api.md) for the alternatives.

## GraphQL integration

### Resolvers

Resolvers read `data`; mutations write the write tables and read the TVIEW back in the same
transaction, where it is already fresh:

```javascript
const resolvers = {
  Query: {
    post: async (_, { id }) => {
      const r = await db.query('SELECT data FROM tv_post WHERE id = $1', [id]);
      return r.rows[0]?.data;
    },
    posts: async (_, { authorId, limit = 10 }) => {
      const r = await db.query(
        `SELECT data FROM tv_post WHERE user_id = $1
         ORDER BY data->>'createdAt' DESC LIMIT $2`,
        [authorId, limit]);
      return r.rows.map((row) => row.data);
    },
  },
  Mutation: {
    updatePost: async (_, { id, input }) => {
      await db.query('UPDATE tb_post SET title = $1 WHERE id = $2', [input.title, id]);
      // The UPDATE refreshed tv_post before it returned.
      const r = await db.query('SELECT data FROM tv_post WHERE id = $1', [id]);
      return r.rows[0]?.data;
    },
  },
};
```

To return every entity a mutation changed (GraphQL Cascade), call
`tviews.pg_tviews_flush_and_report()`: see [GraphQL Cascade](graphql-cascade.md).

### Filtering and pagination

Filter on the plain columns, page on a value from `data`:

```sql
SELECT data FROM tv_post
WHERE user_id = (SELECT id FROM tb_user WHERE identifier = 'alice');

SELECT data FROM tv_post WHERE identifier = 'hello-world';

-- Keyset pagination: the cursor is the last createdAt seen
SELECT data FROM tv_post
WHERE user_id = (SELECT id FROM tb_user WHERE identifier = 'alice')
  AND data->>'createdAt' < '9999-12-31'
ORDER BY data->>'createdAt' DESC
LIMIT 10;
```

Refreshes render values under fixed settings (`TimeZone` UTC, `DateStyle` ISO), so a
timestamp in `data` has one text form whatever the writer's session, and sorts as text.

## Performance

### Indexes

A TVIEW is a table: index it for the resolvers' filters.

```sql
CREATE INDEX idx_tv_post_user_created ON tv_post (user_id, (data->>'createdAt'));
CREATE INDEX idx_tv_post_identifier ON tv_post (identifier);
```

Add a GIN index on `data` only for containment queries (`data @> '{...}'`): nearly every
refresh rewrites `data`, so an index on it makes every refresh a non-HOT update.
`SET pg_tviews.data_gin_index = on` before creating a TVIEW creates one.
`tviews.pg_tviews_ensure_propagation_indexes('post')` adds the indexes pg_tviews' own
lookups need (not executable by `PUBLIC`; it also requires owning the TVIEW).

A filter on a nested value with no index (`data->'author'->>'name' ILIKE '%ali%'`) scans the
table: project such a value as a column, or index the expression.

### Bulk writes

Each statement refreshes the rows it affected once, at its end: an `UPDATE` of 10,000
rows refreshes each affected TVIEW row once, not 10,000 times. For a large load, see
`tviews.pg_tviews_suspend_triggers()` in the [API reference](../reference/api.md).

### Refresh cost

```sql
BEGIN;
UPDATE tb_user SET name = 'Alice Liddell' WHERE identifier = 'alice';
SELECT tviews.pg_tviews_queue_stats();  -- this transaction's refreshes
COMMIT;

SELECT entity, rows_estimate, warnings FROM tviews.pg_tviews_profile('post');
```

`pg_tviews_queue_stats()` reports the current transaction's queued refreshes
(`total_refreshes`, `total_timing_ms`) and, cumulated over the session, how many rows were
patched in place (`direct_patches_applied`) or recomputed from the definition
(`view_recomputes`).
`pg_tviews_profile()` reports sizes, dead tuples and the fan-out of each lookup column,
with warnings. TVIEWs are created UNLOGGED by default (`pg_tviews.unlogged_by_default`):
fast to write, but empty after a crash and unreadable on a standby
([Architect Guide](architects.md#read-replicas)).

## Errors and transactions

A write and the refresh it causes are one transaction: a rollback undoes both.

```sql
BEGIN;
UPDATE tb_post SET title = 'Draft title' WHERE pk_post = 1;
SELECT data->>'title' AS title FROM tv_post WHERE pk_post = 1;  -- Draft title
ROLLBACK;

SELECT data->>'title' AS title FROM tv_post WHERE pk_post = 1;  -- Hello World
```

pg_tviews errors carry a SQLSTATE, so application code and PL/pgSQL can tell them apart:

| SQLSTATE | Condition | Meaning |
|----------|-----------|---------|
| 42704 | `undefined_object` | No such TVIEW |
| 42P07 | `duplicate_table` | The TVIEW already exists |
| 42601 | `syntax_error` | The definition cannot be read |
| 0A000 | `feature_not_supported` | A definition pg_tviews cannot maintain |
| 42501 | `insufficient_privilege` | Not the TVIEW's owner, or a maintenance function not granted |
| 42P17 | `invalid_object_definition` | TVIEWs that would read each other in a cycle |
| 55000 | `object_not_in_prerequisite_state` | A commit with refresh work still queued, or a refresh while suspended |

```sql
DO $$
BEGIN
    PERFORM tviews.pg_tviews_refresh('no_such_entity');
EXCEPTION WHEN undefined_object THEN
    RAISE NOTICE 'not a TVIEW: %', SQLERRM;
END $$;
```

Messages are one line; the definition or query is in the DETAIL and the fix in the HINT.
The [error reference](../error-reference.md) lists them all.

## Migrating from materialized views

A materialized view needs a `REFRESH MATERIALIZED VIEW` after writes, which recomputes it
in full outside the writing transaction. A TVIEW over the same query refreshes the affected
rows in the writing transaction:

```text
-- Before
CREATE MATERIALIZED VIEW mv_post AS SELECT ...;
-- after every write:
REFRESH MATERIALIZED VIEW mv_post;

-- After
CREATE TABLE tv_post AS SELECT p.pk_post, ..., jsonb_build_object(...) AS data FROM ...;
-- writes refresh it; nothing to call
```

## Testing

Test a TVIEW in a transaction that rolls back:

```sql
BEGIN;
INSERT INTO tb_post (identifier, title, fk_user) VALUES ('test-post', 'Test Post', 1);
SELECT count(*) = 1 AS created FROM tv_post WHERE identifier = 'test-post';

DELETE FROM tb_post WHERE identifier = 'test-post';
SELECT count(*) = 0 AS deleted FROM tv_post WHERE identifier = 'test-post';
ROLLBACK;
```

`tviews.pg_tviews_refresh('post')` recomputes a TVIEW from its definition. Comparing the
table before and after is a way to check that writes kept it exact.

## Troubleshooting

### A TVIEW is not updating

```sql
-- Errors and warnings only (an empty result is healthy)
SELECT * FROM tviews.pg_tviews_health_check() WHERE status <> 'OK';

-- The TVIEW's registration: the tables it reads and how each refreshes it
SELECT name, base_tables, cascade_kinds, uncascaded_tables, needs_reregister
FROM tviews.registry WHERE entity = 'post';

-- The triggers on the tables it reads
SELECT tgrelid::regclass AS on_table, tgname
FROM pg_trigger WHERE tgname LIKE 'trg_tview_%' ORDER BY 1, 2;
```

A TVIEW with `needs_reregister` set, or one whose stored plan no longer matches the tables,
is fixed with `tviews.pg_tviews_reregister('post')`. Changes made while triggers were
suspended are repaired with `tviews.pg_tviews_refresh('post')`.

### Slow reads or writes

```sql
EXPLAIN SELECT data FROM tv_post WHERE identifier = 'hello-world';

SELECT tviews.pg_tviews_debug_queue();  -- keys queued in this transaction
SELECT * FROM tviews.pg_tviews_performance_stats();
```

A write that is slow usually reaches many TVIEW rows (a user embedded in every post):
`pg_tviews_profile()` names the lookup columns with a large fan-out.

## Best practices

- Shape each TVIEW's `data` like its GraphQL type, and project the filter columns the
  resolvers use.
- Index for the queries you run, not for every field.
- Write only to the write tables; do not refresh TVIEWs by hand after writes.
- Keep chains of TVIEWs embedding TVIEWs short: each level adds a refresh on writes.

## See also

- [FraiseQL Integration Guide](../getting-started/fraiseql-integration.md)
- [API Reference](../reference/api.md)
- [GraphQL Cascade](graphql-cascade.md)
- [Aggregate TVIEWs](aggregate-tviews.md)
- [Performance Tuning](../operations/performance-tuning.md)
- [Troubleshooting Guide](../operations/troubleshooting.md)
