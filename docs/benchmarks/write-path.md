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
keeps the statements it runs for the backend (see below), returns
from a statement trigger before any query when the TVIEW reads none of the changed
columns, and keeps and shares its catalog caches for the backend. What remains is the
work itself: the patch of the TVIEW rows, the value locks, and the trigger calls.

Embeds two joins away are patched in place since 0.1.0-beta.29; arrays of embedded
rows (`jsonb_agg`) are still recomputed (fraiseql/pg_tviews#228, below).

## Arrays of embedded rows and two-hop embeds

`shapes.sh` measures the two shapes #228 names on the same seed, one TVIEW per database
(`shapes.sql`): a username copied into `tv_post.comments[].author` (the 10 posts a user
commented on) and into `tv_comment.post.author` (the 50 comments on a user's posts).
`*.manual.sql` is the patch pg_tviews would generate, written by hand. Server-side time of
200 username writes in one transaction (no round trips, no commit), stock PostgreSQL
18.4, three rounds, ms per write:

| shape | rows | recompute | pg_tviews patch | hand-written patch | plain write |
|---|---:|---:|---:|---:|---:|
| array of embedded rows | 10 posts | 0.53 / 0.55 / 0.56 | (recomputes) | 0.25 / 0.25 / 0.21 | 0.01-0.02 |
| two hops | 50 comments | 1.01 / 1.03 / 1.02 | 0.47 / 0.47 / 0.47 | 0.28 / 0.25 / 0.24 | 0.01-0.04 |

Two hops: 2.2× less work per write. Arrays: the hand-written patch is 2.4× cheaper than the
recompute, and a pg_tviews patch path would add its own overhead (0.05-0.1 ms at this
fan-out), leaving about 1.8×, so they stay recomputed.

## How pg_tviews plans its statements

The statements a write runs are prepared once per backend and kept. How each is
planned depends on what it looks rows up by:

- **A unique key** (a primary key, a TVIEW's identity, a catalog key): one generic
  plan, never planned again.
- **One key of a column that repeats** (`fk_<entity>`, the children of one parent):
  the statement compares the column with `=`. Its generic plan estimates the rows of
  one value, which is what a plan made for that key would estimate, so it is not
  planned again either. A write to one parent runs this shape.
- **Several keys**: the statement takes them as an array and is planned at each
  execution, for their count (about 30 µs).

The statements that read a backing view or lock rows by identity (the recompute, the
row locks, the delete of rows gone from the view, and the parent lookup outside
REPEATABLE READ) are kept too, but planned by PostgreSQL's own rule
(`plan_cache_mode = auto`): for their arguments at their first five executions, then
from the generic plan when it costs no more than those did. A recompute of one row
settles on the generic plan (six plans over 2,000 writes on stock 18.4).

A generic plan of an array can't count its keys: PostgreSQL assumes ten, each matching
the column's average number of rows. On a TVIEW whose lookup column has few distinct
values (categories, statuses, tenants) that estimate is a large share of the table,
and the plan scans it all to find a few rows. Before this rule, a write to one category
of a freshly loaded 72,220-row TVIEW (85 categories) scanned it on every write
(fraiseql/pg_tviews#235); stock PostgreSQL 18.4, one client, a fresh database per run,
the mean of three interleaved rounds:

| category write | children | array, generic plan | by key count | recompute |
|---|---:|---:|---:|---:|
| one category | 1 | 5.46 ms | 0.145 ms | 0.224 ms |
| one category | 1,000 | 11.48 ms | 3.28 ms | 8.0 ms |

The writes of the table above did not change (`bio` 0.199 ms, `username` 0.483 ms),
nor did `bio_bulk100` (5.60 ms before, 5.64 ms after; its statements with 100 keys are
now planned at each execution).

A plan made for several keys knows how many there are, not which: one statement
writing many parents whose lookup values are each shared by many rows may still scan
the child TVIEW once. That scan is paid once for the statement, not once per parent.

