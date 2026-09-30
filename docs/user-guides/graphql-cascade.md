# Reporting what a mutation changed (GraphQL Cascade)

A mutation changes base tables; pg_tviews refreshes the TVIEWs that depend on them,
including rows reached through cascades. `pg_tviews_flush_and_report()` returns those
read-model rows, so a mutation can answer with every entity it changed, in the
[GraphQL Cascade](https://github.com/graphql-cascade/graphql-cascade) shape, instead of
building the list by hand.

## Usage

Call it last in the mutation, before building the response:

```sql
CREATE FUNCTION app.update_post(p BIGINT, new_title TEXT) RETURNS JSONB
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_post SET title = new_title WHERE pk_post = p;
    RETURN jsonb_build_object(
        'post',    (SELECT data FROM tv_post WHERE pk_post = p),
        'cascade', pg_tviews_flush_and_report());
END $$;
```

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

```sql
pg_tviews_flush_and_report(
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
- **`__typename`** is the entity in PascalCase (`blog_post` → `BlogPost`). Override it:
  `SELECT pg_tviews_set_typename('blog_post', 'Article');` (NULL resets it).
- **Memory**: at most `pg_tviews.report_max_tracked` changed rows (default 10 000) are
  kept per transaction. Beyond it only their types are, and the report is truncated.
  `SET pg_tviews.report_max_tracked = 0` turns the journal off.

The report describes what this transaction wrote. Other transactions committing in
between can change the same rows before yours commits.
