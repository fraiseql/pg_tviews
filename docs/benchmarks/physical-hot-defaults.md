# Physical cost with HOT-friendly defaults (#70, #73)

The [beta.17 physical baseline](physical-baseline-beta17.md) rerun with the new TVIEW
storage defaults: **no GIN index on `data`** (`pg_tviews.data_gin_index = off`) and
**fillfactor 85** (`pg_tviews.fillfactor`). See
[HOT Updates and TVIEW Storage](../operations/hot-updates.md) for the mechanism.

| | |
|---|---|
| Build | `main` @ `1680970` + #71 (propagation indexes) + #70/#73 (this change). **Not** #72 (no-op guard): no-op steps still rewrite rows here |
| Machine / server | same as the baseline (i7-13700K, PG 18.1, `shared_buffers` 160 MB, `data_checksums` on) |
| Harness | `test/sql/real_benchmark/run_physical.sh --scales "small medium large"`, both TVIEW modes |

Every step passed its TVIEW-vs-view divergence gate. Physical counters come from the
full matrix run. The machine was loaded during that run (load average 6.4), which
inflated some large-scale timings, so **all timings below come from quiet reruns**
of the affected scenarios (load average < 2.5). Counters don't depend on load.

## HOT ratio on TVIEW updates

| Scenario / step | before | after |
|---|---:|---:|
| product large, `update_single` / `update_batch` | 0.0 % / 0.0 % | **100.0 % / 99.6 %** |
| payload sweep, every size (100 B–64 KB), single and batch | 0.0 % | **100.0 %** |
| fan-out: median user, p99 user, post titles (post + comment) | 0.0 % | **100.0 %** |
| fan-out: celebrity 10k posts (`tv_post` / `tv_comment`) | 0.0 % | **100.0 % / 99.9 %** |
| fan-out: celebrity 30k posts | 0.0 % | **98.8 % / 96.2 %** |
| fan-out: celebrity 100k posts | 0.0 % | 54.1 % / 53.3 % |
| no-op, 10k rows once / × 3 | 0.0 % / 0.3 % | 19.2 % / 44.6 % |

## What HOT saves

| Step (LOGGED) | new-page updates | index growth | WAL |
|---|---:|---:|---:|
| product large, `update_batch` (5 000 rows) | 3 505 → 21 | 1.59 → 0.00 MB | 64.2 → 28.9 MB |
| fan-out, celebrity 10k (32 731 rows) | 32 305 → 15 | 3.76 → 0.00 MB | 241 → 138 MB |
| fan-out, celebrity 30k (100 397 rows) | 70 516 → 3 025 | 11.0 → 0.26 MB | 146 → 120 MB |
| fan-out, celebrity 100k (335 016 rows) | 232 122 → 155 684 | 27.0 → 18.4 MB | 506 → 164 MB |
| fan-out, 5 median users | 177 → 0 | 0.02 → 0.00 MB | 3.8 → 1.4 MB |

The all-visible fraction also stays higher (for example 0.67 → 0.73 after the
product batch step), and in-page pruning reclaims most dead versions without VACUUM:
in the payload sweep, dead tuples left after 200 single-row refreshes drop from 200
to about 30.

## Timings (quiet reruns)

| Step | before | after |
|---|---:|---:|
| product large, `build` (create 100k-row TVIEW) | 6.0–6.4 s | **2.2–2.3 s** (no GIN to build) |
| product large, `update_batch` median | 534–546 ms | 498–503 ms (−6 to −8 %) |
| product large, single-row ops median | 1.45–2.61 ms | within ±3 % |
| payload sweep, all sizes | — | within noise; batches up to −30 % at small sizes |
| fan-out, median users (5) / p99 users (3) / post titles (20), LOGGED | 176 / 45 / 442 ms | 35 / 11 / 44 ms ¹ |
| fan-out, celebrity 10k / 30k / 100k, LOGGED | 1.04 / 3.24 / 10.4 s | 0.50 / 1.44 / 6.43 s ¹ |

¹ Includes #71's propagation indexes. #71 alone made the celebrity steps 7–16 %
*slower* (every non-HOT rewrite also had to maintain the new index). With HOT the
rewrites skip index maintenance, and those steps are now 1.6–2.2× faster than the
baseline.

## Where HOT still falls short

A HOT update needs free space on the row's own page. Fillfactor 85 leaves room for
about 15 % of a page's rows to get a new version before the page is pruned. When a
single statement rewrites **most rows of the same pages** (the 100k-post celebrity,
or a no-op `UPDATE` over 10k contiguous rows), the free space runs out partway
through and the rest spill to new pages:

- **No-op rewrites** disappear with #72 (no write at all), so this case goes away.
- **Genuine mass rewrites** of one TVIEW (every row of a large table refreshed in
  one statement) can use a lower fillfactor for that TVIEW
  (`SET LOCAL pg_tviews.fillfactor = 70` at creation), at the cost of more heap.
  Most refresh traffic (single rows and fan-outs up to tens of thousands of rows)
  is already 96–100 % HOT at 85.
