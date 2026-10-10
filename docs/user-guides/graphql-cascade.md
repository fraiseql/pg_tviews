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
    reset        boolean DEFAULT true  -- the next call reports only later changes
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
- **Order** is by entity, then primary key, so truncation is deterministic. With more
  than `max_entities` entries, the rest are left out, `truncated` is true and
  `invalidated_types` lists their types, so a client can invalidate those types instead.
- **Savepoints**: changes made in a savepoint that was rolled back are not reported.
- **`__typename`** is the entity in PascalCase (`blog_post` → `BlogPost`). Override it
  with `tviews.pg_tviews_set_typename()`; NULL resets it.
- **Memory**: at most `pg_tviews.report_max_tracked` changed rows (default 10 000) are
  kept per transaction. Beyond it only their types are, and the report is truncated.
  `SET pg_tviews.report_max_tracked = 0` turns the journal off.

```sql
SELECT tviews.pg_tviews_set_typename('user', 'Author');

BEGIN;
UPDATE tb_user SET name = 'Alicia' WHERE pk_user = 1;  -- refreshes tv_user and tv_post
SELECT jsonb_pretty(tviews.pg_tviews_flush_and_report(include_data => false));
COMMIT;
```

The report describes what this transaction wrote. Other transactions committing in
between can change the same rows before yours commits.
