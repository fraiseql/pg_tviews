# Embed or compose

A GraphQL type often holds a list: a post's comments, an order's lines, a user's tags.
A TVIEW can **embed** the list in the parent's `data` (`jsonb_agg` in its definition), or
the reader can **compose** it at read time from the child's own TVIEW. The rule:

> **The child has a `tv_`? Reference it. Otherwise embed it.**

The numbers behind the rule are in [Read path: embed or compose](../benchmarks/read-path.md).

## When to embed

Embed a list when **all** of these hold. These are value arrays owned by the parent: tags,
address lines, an order's frozen line items, translations, a status history written by the
parent's own mutations.

- The elements are not an entity with a TVIEW or queries of their own.
- The list is bounded and small: the parent document stays under the ~2 kB TOAST
  threshold. Past it, every read of the parent pays the detoasting, reads that skip the
  list included (42 against 23 µs for a page of 20 posts with 50 comments each).
- The elements are written with the parent, by the same mutation, and none of their fields
  is copied from another entity. A comment's `author.username` is copied from the user:
  renaming a user rewrites every array that holds one of their comments.
- The list takes no arguments (pagination, filter, order) and has no row security of its
  own.
- Reads are heavy and read the whole list: embedded, a page of 20 posts with 5 comments
  each costs 62 µs, against 108 µs composed with the cheapest SQL.

## When to compose

Compose when **any** of these hold:

- the child is an entity with its own `tv_` (comments, a customer's orders, events);
- the list is unbounded, or grows;
- the elements embed other entities, so writes to those fan out into every array;
- the list is paginated or filtered;
- the child has row-level security;
- the parent is hot: concurrent child inserts would serialize on the parent row.

Composition costs reads (5 to 46 µs more per read here) and saves writes: an author
rename costs 0.567 ms with the comments embedded and 0.337 ms composed, and a comment moved
to another post 0.362 against 0.121 ms. When `tv_comment` exists anyway (the application
has a `Comment` type), composing removes the array's whole maintenance cost. Behind a cache
that evicts composed results precisely ([collection keys](change-feed.md#collection-keys)),
reads pay the composition only on a miss.

Keep embedding **scalars of the children**, such as `commentCount` or `lastCommentAt`:
they are cheap to maintain and are not lists.

## How pg_tviews supports composition

A reader composes `Post.comments` by reading `tv_comment` rows whose `fk_post` holds the
post's `pk_post`. pg_tviews gives it what it needs, from the definitions alone:

- **The relationship.** A `FOREIGN KEY` on the child's own table proves it: an output
  column traced to `tb_comment.fk_post REFERENCES tb_post` is a reference to `tv_post`.
  `tviews.registry.references` lists each one, with the parent, whether it is one-to-one
  and nullable, its index and its order ([read contract](../reference/read-contract.md)).
  Names are never matched: `fk_author REFERENCES tb_user` is a reference to `tv_user`,
  and a column without a constraint is no reference.
- **The index.** Every reference column gets a managed `(column, identity)` index: each
  composed list is one index range scan, already in identity order.
- **Freshness.** A write refreshes the parent and the child TVIEWs in its own
  transaction, and one statement reads both under one snapshot: the composed list is the
  rows the embedded array would hold.
- **Eviction.** The change feed names the collections a changed row left or joined
  (`"c": {"Comment.fk_post": [<post id>]}`): a cache evicts exactly the composed results
  a comment insert, move, edit or delete changed.

## Example

```sql
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;

CREATE TABLE tb_user (pk_user bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                      username text NOT NULL);
CREATE TABLE tb_post (pk_post bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                      id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                      title text NOT NULL,
                      tags text[] NOT NULL DEFAULT '{}');
CREATE TABLE tb_comment (pk_comment bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                         id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
                         fk_post bigint NOT NULL REFERENCES tb_post,
                         fk_author bigint NOT NULL REFERENCES tb_user,
                         content text NOT NULL);
INSERT INTO tb_user (username) VALUES ('ann'), ('bob');
INSERT INTO tb_post (title, tags) VALUES ('Hello', '{intro,news}');
INSERT INTO tb_comment (fk_post, fk_author, content) VALUES (1, 2, 'Nice'), (1, 1, 'Thanks');

-- The comments are an entity: their own TVIEW, the author copied in.
CREATE TABLE tv_comment AS
SELECT c.pk_comment, c.id, c.fk_post, c.fk_author,
       jsonb_build_object('id', c.id, 'content', c.content,
                          'author', jsonb_build_object('username', u.username)) AS data
FROM tb_comment c JOIN tb_user u ON u.pk_user = c.fk_author;

-- The tags are values the post owns: embedded. The comments are not.
CREATE TABLE tv_post AS
SELECT p.pk_post, p.id,
       jsonb_build_object('id', p.id, 'title', p.title, 'tags', to_jsonb(p.tags),
                          'commentCount', (SELECT count(*) FROM tb_comment c
                                           WHERE c.fk_post = p.pk_post)) AS data
FROM tb_post p;
```

What a reader composes by, as the registry publishes it:

```sql
SELECT ref.key AS column_name, ref.value->>'entity' AS parent,
       ref.value->>'column' AS parent_column, ref.value->>'index' AS index
FROM tviews.registry r, jsonb_each(r.references) ref
WHERE r.entity = 'comment';
```

A post with its comments, in one statement. The cheapest spelling splices the comments,
aggregated as `json`, into the parent's text before its closing brace: no `jsonb` is
rebuilt, which halves the cost of composing (the parent's `data` must hold a key).

```sql
SELECT left(p.data::text, -1) || ', "comments": '
       || COALESCE((SELECT json_agg(c.data ORDER BY c.pk_comment)
                    FROM tv_comment c WHERE c.fk_post = p.pk_post)::text, '[]')
       || '}' AS post
FROM tv_post p
WHERE p.pk_post = 1;
```

Correlate on the reference column itself (`c.fk_post = p.pk_post`), never on a key inside
`data` (`(c.data->>'fk_post')::bigint`): no managed index covers that expression, and a
page of 20 posts then reads the whole child table for each post.
