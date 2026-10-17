# Reporting what a mutation changed (GraphQL Cascade)

A mutation changes base tables; pg_tviews refreshes the TVIEWs that depend on them,
including rows reached through cascades. `tviews.pg_tviews_flush_and_report()` returns those
read-model rows, so a mutation can answer with every entity it changed, in the
[GraphQL Cascade](https://github.com/graphql-cascade/graphql-cascade) shape, instead of
building the list by hand.

## Usage

The examples on this page run in order in one database:

```sql
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;

CREATE TABLE tb_user (
    pk_user BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name TEXT NOT NULL
);
CREATE TABLE tb_post (
    pk_post BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id UUID NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    title TEXT NOT NULL,
    fk_user BIGINT NOT NULL REFERENCES tb_user (pk_user)
);
INSERT INTO tb_user (name) VALUES ('Alice');
INSERT INTO tb_post (title, fk_user) VALUES ('Hello', 1);

CREATE TABLE tv_user AS
SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
FROM tb_user u;

CREATE TABLE tv_post AS
SELECT p.pk_post, p.id,
       jsonb_build_object('id', p.id, 'title', p.title, 'author', u.data) AS data
FROM tb_post p JOIN tv_user u ON u.pk_user = p.fk_user;
```

Call it last in the mutation, before building the response:

```sql
CREATE FUNCTION update_post(p BIGINT, new_title TEXT) RETURNS JSONB
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET title = new_title WHERE pk_post = p;
    RETURN jsonb_build_object(
        'post',    (SELECT data FROM tv_post WHERE pk_post = p),
        'cascade', tviews.pg_tviews_flush_and_report());
END $$;

SELECT jsonb_pretty(update_post(1, 'Hello, world'));
```

The `cascade` part has this shape:

```json
{
  "updated": [
    {"__typename": "Post", "id": "…", "operation": "UPDATED", "data": {"title": "…"}},
    {"__typename": "Feed", "id": "…", "operation": "UPDATED", "data": {…}}
  ],
  "deleted": [{"__typename": "Comment", "id": "…"}],
  "truncated": false,
  "invalidated_types": []
}
```

```text
tviews.pg_tviews_flush_and_report(
    max_entities integer DEFAULT 500,  -- entries reported before truncating
    include_data boolean DEFAULT true, -- include each row's fresh data
    reset        boolean DEFAULT true, -- the next call reports only later changes
    types        text[]  DEFAULT NULL  -- report only these types (NULL: all)
) RETURNS jsonb
```

## Semantics

- **Rows that really changed.** Every refresh write records the keys it inserted,
  updated or deleted. A refresh that recomputed a row and found it unchanged records
  nothing, so a no-op mutation reports nothing.
- **The whole transaction so far.** Refreshes run after every statement, so the report
  covers every statement since the transaction began (or since the last call with
  `reset`), whether or not it ran inside the mutation function.
- **`operation`** is `CREATED` for a row first inserted in this span, `UPDATED`
  otherwise. A row inserted and then deleted in the same span is not reported; a row
  deleted and then re-inserted is `UPDATED`.
- **Rebuilds**: a TVIEW rebuilt whole in the transaction (`pg_tviews_refresh()`, the
  catch-up at the end of a suspension) lists its type in `invalidated_types`: its rows
  are not reported one by one.
- **Order** is by entity, then primary key, so truncation is deterministic. With more
  than `max_entities` entries, the rest are left out, `truncated` is true and
  `invalidated_types` lists their types, so a client can invalidate those types instead.
- **Savepoints**: changes made in a savepoint that was rolled back are not reported,
  and a `reset` made in one is undone with it: after `ROLLBACK TO SAVEPOINT`, or a
  plpgsql `EXCEPTION` handler, the next call reports again what that call had.
- **`__typename`** is the entity in PascalCase (`blog_post` → `BlogPost`). Override it
  with the `typename` option, declared with the TVIEW (`pg_tviews_create(…, options)`
  or `pg_tviews_create_or_replace(…, options)`). The options passed are the whole
  declaration: a replace that leaves `typename` out puts the default back.
- **Only the types asked for**: `types` names the GraphQL types the response selects
  (`__typename` values). Rows of other types are left out before `max_entities` applies,
  and their types are listed in `omitted_types` (present only when `types` is given),
  apart from `invalidated_types`. They were still refreshed, and `reset` clears them too.
  `'{}'` reports nothing and omits every changed type. A name no TVIEW has fails with
  `22023`, before anything is reported or reset. `types` filters the report only, never
  the [change feed](change-feed.md): a cache elsewhere needs every change.
- **Collections**: when the rows were written with `pg_tviews.publish_changes = 'rows'`,
  the report also carries `collections`, the parents whose composed child lists the
  reported rows changed, as
  the [change feed](change-feed.md#collection-keys) names them in `c`:
  `"collections": {"Comment.fk_post": [<post id>, …]}`. A comment inserted into post P,
  moved from P to Q, edited or deleted names P (and Q), so a layer that ran the write
  evicts exactly the cached results composing those lists. Every parent each row named
  since the last reset is listed once, before and after each write, ids in text order;
  `types` filters by the child type; `reset` and savepoints apply as to rows. A
  collection whose parents cannot be listed (a parent TVIEW without an `id` column, past
  the feed's byte budget) puts its child type in `invalidated_types` instead, and a type
  there (rows left out past `max_entities` too) stands for all its collections, which
  are then not listed. Rows written without `rows` captured nothing: the key is absent,
  whatever the setting when the report runs. Absent means "not known", `{}` means
  "none changed".
- **Memory**: at most `pg_tviews.report_max_tracked` changed rows (default 10 000) are
  kept per transaction, not counting reported ones (kept only until the
  subtransaction that reported them ends). Beyond it only their types are, and the report is truncated.
  `SET pg_tviews.report_max_tracked = 0` turns the journal off.

```sql
SELECT tviews.pg_tviews_create_or_replace('tv_user', $$
    SELECT u.pk_user, u.id, jsonb_build_object('id', u.id, 'name', u.name) AS data
    FROM tb_user u
$$, options => '{"typename": "Author"}');

BEGIN;
UPDATE tb_user SET name = 'Alicia' WHERE pk_user = 1;  -- refreshes tv_user and tv_post
SELECT jsonb_pretty(tviews.pg_tviews_flush_and_report(include_data => false));
COMMIT;

BEGIN;
UPDATE tb_user SET name = 'Alice' WHERE pk_user = 1;
-- The mutation selects authors only: Post rows are left out, "omitted_types": ["Post"].
SELECT jsonb_pretty(tviews.pg_tviews_flush_and_report(types => '{Author}'));
COMMIT;
```

The report describes what this transaction wrote. Other transactions committing in
between can change the same rows before yours commits.

The report is for the mutation's own response, inside its transaction. Caches in other
processes learn what committed transactions changed from the
[change feed](change-feed.md) (`pg_tviews.publish_changes`).
