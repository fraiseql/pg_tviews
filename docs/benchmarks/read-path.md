# Read path: embed or compose

What a reader pays for a child list embedded in the parent's document
(`tv_post.data.comments[]`) against the same list composed at read time from the child's
own TVIEW (`tv_comment`), and what each design costs on the write path. The harness is
[`test/bench/read_path/`](../../test/bench/read_path/README.md). The rule these numbers
support is in the user guide: [Embed or compose](../user-guides/embed-or-compose.md).

## Setup

- Stock PostgreSQL 18.4, release builds of pg_tviews and jsonb_delta, `fsync` on,
  `shared_buffers = 1GB`.
- The write-path array seed (`shapes.sql`, shape `array`) with deterministic ids: 2,000
  users, 20,000 posts, 100,000 comments, so 5 comments per post. Each user wrote 50
  comments, spread over 10 posts.
- **embedded**: `tv_post.data.comments[]`, each comment with its author nested.
  **composed**: `tv_post` without comments, plus `tv_comment(pk_comment, id, fk_post,
  fk_author, data)`. The two hold byte-identical documents: `setup.sh` checks the md5
  over all 20,000 composed posts.
- Every cell runs on a fresh `CREATE DATABASE … STRATEGY FILE_COPY` copy, a warm-up loop,
  then a measured PL/pgSQL loop. Every document is cast to `text`. Medians of 5 rounds
  (8 for 50 comments per post), variants interleaved; the spread is ≤ 5 % except where
  noted.

Read variants, all composed from `tv_comment` except the first:

| variant | SQL |
|---|---|
| embedded | `SELECT data::text FROM tv_post …` |
| lateral | `p.data \|\| jsonb_build_object('comments', (SELECT jsonb_agg(c.data ORDER BY c.pk_comment) … WHERE c.fk_post = p.pk_post))` |
| lateral_text | the same rows, with `json_agg` spliced into `p.data::text`: no jsonb rebuilt |
| fraiseql | the shape FraiseQL's composed read renders (`jsonb_build_object('d', doc, 'e', …)`, `row_number()`, `jsonb_agg`), correlated on the native `fk_post` |
| fraiseql_key | the same, correlated on `(data->>'fk_post')::int` as FraiseQL renders it today, with an expression index |
| batched | two statements: the posts, then `… FROM tv_comment WHERE fk_post = ANY ($1)`, stitched by the client (not timed), one more round trip (10 µs on localhost TCP) |

## Reads, µs per read (5 comments per post)

| read | embedded | lateral_text | lateral | fraiseql | fraiseql_key | batched |
|---|---:|---:|---:|---:|---:|---:|
| 1 post by id | **10.1** | 15.0 | 20.6 | 23.2 | 24.7 | 22.5 (+RT) |
| page of 20 (keyset) | **61.9** | 108.1 | 202.3 | 240.7 | 265.4 | 111.5 (+RT) |
| 20 random posts | **83.7** | 150.5 | 252.5 | 285.3 | 311.1 | 180.9 (+RT) |

- Correlated on `data->>'fk_post'` with **no** expression index, a page takes about
  **180 ms**: a sequential scan of `tv_comment` per parent row.
- The cost of `lateral` is mostly building jsonb, not the lookup: the managed
  `(fk_post, pk_comment)` index makes each LATERAL an index range scan already in
  `pk_comment` order. `lateral_text` reads the same rows for half the cost.
- 20 random posts cost more than a page because their comments sit on about 100 heap
  pages against about 5 for consecutive posts. An embedded read has no such effect.
- Reads that do not select the list favour composition, the parent document being
  smaller: a page of 20 projected to `{id, title}` costs 23–26 µs embedded against
  22–24 µs composed.

## Writes, ms per write

| write | embedded | composed | plain | saving if `tv_comment` exists only to compose (A) | saving if `tv_comment` exists anyway (B) |
|---|---:|---:|---:|---:|---:|
| author rename (50 comments, 10 posts) | 0.567 | 0.337 | 0.010 | 0.230 | **0.557** |
| comment content | 0.108 | 0.056 | 0.016 | 0.052 | 0.092 |
| comment insert | 0.130 | 0.103 | 0.018 | 0.027 | 0.112 |
| comment moved to another post | 0.362 | 0.121 | 0.023 | 0.241 | 0.339 |
| post title | 0.067 | 0.045 | 0.021 | 0.022 | 0.022 |

- Case B adds the composed design's `tv_comment` work to the embedded design: an
  application with a `Comment` type (comment queries, `comment(id:)`) maintains
  `tv_comment` in both designs. Under B, the rename saving is the whole cost of keeping
  the array.
- WAL per write is dominated by full-page images (each copy starts after a checkpoint):
  it says nothing about the designs here.

## Break-even

Composition wins on raw CPU, with no cache, below this many reads per child write: the
write saving divided by the extra read cost. Best single-statement spelling
(`lateral_text`) / FraiseQL's current renderer (`fraiseql`).

| write (case B) | single-post reads | pages of 20 |
|---|---:|---:|
| author rename (0.557 ms) | 114 / 43 | 12 / 3.1 |
| comment moved (0.339 ms) | 69 / 26 | 7.3 / 1.9 |
| comment insert (0.112 ms) | 23 / 8.5 | 2.4 / 0.6 |
| comment content (0.092 ms) | 19 / 7.0 | 2.0 / 0.5 |
| author rename, case A (0.230 ms) | 47 / 18 | 5.0 / 1.3 |

Read-heavy GraphQL traffic, 10–100 page reads per child write, sits above these lines:
with no cache, embedding wins on CPU for this shape. What moves the decision is
elsewhere: nested pagination, filters and ordering, child row security, unbounded lists,
writes that fan out from other entities, and a result cache that evicts composed results
precisely ([collection keys](../user-guides/change-feed.md#collection-keys)), which pays
the read overhead only on a miss.

## 50 comments per post

`POSTS=2000 SUFFIX=_50` (2,000 posts, TOASTed 13 kB parents). In this seed a user's 50
comments land on one post.

| | embedded | lateral_text | fraiseql | batched |
|---|---:|---:|---:|---:|
| 1 post (µs) | 27.2 | 56.2 | 102.8 | 70.3 |
| page of 20 (µs) | 471.6 | 844.0 | 1,820 | 914.1 |
| page of 20, `{id, title}` only (µs) | 42.3 | 23.0 (composed parent) | | |

| write (ms) | embedded | composed | plain |
|---|---:|---:|---:|
| comment content | 0.373 | 0.049 | 0.011 |
| comment insert | 0.399 | 0.100 | 0.017 |
| comment moved | 0.752 | 0.110 | 0.022 |
| post title | 0.123 | 0.040 | 0.010 |

Longer arrays make embedded writes 3–7× dearer, and composed reads dearer too, at about
0.4–0.5 µs per composed element with the best spelling. The break-even ratio stays the
same order: array length alone does not flip the decision.

## Reproducing

`setup.sh`, then `reads.sh 5`, `writes.sh 5` and `projection.sh 5` on a stock server. A
rerun on 2026-10-14 (load 2–4) reproduced every read cell within 10 %, and every write
cell within 10 % but the composed comment insert (0.115 ms, +12 %).
