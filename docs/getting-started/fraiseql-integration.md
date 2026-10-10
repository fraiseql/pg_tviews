# FraiseQL Integration Guide

How pg_tviews fits FraiseQL's CQRS layout, and which FraiseQL conventions it uses.

## FraiseQL CQRS Overview

FraiseQL separates write models from read models:

```
tb_* tables  →  v_* views  →  tv_* tables  →  GraphQL
(normalized)    (declarative)  (materialized)
```

- **`tb_*` tables**: normalized write models, written by mutations
- **`v_*` views**: declarative read model definitions
- **`tv_*` tables**: the read models materialized by pg_tviews, kept up to date inside
  each writing transaction
- **GraphQL**: queries read `tv_*.data`

pg_tviews maintains `tv_*` from its definition, which may be the `v_*` view itself
(`CREATE TABLE tv_post AS SELECT * FROM v_post`).

## FraiseQL's identifiers, and what pg_tviews requires

FraiseQL names its columns by role (the "trinity" identifiers):

| Column | Type | Role in FraiseQL |
|---|---|---|
| `pk_<entity>` | `bigint` | internal primary key, used in joins |
| `id` | `uuid` | public GraphQL identifier |
| `identifier` | `text` | human-readable unique slug (optional) |
| `fk_<parent>` | `bigint` | foreign key to the parent's `pk_<parent>` |
| `<parent>_id` | `uuid` | the parent's public id, for filtering (optional) |

These names fit pg_tviews, but only one is required: the definition outputs a
`pk_<entity>` column named after the TVIEW. pg_tviews reads the definition from
PostgreSQL's query tree, so it follows joins whatever the tables and columns are
called. In a TVIEW's table, some names fix the column type: `pk_<entity>` and `fk_*`
are `bigint`, `id` is `uuid`, `data` is `jsonb`; `id` and `*_id` columns are indexed
([DDL Reference](../reference/ddl.md#columns)).

## Example

```sql
CREATE EXTENSION IF NOT EXISTS pg_tviews;
SET search_path = "$user", public, tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    name TEXT NOT NULL
);

CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    identifier TEXT UNIQUE,
    title TEXT NOT NULL,
    content TEXT,
    fk_user BIGINT NOT NULL REFERENCES tb_user (pk_user)
);

CREATE TABLE tb_comment (
    pk_comment BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    body TEXT NOT NULL,
    fk_post BIGINT NOT NULL REFERENCES tb_post (pk_post)
);

INSERT INTO tb_user (identifier, name) VALUES ('alice', 'Alice');
INSERT INTO tb_post (identifier, title, content, fk_user)
VALUES ('hello', 'Hello', 'First post', 1);
```

### A TVIEW per entity

```sql
CREATE TABLE tv_user AS
SELECT u.pk_user, u.id, u.identifier,
       jsonb_build_object('id', u.id, 'identifier', u.identifier, 'name', u.name) AS data
FROM tb_user u;

CREATE TABLE tv_post AS
SELECT
    p.pk_post,
    p.id,
    p.identifier,
    p.fk_user,
    u.id AS user_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'content', p.content,
        'author', u.data
    ) AS data
FROM tb_post p
JOIN tv_user u ON u.pk_user = p.fk_user;
```

`tv_post` embeds `tv_user`'s document by joining its table on its key. A rename
refreshes `tv_user`, and that refresh refreshes the posts embedding the user, in the
same flush.

### Embedding a list

```sql
CREATE TABLE tv_comment AS
SELECT c.pk_comment, c.id, c.fk_post,
       jsonb_build_object('id', c.id, 'body', c.body) AS data
FROM tb_comment c;

SELECT pg_tviews_create_or_replace('tv_post', $$
SELECT
    p.pk_post, p.id, p.identifier, p.fk_user, u.id AS user_id,
    jsonb_build_object(
        'id', p.id,
        'identifier', p.identifier,
        'title', p.title,
        'author', u.data,
        'comments', COALESCE(
            (SELECT jsonb_agg(c.data ORDER BY c.pk_comment)
             FROM tv_comment c WHERE c.fk_post = p.pk_post),
            '[]'::jsonb)
    ) AS data
FROM tb_post p
JOIN tv_user u ON u.pk_user = p.fk_user
$$);
```

`pg_tviews_create_or_replace()` changed the definition in place (`replaced`: same
columns). A comment written, changed or deleted refreshes its post.

## Writes and reads

A mutation writes the `tb_*` tables; the TVIEW rows it affects are refreshed inside the
same transaction, at the end of each statement, so the mutation can return them:

```sql
BEGIN;
INSERT INTO tb_comment (body, fk_post) VALUES ('Nice post', 1);
UPDATE tb_user SET name = 'Alice Martin' WHERE identifier = 'alice';
SELECT data FROM tv_post WHERE identifier = 'hello';   -- both changes, already
COMMIT;
```

Queries read `data`, filtered on the indexed columns:

```sql
SELECT data FROM tv_post WHERE id = (SELECT id FROM tb_post WHERE identifier = 'hello');
SELECT data FROM tv_post WHERE user_id = (SELECT id FROM tb_user WHERE identifier = 'alice');
```

## What a write refreshes

```sql
SELECT name, cascade_kinds FROM tviews.registry ORDER BY name;
```

| TVIEW | Table | Kind | Meaning |
|---|---|---|---|
| `tv_user` | `tb_user` | `local` | each changed row's key is read off the row |
| `tv_post` | `tb_post` | `local` | |
| `tv_post` | `tv_user` | `propagated` | a refreshed user refreshes the posts embedding it |
| `tv_post` | `tv_comment` | `mapped` | a refreshed comment refreshes its post, through `fk_post`, in the same flush |
| `tv_comment` | `tb_comment` | `local` | |

A definition that reads a table no condition links to the key (an uncorrelated
subquery, a window function without a `PARTITION BY` linked to the key, a recursive CTE,
a materialized view) is refused unless it declares what a write to that table does
(`uncascaded_policy`, `uncascaded_tables`): see
[Tables no cascade reaches](../reference/ddl.md#tables-no-cascade-reaches).

## Performance notes

- Every TVIEW gets a statement-level trigger: a bulk statement refreshes the rows it
  affected once, at its end.
- With the `jsonb_delta` extension installed, eligible single-row updates patch the
  stored documents instead of recomputing them
  ([Installation](installation.md#jsonb_delta)).
- `pg_tviews_profile()` and `pg_tviews_ensure_propagation_indexes()` find the
  indexes a TVIEW's lookups need ([Profile](../reference/profile.md)).

## Monitoring

```sql
SELECT * FROM pg_tviews_health_check();
SELECT * FROM pg_tviews_performance_stats();
SELECT pg_tviews_queue_stats();   -- this session's current transaction
```

## Troubleshooting

**A TVIEW is not updating:**

```sql
-- the triggers on the base tables
SELECT tgrelid::regclass, tgname FROM pg_trigger WHERE tgname LIKE 'trg\_tview%';

-- missing triggers, unreadable plans
SELECT * FROM pg_tviews_health_check() WHERE status <> 'OK';
```

`SELECT * FROM tviews.pg_tviews_reregister_all()` re-installs missing triggers (it is
not executable by `PUBLIC`: [Operator role](../user-guides/operators.md#operator-role)),
and `SELECT pg_tviews_refresh('post')` recomputes a TVIEW in full.

**What depends on what:**

```sql
SELECT * FROM pg_tviews_show_cascade_path('user');
```

## Next Steps

- **[Developer Guide](../user-guides/developers.md)** - Application integration patterns
- **[Architect Guide](../user-guides/architects.md)** - CQRS design decisions
- **[DDL Reference](../reference/ddl.md)** - What a definition may contain
- **[API Reference](../reference/api.md)** - Complete function reference

## Related Resources

- **FraiseQL Framework**: [github.com/fraiseql/fraiseql](https://github.com/fraiseql/fraiseql)
