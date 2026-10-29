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

## 0.1.0-beta.34 against 0.1.0-beta.33 (2026-10-29)

Stock PostgreSQL 18.6 (Arch), release builds (beta.34 at 7d830f98, `.so` md5 bb023cd0…;
beta.33 17d8ccd0…), jsonb_delta 0.3.0, `synchronous_commit = off`, `autovacuum = off`, both
postmasters pinned to the same two cores. `test/bench/write_path/compare.sh` interleaved the
builds per write, alternating which ran first, and started each run only when the
machine's 1-minute load average was below 4 (1.5–7.1 across the 72 runs, none discarded).
Three rounds of 15 s, a fresh database copy per run; the ratio is the median of the three
paired beta.34 / beta.33 runs.

| write | clients | beta.33 tps | beta.34 tps | paired ratio | WAL B/write, beta.33 | beta.34 |
|---|---:|---:|---:|---:|---:|---:|
| `bio` | 1 | 5,131 | 5,051 | 0.99 | 13,583 | 9,128 |
| `bio` | 16 | 6,392 | 6,323 | 0.99 | 13,785 | 9,026 |
| `username` | 1 | 1,958 | 2,035 | 1.02 | 43,055 | 24,549 |
| `username` | 16 | 2,511 | 2,532 | 1.01 | 42,508 | 24,030 |
| `title` | 1 | 4,952 | 5,280 | 1.07 | 6,479 | 3,633 |
| `title` | 16 | 5,868 | 6,845 | 1.17 | 6,625 | 3,324 |
| `comment_insert` | 1 | 6,984 | 7,134 | 0.98 | 2,314 | 1,802 |
| `comment_insert` | 16 | 8,455 | 8,518 | 1.01 | 2,207 | 1,746 |
| `bio_bulk100` | 1 | 184 | 176 | 0.96 | 1,417,363 | 923,625 |
| `bio_bulk100` | 16 | 41 | 40 | 0.98 | 2,036,621 | 1,948,315 |
| `untracked_insert` | 1 | 61,447 | 60,348 | 0.98 | 295 | 295 |
| `untracked_insert` | 16 | 81,279 | 80,545 | 0.99 | 295 | 295 |

No write is more than 4 % slower (`bio_bulk100` at one client, the largest gap), and
`title` is 7–17 % faster. Every write that changes TVIEW rows writes 25–45 % less WAL
(`bio_bulk100` at 16 clients 4 %): the rows no longer carry `created_at` and `updated_at`.
A write that changes no link takes no shared value lock; one that does also checks
its link columns.

**Crossed writes** ([#263](https://github.com/fraiseql/pg_tviews/issues/263)): N users,
one post each, every user commenting on every other's post; client *i* updates the `bio`
of user *i*, so each write's refresh of the posts reads the users the other clients
write. `embed` nests `tv_user` and `tv_comment` rows into `tv_post`; `mapped` joins the
base tables. 15 s, READ COMMITTED, `deadlock_timeout = 50ms`, no retries; tps over the
three rounds, failed transactions (all deadlocks) per run:

| shape | clients | beta.33 tps | beta.33 deadlocks | beta.34 tps | beta.34 deadlocks |
|---|---:|---:|---:|---:|---:|
| `embed` | 2 | 16 | 251–252 | 3,885–3,902 | 0 |
| `embed` | 20 | 0 | 424–536 | 260–285 | 0 |
| `mapped` | 2 | 60 | 229 | 4,396–4,407 | 0 |
| `mapped` | 20 | 0 | 105–125 | 405–417 | 0 |

Every TVIEW was fresh after every run, on both builds.

**The first write in a backend, and the first after DDL**
([#266](https://github.com/fraiseql/pg_tviews/issues/266)), measured at b20c7ef5 on a second
machine: Debian 13, 12 cores, PostgreSQL 18.6 built from source with the default
`configure`, the same settings (beta.34 `.so` md5 d68280b5…, beta.33 b978b3af…). Backend CPU
per write from `/proc/<pid>/schedstat`, median of three rounds: 300 warm writes, 100 writes
each right after `pg_tviews_invalidate_caches()` of the catalog (what TVIEW DDL anywhere
does to every backend), 30 first writes of a new backend. An author `UPDATE` reaching three
TVIEWs (#266's schema), then reaching five in a schema of 13 TVIEWs:

| ms CPU per write | 3 TVIEWs, beta.33 | beta.34 | 13 TVIEWs, beta.33 | beta.34 |
|---|---:|---:|---:|---:|
| warm | 2.77 | 2.21 | 3.46 | 2.81 |
| warm, after a `ROLLBACK` | 14.88 | 2.24 | 23.91 | 2.88 |
| after TVIEW DDL anywhere | 14.54 | 4.19 | 24.36 | 5.90 |
| first write of a backend | 21.25 | 11.99 | 32.03 | 16.09 |
| the same write, triggers off | 0.080 | 0.081 | 0.081 | 0.081 |

A rolled-back write now costs what a warm one does (10 nested statements instead of 40 for
three TVIEWs). After DDL and in a new backend, a write reads the catalog rows of every TVIEW
it reaches at once, without the change feed's collections unless the feed names them, and
plans each catalog read once per backend: kept plans outlive cache invalidations. What is
left in a new backend is mostly PostgreSQL loading its own caches and planning each refresh
statement once. The first `bio` write of the benchmark schema, ms (median of 20, three
rounds): warm 0.51 / 0.51, right after `CREATE INDEX` on a base table 0.71 / 0.65, right after
`pg_tviews_create` of an unrelated TVIEW 6.68 / 1.93 (beta.33 / beta.34).

**A `UNION` TVIEW's refresh reads its backing view once**
([#267](https://github.com/fraiseql/pg_tviews/issues/267)): the author write of #267's schema
read 96 buffers of the view to look for a duplicated key, then 348 in the upsert (96 of
them the same rows); now 348.

**TRUNCATE of a partition** holding 1 % of a table's rows (1,000 of 100,000 lines, 100 of
10,000 orders, an aggregate TVIEW over the orders), median of 10, three rounds: 1,062–1,066
ms on beta.33, which refreshed the TVIEW whole, and 4.5–4.6 ms on beta.34, which refreshes
the 100 orders the lines reached.

## 0.1.0-beta.33 against 0.1.0-beta.32

beta.33 changes no code on the write path (its fix runs on `DROP TABLE` and `DETACH
PARTITION` only); this run checks that, with a write added: `untracked_insert`, an insert into
`tb_audit`, a table no TVIEW reads, which measures what the installed extension costs a table
it does not maintain. Stock PostgreSQL 18.6 (Arch), release builds, `synchronous_commit =
off`, both clusters pinned to the same cores, on a machine shared with other builds (load
30–75): runs of single writes swung by 2× or more from one minute to the next, so only
adjacent pairs are compared.

Ten alternating 5 s pgbench runs per build, one client, one database copy per build; tps:

| write | beta.32, median | beta.33, median | |
|---|---:|---:|---:|
| `bio` | 2,550 | 2,552 | +0.1 % |
| `untracked_insert` | 34,058 | 33,881 | −0.5 % |

All six writes, interleaved per write in three rounds at 1 and 16 clients (15 s runs, fresh
copies): the paired head / beta.32 ratios ranged 0.35–2.6 with no direction, the noise of the
machine. A difference below a few percent cannot be resolved on a shared box; the code says
there is none.

## 0.1.0-beta.29 against 0.1.0-beta.28

Stock PostgreSQL 18.4 (Arch), release builds, data directory on NVMe with `fsync` on (a
plain write costs ~0.5 ms here, mostly the commit), pgbench one client, 10 s per run, a
fresh copy of the seeded database per run, three interleaved rounds, ms per write;
`publish_changes` and `max_refresh_rows` at their defaults (off). "beta.28" is 0f010972.

| write | beta.28 | beta.29 | | WAL B, beta.28 | WAL B, beta.29 |
|---|---:|---:|---:|---:|---:|
| `bio` | 0.574 / 0.556 / 0.569 | 0.574 / 0.560 / 0.550 | −0.9 % | 14,779 | 14,751 |
| `title` | 0.552 / 0.561 / 0.542 | 0.553 / 0.543 / 0.560 | +0.1 % | 11,382 | 11,390 |
| `username` | 0.858 / 0.860 / 0.869 | 0.857 / 0.860 / 0.865 | −0.2 % | 44,011 | 44,069 |
| `comment_insert` | 0.526 / 0.537 / 0.528 | 0.540 / 0.518 / 0.517 | −1.0 % | 6,080 | 6,016 |
| `bio_bulk100` | 6.722 / 6.736 / 6.748 | 6.882 / 6.912 / 6.927 | +2.5 % | 1,412,717 | 1,417,250 |
| fan-out 1 (`fanout.sql`) | 0.513 / 0.521 / 0.529 | 0.534 / 0.525 / 0.533 | +1.9 % | 1,168 | 1,170 |
| fan-out 1,000 | 4.038 / 4.044 / 4.061 | 4.139 / 4.142 / 4.121 | +2.1 % | 884,827 | 896,980 |
| `username_two_hop` (`shapes.sql`) | 1.364 / 1.360 / 1.365 | 0.749 / 0.752 / 0.747 | −45 % | 28,754 | 22,067 |

Writes that change a thousand TVIEW rows cost about 2 % more: each refresh write now
returns the rows' public ids, so that `pg_tviews_flush_and_report()` and the change feed
never look rows up again. A multi-key refresh of an aggregate TVIEW no longer evaluates its
view a second time (fraiseql/pg_tviews#237, not in this table: 17× on 100 keys, 140× on
2 keys of a 5,000-group aggregate).

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

