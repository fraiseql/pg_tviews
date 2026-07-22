# Understanding Benchmark Results

How to read the pg_tviews benchmark output honestly — what the numbers mean,
where the advantage is real, and where it isn't. All figures cited here come from
[results.md](results.md), a measured run of
[`test/sql/real_benchmark/`](../../test/sql/real_benchmark/README.md).

**pg_tviews**: 0.1.0 • **jsonb_delta**: 0.3.0 • **PostgreSQL**: 18.4 •
**Last updated**: 2026-07-22

## The three arms

| Arm | What it is |
|-----|------------|
| **pg_tviews + jsonb_delta** | incremental refresh with jsonb_delta's surgical scalar patch |
| **pg_tviews + native** | incremental refresh with the in-core JSONB fallback |
| **full refresh** | plain `REFRESH MATERIALIZED VIEW` — the O(n) baseline |

All three produce identical tview rows; every run is gated on a row-for-row
divergence check before its timings are trusted. Speedups are
`full_refresh ÷ pg_tviews+jsonb_delta` on the same operation and scale.

## The headline: flat vs linear

The core finding is a **complexity-class difference**, not a constant factor. A
single-row `UPDATE`:

| Scale | pg_tviews (ms) | full refresh (ms) | speedup |
|-------|---------------:|------------------:|--------:|
| small (1K products)   | 1.77 | 15.0    | 8.5× |
| medium (10K products) | 1.87 | 167.9   | 90× |
| large (100K products) | 1.97 | 1,523.5 | 775× |

pg_tviews stays ~1.8–2.0 ms across a **100× growth** in table size; the full
rebuild tracks the row count. That is why the speedup grows without bound: it is
the ratio of a flat line to a rising one. At large scale a single delete reaches
**1,313×** (1.18 ms vs 1,551 ms).

**How to read it:** don't fixate on the multiplier at any one scale. The point is
that the incremental cost is *independent of dataset size* while the full-refresh
cost is *proportional to it*. The bigger your table, the larger the gap — and at
100K+ rows a full refresh per change (~1.5 s) is simply not viable for
interactive workloads.

## Where the advantage narrows

Read these caveats before quoting a headline number:

### Batch changes win less than point changes

A batch touching 1% of rows still refreshes 1% of the tview, so its cost rises
with the batch size. At large scale a 1,000-row batch update is 540 ms vs 1,517 ms
for a full refresh — **~2.8×**, not 775×. Incremental refresh pays off in
proportion to how *small* the change is relative to the table. If you routinely
rewrite large fractions of a table in one statement, a full refresh is closer to
competitive.

### Initial build is a cost pg_tviews loses

Building the tview (trigger + metadata setup, then materialising every row
through the refresh machinery) is ~3× slower than a single
`CREATE MATERIALIZED VIEW` — e.g. at large scale 4,427 ms vs 1,499 ms. This is a
**one-time** cost paid at creation; every subsequent change is where pg_tviews
wins it back. It is reported as the `build` op so it is visible, not hidden.

### jsonb_delta vs native is a wash on this workload

The `A vs B` column sits at 0.92–1.05× everywhere. For the moderately-sized JSON
documents in this catalogue, jsonb_delta's surgical scalar patch and the native
fallback are indistinguishable. jsonb_delta earns its keep on large nested /
array-valued documents (see
[jsonb-ivm-integration.md](jsonb-ivm-integration.md) and
[array-fastpath-go-no-go.md](array-fastpath-go-no-go.md)), which this benchmark
does not stress. Install it for the stable patch path, not for these numbers.

## What the timings include

Each figure is `psql \timing` on an autocommit statement — the end-to-end,
client-observed cost, **including** the post-statement refresh flush for the
pg_tviews arms. For the full-refresh arm the timed statement is the `REFRESH`
that a change forces. So these are not internal micro-timings; they are what an
application actually waits for. Medians are reported (25 iterations for single-row
ops, fewer for batch/build/refresh).

## Scope — what these numbers do *not* cover

- **Only direct `tb_product` mutations.** Cascades from joined dimension/aggregate
  tables are out of scope: they propagate only when the parent is itself a
  registered `tv_` entity exposing its `fk_<parent>` key, which this
  single-entity benchmark does not model. The cascade contract is covered by
  [`test/sql/42_cascade_fk_lineage.sql`](../../test/sql/42_cascade_fk_lineage.sql).
- **Non-concurrent baseline.** The baseline is plain `REFRESH MATERIALIZED VIEW`.
  `REFRESH ... CONCURRENTLY` is non-blocking but not faster.
- **One machine, one client.** Moderate JSON document size, single connection,
  untuned `shared_buffers`. Your hardware and schema will shift the absolute
  numbers; the *shape* (flat vs linear) is the portable finding.

## Common misreadings

- **"pg_tviews is ~775× faster."** Only for a single-row change at 100K rows.
  It's 8.5× at 1K rows and ~2.8× for a 1%-batch at 100K rows. The multiplier is a
  function of scale and change size — always quote both.
- **"You must install jsonb_delta for the speedup."** No — the incremental
  advantage is pg_tviews' doing; arm B (native, no jsonb_delta) matches arm A on
  this workload.
- **"Incremental always wins."** Not for the initial build, and not by much for
  large batch rewrites. It wins decisively for *point changes on large tables*.

## Reproduce it yourself

The portable conclusion is the flat-vs-linear shape; the absolute numbers depend
on your hardware and schema. Run the harness on your own workload — see
[Running Benchmarks](running-benchmarks.md).

## See also

- [Results](results.md) — the full measured figures
- [Running Benchmarks](running-benchmarks.md) — how to execute the harness
- [Overview](overview.md) — schema, operations, and methodology
