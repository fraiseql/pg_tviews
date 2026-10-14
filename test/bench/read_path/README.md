# Read-path benchmark

What reading `post.comments` costs when the comments are embedded in `tv_post.data`
against composing them at read time from `tv_comment`, and what each design costs on the
write path. The results and the rule they support are in
[docs/benchmarks/read-path.md](../../../docs/benchmarks/read-path.md).

Both designs hold the same documents, from the same seed (`seed.sql`: 2,000 users,
`POSTS` posts, 100,000 comments, deterministic ids):

| template | design |
|---|---|
| `rc_embedded` | `tv_post.data.comments[]`, the [#228](https://github.com/fraiseql/pg_tviews/issues/228) array shape of `../write_path/shapes.sql` |
| `rc_composed` | `tv_post` without comments, plus `tv_comment(pk_comment, id, fk_post, fk_author, data)` |
| `rc_composed_key` | `rc_composed` with the post's key inside each comment's `data` and an expression index on it (`key.sql`) |

`setup.sh` builds the three templates and fails unless the md5 over every embedded post
equals the md5 over every post composed from `tv_comment`. `POSTS=2000 SUFFIX=_50` builds
the 50-comments-per-post variant (`rc_embedded_50`, …).

| script | measures |
|---|---|
| `reads.sh [rounds]` | µs per read, server-side, for one post by id, a keyset page of 20 and 20 random posts, embedded and composed in five spellings (LATERAL with `jsonb_agg`, `json_agg` spliced into text, FraiseQL's composed shape on the native column and on the `data` key, and a batched `fk_post = ANY ($1)` second query) |
| `writes.sh [rounds]` | ms per write for an author rename, a comment's content, a post's title, a comment insert and a comment moved to another post, embedded, composed and plain (no triggers) |
| `projection.sh [rounds]` | a page of 20 posts selecting `id` and `title` only: what the array costs the reads that skip it |

Every cell runs on a fresh `CREATE DATABASE … STRATEGY FILE_COPY` copy of its template: a
warm-up loop, then a measured PL/pgSQL loop. Documents are cast to `text`, which is what
the wire carries. Variants are interleaved within each round: report medians.

Time a release build on a stock server, as for the write-path benchmark
([../write_path/README.md](../write_path/README.md#measure-a-release-build-on-a-stock-server)).
WAL per write is printed, but each copy starts after a checkpoint, so it is mostly
full-page images: do not compare the designs on it.
