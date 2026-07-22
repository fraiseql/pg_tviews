# Performance Benchmark Results

Measured performance of pg_tviews' incremental refresh against a traditional
`REFRESH MATERIALIZED VIEW`, using the real `pg_tviews_create` API.

**pg_tviews**: 0.1.0 • **jsonb_delta**: 0.3.0 • **PostgreSQL**: 18.4 •
**Last updated**: 2026-07-22

> Reproduce with [`test/sql/real_benchmark/`](../../test/sql/real_benchmark/)
> (`run.sh --scales "small medium large"`). These numbers supersede the earlier
> `comprehensive_benchmarks/` figures, whose harness targeted a
> `pg_tviews.enable_tview(...)` function the shipped extension never exported and
> so never ran against the real extension.

## What is measured

A denormalised product catalogue — one JSONB row per product joining category,
supplier, inventory, and a review aggregate — maintained three ways:

| Arm | Approach | Maintenance on a base-table change |
|-----|----------|------------------------------------|
| **pg_tviews + jsonb_delta** | incremental refresh, surgical JSONB patch | refresh only the affected rows |
| **pg_tviews + native** | incremental refresh, no jsonb_delta (fallback) | refresh only the affected rows |
| **full refresh** | `REFRESH MATERIALIZED VIEW` | rebuild every row |

Only `tb_product` mutations are timed — the operation pg_tviews refreshes
incrementally **and** correctly (every run is gated on a row-for-row divergence
check of the tview against its backing view; a non-zero divergence fails the run).
Timing is `psql \timing` on autocommit statements, so each figure is the
end-to-end, client-observed cost **including** the post-statement refresh flush;
for the full-refresh arm the timed statement is the `REFRESH` a change forces.
Medians are reported (25 iterations for single-row ops, 5 for batch/refresh).

## Executive summary

pg_tviews turns O(n) view maintenance into O(1) for point changes. A single-row
update costs a **flat ~1.8–2.0 ms regardless of dataset size**, while a full
`REFRESH` grows linearly with the table. At 100K products / 500K reviews that is a
**~775× speedup for a single update and ~1,300× for a single delete.**

## The headline: incremental is flat, full refresh is linear

Single-row `UPDATE` on `tb_product`, median ms:

| Scale | products / reviews | pg_tviews (ms) | full refresh (ms) | speedup |
|-------|--------------------|----------------|-------------------|---------|
| small  | 1K / 5K    | 1.77 | 15.0    | **8.5×** |
| medium | 10K / 50K  | 1.87 | 167.9   | **90×** |
| large  | 100K / 500K | 1.97 | 1,523.5 | **775×** |

pg_tviews stays ~2 ms across a 100× growth in table size; full refresh tracks the
row count. The speedup is the ratio of the two and grows without bound as the
dataset grows.

## Per-operation detail

Median ms; **speedup** = full-refresh ÷ pg_tviews+jsonb_delta.

### Small — 1,000 products / 5,000 reviews
| Operation | pg_tviews+jsonb_delta | pg_tviews+native | full refresh | speedup |
|-----------|----------------------:|-----------------:|-------------:|--------:|
| single update | 1.77 | 1.67 | 15.0 | 8.5× |
| batch update (1% = 10 rows) | 5.12 | 4.97 | 15.2 | 3.0× |
| single insert | 1.83 | 1.67 | 14.9 | 8.2× |
| single delete | 1.15 | 1.14 | 15.2 | 13.3× |
| initial build (one-time) | 59.8 | 60.2 | 33.8 | 0.6× |

### Medium — 10,000 products / 50,000 reviews
| Operation | pg_tviews+jsonb_delta | pg_tviews+native | full refresh | speedup |
|-----------|----------------------:|-----------------:|-------------:|--------:|
| single update | 1.87 | 1.81 | 167.9 | 90× |
| batch update (1% = 100 rows) | 39.3 | 37.2 | 140.3 | 3.6× |
| single insert | 1.79 | 1.81 | 165.8 | 93× |
| single delete | 1.12 | 1.18 | 139.8 | 124× |
| initial build (one-time) | 405.8 | 395.3 | 156.2 | 0.4× |

### Large — 100,000 products / 500,000 reviews
| Operation | pg_tviews+jsonb_delta | pg_tviews+native | full refresh | speedup |
|-----------|----------------------:|-----------------:|-------------:|--------:|
| single update | 1.97 | 1.94 | 1,523.5 | 775× |
| batch update (1% = 1,000 rows) | 540.3 | 542.1 | 1,517.0 | 2.8× |
| single insert | 1.91 | 1.86 | 1,522.0 | 799× |
| single delete | 1.18 | 1.20 | 1,551.1 | 1,313× |
| initial build (one-time) | 4,426.7 | 4,382.8 | 1,498.7 | 0.3× |

## Reading the numbers honestly

- **Point changes win enormously; batch changes win less.** A batch touching 1%
  of rows still refreshes 1% of the tview, so its cost rises with the batch size
  (540 ms for 1,000 rows at large scale) and the advantage over a full rebuild
  narrows to ~2.8×. Incremental refresh pays off in proportion to how *small* the
  change is relative to the table.

- **jsonb_delta vs native is at parity here (0.92–1.05×).** For these
  moderately-sized JSON documents, jsonb_delta 0.3.0's surgical scalar patch and
  the native fallback are statistically indistinguishable. jsonb_delta's advantage
  is on large nested/array-valued documents, which this catalogue does not stress;
  its value here is having a stable, presence-detected patch path, not a headline
  speed win. Install it, but do not expect it to move these numbers.

- **Initial build is a one-time cost pg_tviews loses.** Building the tview
  (trigger + metadata setup, then materialising every row through the refresh
  machinery) is ~3× slower than a single `CREATE MATERIALIZED VIEW`. This is paid
  once at creation; every subsequent change is where pg_tviews wins.

## Scope and limitations

- **Direct entity mutations only.** These figures cover changes to the tview's own
  base table (`tb_product`). Cascades from joined dimension/aggregate tables
  (category, reviews) only propagate when the parent is itself a registered `tv_`
  entity exposing its `fk_<parent>` key; this single-entity benchmark embeds that
  data inline and does not exercise cascade propagation. See
  `test/sql/42_cascade_fk_lineage.sql` for the cascade contract.
- **Non-concurrent full refresh.** The baseline is plain `REFRESH MATERIALIZED
  VIEW` (a full O(n) rebuild). `REFRESH ... CONCURRENTLY` trades a unique-index
  requirement and a full diff for non-blocking behaviour; it is not faster.
- Moderate JSON document size; single client; medians over repeated autocommit
  statements.

## Test environment

- **Machine**: Hetzner ccx33 — 8 dedicated vCPU (AMD EPYC-Milan), 30 GiB RAM
- **PostgreSQL**: 18.4 (PGDG), default `shared_buffers = 128MB` (untuned, for
  reproducibility)
- **Extensions**: pg_tviews 0.1.0 (pgrx 0.17.0), jsonb_delta 0.3.0
- **Harness**: `test/sql/real_benchmark/`; each arm in its own database seeded from
  a common template; every run divergence-gated for correctness

## See also

- [Benchmark Overview](overview.md) — methodology and scenarios
- [`test/sql/real_benchmark/README.md`](../../test/sql/real_benchmark/README.md) — harness details
