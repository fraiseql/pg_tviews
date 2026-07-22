# Performance Benchmarks Overview

Methodology and scenarios for the pg_tviews performance benchmark. The harness
lives in [`test/sql/real_benchmark/`](../../test/sql/real_benchmark/README.md)
and drives the extension through its real `pg_tviews_create` API. Published
figures from a measured run are in [results.md](results.md).

**pg_tviews**: 0.1.0 (pgrx 0.17.0) • **jsonb_delta**: 0.3.0 •
**PostgreSQL**: 18.4 • **Last updated**: 2026-07-22

## Quick links

- **[Running Benchmarks](running-benchmarks.md)** — how to run the harness and read its output
- **[Results](results.md)** — published figures from a measured run
- **[Results Interpretation](results-interpretation.md)** — how to read the numbers honestly
- **[jsonb_delta Integration](jsonb-ivm-integration.md)** — jsonb_delta's role and the parity finding
- **[Array fast-path GO/NO-GO](array-fastpath-go-no-go.md)** — surgical array patching: measured, deferred

## What is measured

A denormalised product catalogue — one JSONB `data` row per product joining
category, supplier, inventory, and a review aggregate — maintained three ways,
timing `tb_product` mutations against each:

| Arm | Approach | Maintenance on a base-table change |
|-----|----------|------------------------------------|
| **A** — pg_tviews + jsonb_delta | incremental refresh, surgical JSONB patch | refresh only the affected rows |
| **B** — pg_tviews + native | incremental refresh, no jsonb_delta (fallback) | refresh only the affected rows |
| **C** — full refresh | `REFRESH MATERIALIZED VIEW` | rebuild every row |

Arms A and B use the extension's real refresh machinery; arm C is a plain
PostgreSQL materialized view and serves as the O(n) baseline. This is a
**single-entity** benchmark: it measures the cost of maintaining one tview under
direct changes to its own base table.

## Test schema

The base schema (`schema.sql`) follows the extension's Trinity naming — every
entity has `id` (UUID) + `pk_<entity>` + `fk_<parent>`:

```
tb_category   (pk_category, name, slug)
tb_supplier   (pk_supplier, name, country)
tb_product    (pk_product, fk_category, fk_supplier, sku, name,
               base_price, current_price, currency, status)   ← the timed entity
tb_inventory  (pk_inventory, fk_product, quantity, reserved, warehouse_location)
tb_review     (pk_review, fk_product, fk_user, rating, verified_purchase, …)
```

The tview definition (`product_select.sql`) builds one JSONB document per
product: price (with a computed discount %), category, supplier, an inventory
sub-object, and a review aggregate (`count`, `avg_rating`, `verified_count`).
Arms A/B register it with:

```sql
SELECT pg_tviews_create('tv_product', $$ <product_select.sql> $$);
```

`pg_tviews_create` materialises `tv_product` and its backing view `v_product`,
and installs the triggers that keep `tv_product` incrementally in sync. Arm C
creates a `MATERIALIZED VIEW mv_product` from the same SELECT with a unique index
on `pk_product`.

## Scales

Each arm is seeded from a common template database so the base data is identical
across arms:

| Scale | Categories | Suppliers | Products | Reviews |
|-------|-----------:|----------:|---------:|--------:|
| small  | 20  | 10  | 1,000   | 5,000   |
| medium | 50  | 30  | 10,000  | 50,000  |
| large  | 100 | 100 | 100,000 | 500,000 |

## Operations timed

Only `tb_product` mutations are timed — the operation pg_tviews refreshes
incrementally **and** correctly:

| Operation | What it does | Iterations (A/B · C) |
|-----------|--------------|:--------------------:|
| `build` | one-time tview / matview creation and initial materialisation | 1 · 1 |
| `update_single` | single-row `UPDATE` on a scattered `pk_product` | 25 · 5 |
| `update_batch` | `UPDATE` over a 1%-of-table `pk_product` window | 5 · 5 |
| `insert_single` | `INSERT` one new product | 10 · 5 |
| `delete_single` | `DELETE` one product (the one just inserted) | 10 · 5 |

For arms A/B the timed statement is the base-table mutation itself (the tview
refresh happens in the post-statement flush, which the timing includes). For arm
C the timed statement is the `REFRESH MATERIALIZED VIEW` that each change forces.

## Measurement methodology

- **Clock**: `psql \timing` on **autocommit** statements. Each figure is the
  end-to-end, client-observed cost, **including** the post-statement refresh
  flush for arms A/B. This is deliberately not an internal micro-timing — it is
  what an application actually waits for.
- **Statistic**: the **median** of the per-op iterations above. Medians are
  reported because they are robust to the occasional autovacuum/checkpoint blip.
- **Correctness gate**: before any timing is trusted, `tv_product` is compared
  row-for-row against `v_product`
  (`WHERE t.data IS DISTINCT FROM v.data`). A non-zero divergence fails the run.
- **Isolation**: each arm runs in its own database, cloned from a shared
  template; scratch databases are dropped between arms.
- **Baseline**: arm C is a **non-concurrent** `REFRESH MATERIALIZED VIEW` (a full
  O(n) rebuild). `REFRESH ... CONCURRENTLY` trades a unique-index requirement and
  a full diff for non-blocking behaviour; it is not faster.

## Headline result

pg_tviews turns O(n) view maintenance into O(1) for point changes. A single-row
`UPDATE` costs a flat **~1.8–2.0 ms regardless of dataset size**, while a full
`REFRESH` grows linearly with the table:

| Scale | products / reviews | pg_tviews (ms) | full refresh (ms) | speedup |
|-------|--------------------|---------------:|------------------:|--------:|
| small  | 1K / 5K     | 1.77 | 15.0    | **8.5×** |
| medium | 10K / 50K   | 1.87 | 167.9   | **90×** |
| large  | 100K / 500K | 1.97 | 1,523.5 | **775×** |

(Medians, single-row `UPDATE`, from [results.md](results.md).) The speedup is the
ratio of the two columns and grows without bound as the table grows, because the
incremental cost stays flat while the full rebuild tracks the row count. Per-op
tables for every scale, and the honest caveats below, are in
[results.md](results.md) and [results-interpretation.md](results-interpretation.md).

## Scope and limitations

- **Direct entity mutations only.** These figures cover changes to the tview's
  own base table (`tb_product`). Cascades from the joined dimension/aggregate
  tables (category, supplier, inventory, reviews) only propagate when the parent
  is itself a registered `tv_` entity exposing its `fk_<parent>` key; this
  single-entity benchmark embeds that data inline and does **not** exercise
  cascade propagation. The cascade contract is covered by
  [`test/sql/42_cascade_fk_lineage.sql`](../../test/sql/42_cascade_fk_lineage.sql).
- **jsonb_delta vs native is at parity here.** For these moderately-sized JSON
  documents, arms A and B are statistically indistinguishable. See
  [jsonb-ivm-integration.md](jsonb-ivm-integration.md).
- **Point changes win; batch changes win less.** A batch touching 1% of rows
  still refreshes 1% of the tview, so the advantage narrows with batch size.
- **Initial build is a one-time cost pg_tviews loses.** Materialising every row
  through the refresh machinery is slower than a single
  `CREATE MATERIALIZED VIEW`; it is paid once.
- Moderate JSON document size; single client; non-concurrent full-refresh
  baseline.

## See also

- [Running Benchmarks](running-benchmarks.md) — execute the harness
- [Results](results.md) — published performance data
- [Results Interpretation](results-interpretation.md) — reading the numbers
- [`test/sql/real_benchmark/README.md`](../../test/sql/real_benchmark/README.md) — harness details
