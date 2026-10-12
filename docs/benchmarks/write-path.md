# Write path

What a write to a base table costs with pg_tviews maintaining FraiseQL-shaped TVIEWs,
against the same write with triggers off. The harness is
[`test/bench/write_path/`](../../test/bench/write_path/README.md).

## Setup

velocitybench's CQRS schema: `tv_user`, `tv_post` (the author nested as `author`) and
`tv_comment` (the author and the post nested as `author` and `post`), seeded with 2,000
users, 20,000 posts and 100,000 comments. Each write is a pgbench script; "plain" runs
it with `session_replication_role = replica` (no triggers).

| write | TVIEW rows it changes |
|---|---|
| `bio` | 1 `tv_user`, 10 `tv_post` (`author.bio`) |
| `username` | 1 `tv_user`, 10 `tv_post`, 50 `tv_comment` (`author.username`) |
| `title` | 1 `tv_post`, ~5 `tv_comment` (`post.title`) |
| `comment_insert` | 1 new `tv_comment` row |
| `bio_bulk100` | 100 `tv_user`, 1,000 `tv_post` rows, one statement |

Stock PostgreSQL 18.4 (Arch), release builds of pg_tviews and jsonb_delta 0.3.0,
`fsync = off`, `shared_buffers = 512MB`, 15 s per run.

## Results

Transactions per second, the mean of two interleaved rounds (one client) and one round
with 16 clients. "Before" is 5f10e37d, the beta.28 branch before fraiseql/pg_tviews#224;
"plain" is the same write without triggers.

| write | before, 1 client | after, 1 client | plain | after vs plain | before, 16 clients | after, 16 clients |
|---|---:|---:|---:|---:|---:|---:|
| `bio` | 764 | 4,930 (6.5×) | 19,850 | 4.0× | 9,120 | 50,480 (5.5×) |
| `username` | 347 | 2,074 (6.0×) | 15,020 | 7.2× | 4,323 | 20,736 (4.8×) |
| `title` | 893 | 5,042 (5.6×) | 19,640 | 3.9× | 11,012 | 54,061 (4.9×) |
| `comment_insert` | 2,062 | 7,155 (3.5×) | 20,580 | 2.9× | 23,614 | 67,404 (2.9×) |
| `bio_bulk100` | 53 | 178 (3.4×) | 5,200 | 29× | 32 | 31 |

Per write, a `bio` UPDATE cost ~1.26 ms more than the plain one before and ~0.15 ms
after. `bio_bulk100` rewrites 1,100 TVIEW rows for 100 base rows: what remains is
that work; at 16 clients every write of it waits on the same rows, plain ones too.

## Where the time went

Before, a `bio` write cost ~1.3 ms more than the plain write
(fraiseql/pg_tviews#224):

- the posts embedding the author were recomputed through their view, because the
  fan-out patch wrote top-level keys only and FraiseQL nests the joined entity;
- nine of the ten statements a write runs were planned every time (0.5 ms);
- each mapped TVIEW's statement trigger queried the transition tables even when the
  statement changed nothing it reads (`tv_comment` on a `bio` write);
- the catalog rows and the tables a write reaches were read again per transaction, and
  copied on every lookup.

0.1.0-beta.28 patches nested fields in place (one statement for every embedding row),
keeps the statements' plans for the backend (the patches with generic plans), returns
from a statement trigger before any query when the TVIEW reads none of the changed
columns, and keeps and shares its catalog caches for the backend. What remains is the
work itself: the patch of the TVIEW rows, the value locks, and the trigger calls.

Still recomputed: arrays of embedded rows (`jsonb_agg`) and embeds two joins away
(fraiseql/pg_tviews#228).
