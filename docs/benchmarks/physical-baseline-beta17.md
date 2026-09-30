# Physical-cost baseline: v0.1.0-beta.17 defaults

What a TVIEW refresh costs **physically** (tuple versions, HOT eligibility, WAL,
dead tuples, relation growth, visibility-map churn) on the shipped defaults,
before any of the refresh/defaults changes planned in #70, #71, #72 and #73.
Those changes report their before/after numbers against this page.

Latency figures for the same harness are in [results.md](results.md).

## Environment

| | |
|---|---|
| pg_tviews | 0.1.0-beta.17 (`main` @ `1680970`; harness from `feat/issue-69-physical-bench`) |
| jsonb_delta | 0.3.0 |
| PostgreSQL | 18.1 (pgrx-managed cluster) |
| Machine | Intel i7-13700K, 24 threads, 94 GiB RAM, Linux 7.0.9 (developer workstation) |
| Server settings | `shared_buffers` 160 MB, `checkpoint_timeout` 300 s, `max_wal_size` 1 GB, `wal_level` replica, `full_page_writes` on, `data_checksums` on, `wal_compression` off |
| TVIEW defaults | `unlogged_by_default` on, GIN index on `data`, heap fillfactor 100, `direct_patch_enabled` on |

This run used a developer workstation, not the `provision-ubuntu.sh` box behind
[results.md](results.md), and pgrx's small default `shared_buffers`. The
**ratios and counters** (HOT %, tuples written, WAL per statement, buffers per
lookup) don't depend on the machine. Absolute milliseconds do. The full
environment is in [`env.tsv`](../../test/sql/real_benchmark/results/physical/baseline-beta17/env.tsv).

Reproduce:

```bash
cd test/sql/real_benchmark
RUN_DIR=$PWD/results/physical/baseline-beta17 ./run_physical.sh --scales "small medium large"
```

Every step passed its row-for-row TVIEW-vs-view divergence gate. The raw data
([`physical.csv`](../../test/sql/real_benchmark/results/physical/baseline-beta17/physical.csv))
and the full per-scenario report
([`report.md`](../../test/sql/real_benchmark/results/physical/baseline-beta17/report.md))
are committed next to the harness. Column definitions are in the harness
[README](../../test/sql/real_benchmark/README.md#physical-cost).

## Known pathologies

### 1. No refresh is ever HOT (#70, #73)

Across **every** scenario, mode and step (product catalogue at all three
scales, payload sweep, skewed fan-out), `tv_*` updates are **0.0 % HOT**. The
only non-zero values are in the no-op scenario: ≤ 0.3 % on the repeats, and
17–29 % in the small multi-path step, which lands on space the previous step's
dead tuples left prunable. The base tables in the same steps do get HOT (for
example `tb_doc` at up to 85 % for single-row updates), so the cause is on the
TVIEW side:
the default GIN index on `data` makes every `data` rewrite an indexed-column
update. Most new versions also leave the page (`n_tup_newpage_upd`), because
heap fillfactor is 100:

| Step | tv updates | HOT | new-page updates |
|---|---:|---:|---:|
| product large, `update_batch` (1 % of rows × 5) | 5 000 | 0 | 3 505 |
| no-op, 10 000 rows | 10 000 | 0 | 10 000 |
| fan-out, celebrity (100k posts) → `tv_post` | 100 000 | 0 | 69 458 |
| fan-out, celebrity (100k posts) → `tv_comment` | 235 015 | 0 | 162 664 |

Every refresh therefore inserts into **every** index on the TVIEW and leaves a
dead tuple that only VACUUM reclaims. The celebrity step leaves 328 523 dead
tuples in `tv_comment` and grows it by 34 MB heap + 17 MB index. The all-visible
fraction of `tv_post`/`tv_comment` drops from ~1.0 to 0.0, which disables
index-only scans until the next VACUUM.

### 2. Cascade propagation seq-scans the parent TVIEW (#71)

The propagation lookup scans the whole parent TVIEW whatever the fan-out. For
one p99 user (83 posts, 216 comments; auto_explain, LOGGED):

| Statement | Rows | Buffers (hit + read) |
|---|---:|---:|
| `SELECT fk_user, pk_post FROM tv_post WHERE fk_user = ANY($1)` | 83 | 8 651 |
| `SELECT fk_post, pk_comment FROM tv_comment WHERE fk_post = ANY($1)` | 216 | 25 805 |
| the two `UPDATE … jsonb_smart_patch_nested` that follow | 299 | 4 891 |

The lookups cost **7.0×** the buffers of the writes they feed. `seq_scan` on
`tv_comment` is 3 per `post_title` refresh (60 for 20 refreshes, LOGGED). The cost grows
with the TVIEW's size (300k posts, 700k comments here), not with the fan-out
(p50 12 posts per user, 2 comments per post).

### 3. Unchanged rows are rewritten (#72)

| Step (20 000-post TVIEW) | Distinct keys | tv tuples written | WAL (LOGGED) | WAL (UNLOGGED) |
|---|---:|---:|---:|---:|
| `UPDATE tb_post SET title = title` on 10 000 rows | 10 000 | 10 000 | 13.0 MB | 4.1 MB |
| the same statement × 3 | 10 000 | 30 000 | 30.6 MB | 9.0 MB |
| one transaction reaching 40 keys via author cascade + 2 direct no-ops | 40 | 120 | 0.93 MB | 0.31 MB |

A refresh whose output is byte-identical still writes a new tuple version, a GIN
entry and a dead tuple for each key. A key reached through several paths in one
transaction is written once per path, because the AFTER STATEMENT flush runs per
statement. (UNLOGGED WAL here is the base table's.)

### 4. One-field change rewrites the whole document into TOAST

`UPDATE tb_doc SET counter = counter + 1` touches one integer, but the refreshed
`data` is a new jsonb value, stored again in full:

| `data` size | TOAST growth per 200 refreshes | WAL per refresh, LOGGED (step) | `UPDATE tv_doc` WAL (auto_explain) |
|---:|---:|---:|---:|
| 100 B | 0 | 6.2 kB | — |
| 1 KB | 0 (inline) | 20.0 kB | — |
| 4 KB | 1.05 MB | 17.3 kB | — |
| 16 KB | 3.26 MB | 46.6 kB | — |
| 64 KB | 13.03 MB | 148.6 kB | 72.2 kB |

At 64 KB, each one-integer refresh adds ~65 KB of TOAST and ~72 kB
of WAL for the TVIEW write alone. This is outside #70–#73 and belongs to the
refresh-strategy research (#77/#78).

### 5. UNLOGGED: a seq scan on every refresh (#75)

In UNLOGGED mode each flush runs `SELECT EXISTS(SELECT 1 FROM tv_x LIMIT 1)`,
the lazy crash-recovery probe. `seq_scan` on the TVIEW equals the refresh count
(product: 25 for 25 `update_single`; payload: 200 for 200), versus 0 in LOGGED
mode. It stops after the first tuple, so it's cheap (1–3 buffers), but it runs
on every refresh.

## LOGGED vs UNLOGGED

UNLOGGED TVIEW writes generate no WAL. What remains in the UNLOGGED columns is
the base-table write:

| Step | WAL LOGGED | WAL UNLOGGED |
|---|---:|---:|
| product large, `update_batch` (5 × 1 000 rows) | 64.2 MB | 5.6 MB |
| product large, `update_single` (per refresh) | 51.4 kB | 29.7 kB |
| fan-out, celebrity 10k posts (32 731 tv rows) | 241 MB | 0.0 MB |
| fan-out, celebrity 100k posts (335 016 tv rows) | 506 MB | 72 MB ¹ |
| full `REFRESH MATERIALIZED VIEW`, product large (per refresh) | 80–108 MB | — |

¹ No logged relation in the database changed during this step. The WAL is
cluster-wide and most likely hint-bit full-page images: the recompute reads
`tb_post`/`tb_comment`, and with `data_checksums` on, the first hint-bit write
to a page after a checkpoint is WAL-logged. The 10k/30k steps didn't cross a
checkpoint. Treat per-step WAL on short steps as an upper bound; the
auto_explain column gives the TVIEW write's own WAL.

Per-refresh WAL in short steps also includes full-page images from the
`CHECKPOINT` that starts each scenario. For example, a product `update_single`
step shows ~51 kB per refresh while its `INSERT … ON CONFLICT` writes 2.2 kB.

## Cascade fan-out (skewed_fanout, SCALE=1)

| Edge | Parents | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|
| user → post | 10 000 | 12 | 36 | 85 | 100 000 |
| post → comment | 300 000 | 2 | 6 | 13 | 1 266 |
| user → comment | 9 999 | 27 | 92 | 206 | 235 015 |

| Step | tv rows rewritten | ms (UNLOGGED / LOGGED) |
|---|---:|---:|
| 5 median users | 181 | 180 / 176 |
| 3 p99 users | 300 | 44 / 45 |
| celebrity, 10k posts | 32 731 | 866 / 1 041 |
| celebrity, 30k posts | 100 397 | 3 060 / 3 244 |
| celebrity, 100k posts | 335 016 | 9 826 / 10 392 |

A single celebrity update is a ~10 s, 335k-tuple, non-HOT rewrite. The time is
linear in fan-out, but the physical side effects (dead tuples, bloat, lost
all-visible) follow each such update.

## What to compare against this baseline

| Issue | Metric to report | Baseline |
|---|---|---|
| #70 no GIN by default | `hot_pct` on `tv_*` | 0.0 % everywhere |
| #73 fillfactor 85 | `n_tup_newpage_upd / n_tup_upd` | 11–100 % (70 % product large batch, 100 % no-op) |
| #71 propagation index | buffers of the `WHERE fk_* = ANY($1)` lookup; `seq_scan` | 8 651 / 25 805 for 83 / 216 rows |
| #72 no-op guard | tv tuples written by the no-op steps | 10 000 / 30 000 / 120 |
| #75 standby safety | UNLOGGED per-refresh `seq_scan` | 1 per refresh |
