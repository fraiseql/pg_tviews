# Arrays in TVIEWs

A TVIEW can hold arrays in two ways: an array column (`uuid[]`, `text[]`, …) and a JSON
array inside `data` (`jsonb_agg(…)`). It can also join on an array of keys. In every
case pg_tviews reads the definition from PostgreSQL's query tree, like any other
definition: nothing depends on how the array is spelled or what its columns are called.

## Array columns and JSON arrays

```sql
CREATE TABLE tb_article (
    pk_article bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    title text NOT NULL
);
CREATE TABLE tb_comment (
    pk_comment bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    fk_article bigint NOT NULL REFERENCES tb_article,
    body text NOT NULL
);
INSERT INTO tb_article (title) VALUES ('First');
INSERT INTO tb_comment (fk_article, body) VALUES (1, 'Hello');

CREATE TABLE tv_article AS
SELECT
    a.pk_article,
    a.id,
    ARRAY(SELECT c.id FROM tb_comment c
          WHERE c.fk_article = a.pk_article ORDER BY c.pk_comment) AS comment_ids,
    jsonb_build_object(
        'id', a.id,
        'title', a.title,
        'comments', COALESCE(
            (SELECT jsonb_agg(jsonb_build_object('id', c.id, 'body', c.body)
                              ORDER BY c.pk_comment)
             FROM tb_comment c WHERE c.fk_article = a.pk_article),
            '[]'::jsonb)
    ) AS data
FROM tb_article a;
```

- `comment_ids` takes the type the backing view gives it, `uuid[]`, as every column
  without a fixed type does ([Columns](reference/ddl.md#columns)).
- The correlated subqueries link `tb_comment` to the key through
  `c.fk_article = a.pk_article`, so `tb_comment` is `local` in
  `tviews.registry.cascade_kinds`: inserting, updating or deleting a comment refreshes
  its article's row, the array column and the JSON array together.

```sql
INSERT INTO tb_comment (fk_article, body) VALUES (1, 'Second');
DELETE FROM tb_comment WHERE body = 'Hello';
SELECT comment_ids, data->'comments' FROM tv_article;   -- one comment, 'Second'
```

The row is recomputed from the backing view as a whole; there is no element-by-element
patch of an array. Writing the same subquery with a `LEFT JOIN … GROUP BY` works the
same way.

Embedding the rows of another TVIEW (`jsonb_agg(c.data …) FROM tv_comment c`) reads its
table: a refresh of the child TVIEW then refreshes the parents it maps to, in the same
flush ([FraiseQL Integration](getting-started/fraiseql-integration.md#embedding-a-list)).

## Joining on an array of keys

A row may hold the keys of the rows it refers to, such as a hierarchy stored as the path
of its ancestors:

```sql
CREATE TABLE tb_node (
    pk_node bigint PRIMARY KEY,
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    name text NOT NULL,
    ancestors bigint[] NOT NULL DEFAULT '{}'
);
INSERT INTO tb_node VALUES (1, DEFAULT, 'root', '{}'),
                           (2, DEFAULT, 'child', '{1}'),
                           (3, DEFAULT, 'leaf', '{1,2}');

CREATE TABLE tv_node AS
SELECT n.pk_node, n.id,
       jsonb_build_object(
           'name', n.name,
           'path', (SELECT jsonb_agg(a.name ORDER BY a.pk_node)
                    FROM tb_node a WHERE a.pk_node = ANY (n.ancestors))) AS data
FROM tb_node n;

UPDATE tb_node SET name = 'ROOT' WHERE pk_node = 1;   -- refreshes nodes 1, 2 and 3
```

`a.pk_node = ANY (<array>)`, a join on `unnest(<array>)` in a subquery's or CTE's
select list, and `LATERAL unnest(<array>)` are the same condition: the array holds the
key. `tb_node` is then `mapped` for this read: a write maps to TVIEW keys through
`<array> @> ARRAY[<key>]`. A cast of the element (`unnest(string_to_array(n.path,
'.'))::bigint`) is an element of the cast array, `(string_to_array(n.path,
'.'))::bigint[]`.

That lookup is served by a GIN index on the array expression. When it would scan a
large table sequentially, the create-time NOTICE names the index to create:

```
NOTICE:  writes to public.tb_node map to tv_node keys with a sequential scan of tb_node
         (about 3000 rows); CREATE INDEX ON public.tb_node USING gin
         (((pg_catalog.string_to_array(path, '.'::pg_catalog.text))::bigint[])) would
         make them cheaper
```

`tviews.pg_tviews_mapping_query('tv_node', 'tb_node'::regclass)` returns the mapping query, to
check its plan with `EXPLAIN`.

## Tests

`test/sql/regress/arrays/` covers array dependencies, the three array-join spellings
and casts of `unnest` elements; the integration files `test/sql/50_array_columns.sql`,
`51_jsonb_array_update.sql`, `52_array_insert_delete.sql` and
`53_batch_optimization.sql` cover array columns and JSON arrays under inserts, updates
and deletes.
