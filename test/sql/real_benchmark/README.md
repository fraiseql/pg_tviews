# Real benchmark (uses the actual `pg_tviews_create` API)

This harness measures pg_tviews against a traditional materialized view using
the **real extension API** — `pg_tviews_create('tv_product', <select>)` — not a
simulation. It replaces the older `../comprehensive_benchmarks/` suite, whose
`04_way_comparison.sql` targets a `pg_tviews.enable_tview(...)` function that the
shipped extension never exported (its numbers were never produced against the
real extension).

## What it compares

A denormalised product catalogue — one JSONB row per product, joining category,
supplier, inventory, and a review aggregate — maintained three ways:

| Arm | Approach | Maintenance on a base change |
|-----|----------|------------------------------|
| A | pg_tviews + jsonb_delta | incremental refresh, surgical JSONB patch |
| B | pg_tviews + native | incremental refresh, no jsonb_delta (fallback) |
| C | full `REFRESH MATERIALIZED VIEW` | O(n) rebuild of every row |

Only `tb_product` mutations are timed — the operation pg_tviews refreshes
incrementally **and** correctly. Every run gates on a row-for-row divergence
check of `tv_product` against its backing view (`v_product`); a non-zero
divergence fails the run. Cascades from the embedded dimension/aggregate tables
(category, reviews) are intentionally out of scope here: they only propagate when
the parent is itself a registered `tv_` entity, which this single-entity
benchmark does not model.

## Measurement

`psql \timing` on an autocommit statement, so each number is the end-to-end,
client-observed cost **including** the post-statement refresh flush. For arm C
the timed statement is the `REFRESH` that a change forces. Medians are reported
over many iterations (`--single-iters`, `--c-iters`).

## Running

```bash
PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales "small medium large"
```

Requires `pg_tviews` in `shared_preload_libraries`, and `jsonb_delta` + `pg_tviews`
installable in the cluster. Scales: small = 1K products / 5K reviews,
medium = 10K / 50K, large = 100K / 500K. Results land in `results/` (`raw.tsv`,
`summary.tsv`, per-arm `\timing` logs). `aggregate.py` prints the comparison
table and writes `summary.tsv`.

`docs/benchmarks/results.md` is generated from a run of this harness.
