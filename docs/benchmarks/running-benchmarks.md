# Running Benchmarks

This guide explains how to run the pg_tviews performance benchmark and read its
output. The benchmark lives in
[`test/sql/real_benchmark/`](../../test/sql/real_benchmark/README.md) and drives
the extension through its **real** `pg_tviews_create` API — the same call path a
production tview uses. Published numbers from a measured run live in
[results.md](results.md).

## Table of Contents

- [What the benchmark does](#what-the-benchmark-does)
- [Prerequisites](#prerequisites)
- [Quick start](#quick-start)
- [Options](#options)
- [How a run works](#how-a-run-works)
- [Reading the results](#reading-the-results)
- [Reproducing on a clean box](#reproducing-on-a-clean-box)
- [Troubleshooting](#troubleshooting)
- [See also](#see-also)

---

## What the benchmark does

The harness maintains a denormalised product catalogue — one JSONB row per
product joining category, supplier, inventory, and a review aggregate — three
ways, and times `tb_product` mutations against each:

| Arm | Approach | Maintenance on a base-table change |
|-----|----------|------------------------------------|
| **A** — pg_tviews + jsonb_delta | incremental refresh, surgical JSONB patch | refresh only the affected rows |
| **B** — pg_tviews + native | incremental refresh, no jsonb_delta (fallback) | refresh only the affected rows |
| **C** — full refresh | `REFRESH MATERIALIZED VIEW` | rebuild every row |

Each arm runs in its own database, seeded from a common template so the base
data is identical. Every run is **gated on correctness**: `tv_product` is
compared row-for-row against its backing view `v_product`, and any divergence
fails the run before a single timing is trusted.

See [overview.md](overview.md) for the schema, the operations measured, and the
measurement methodology.

---

## Prerequisites

- **PostgreSQL 18** cluster with `pg_tviews` in `shared_preload_libraries`.
  A `cargo pgrx` cluster (default port **28818**) is the reference setup; a
  system PostgreSQL 18 works too.
- **pg_tviews** installed into that cluster.
- **jsonb_delta 0.3.0** installed into that cluster (needed only for arm A;
  arms B and C run without it).
- **A superuser / `CREATEDB` role** — the harness creates and drops several
  scratch databases per run.
- **`psql`, `bash`, and `python3`** on `PATH` (`aggregate.py` needs python3).

Install both extensions into a local pgrx cluster (always pass `--pg-config`):

```bash
PGC="$HOME/.pgrx/18.4/pgrx-install/bin/pg_config"   # adjust to your cluster

# pg_tviews (from this repo)
cargo pgrx install --release --no-default-features --features pg18 --pg-config "$PGC"

# jsonb_delta 0.3.0 (sibling checkout)
cd ../jsonb_delta && cargo pgrx install --release --pg-config "$PGC"
```

`pg_tviews` must be in `shared_preload_libraries` (its GUCs and hooks are
registered at postmaster start). Add it to the cluster's `postgresql.conf`
(for a pgrx cluster, `~/.pgrx/data-18/postgresql.conf`) and restart:

```conf
shared_preload_libraries = 'pg_tviews'
```

---

## Quick start

```bash
cd test/sql/real_benchmark
PGHOST=localhost PGPORT=28818 PGUSER=postgres ./run.sh --scales "small medium large"
```

`PGHOST`/`PGPORT`/`PGUSER` default to `localhost` / `28818` / `postgres`, so on
the reference pgrx cluster you can just run `./run.sh --scales "small"`.

Scales:

| Scale | Categories | Suppliers | Products | Reviews |
|-------|-----------:|----------:|---------:|--------:|
| small  | 20  | 10  | 1,000   | 5,000   |
| medium | 50  | 30  | 10,000  | 50,000  |
| large  | 100 | 100 | 100,000 | 500,000 |

Start with `small` to validate the setup end-to-end (it finishes quickly); add
`medium` and `large` once it passes. At large scale each of the three arms
materialises 100K products / 500K reviews, so the run takes correspondingly
longer — expect the full `REFRESH` arm to dominate wall-clock time, which is the
cost the comparison is measuring.

---

## Options

`run.sh` accepts:

| Flag | Default | Meaning |
|------|---------|---------|
| `--scales "<list>"` | `small` | Space-separated scales to run (`small`, `medium`, `large`). |
| `--single-iters N` | `25` | Iterations for the single-row `UPDATE` op on arms A/B. |
| `--c-iters N` | `5` | Iterations per op for arm C (each is a full `REFRESH`). |
| `--help` | — | Print the script header and exit. |

Insert/delete ops use 10 iterations and batch ops use 5 (fixed in the script).
Medians are reported, so a handful of iterations is enough to be stable; raise
`--single-iters` if you want tighter medians on a noisy machine.

---

## How a run works

For each scale, `run.sh`:

1. Builds a template database `bench_rb_data`, loads `schema.sql`, and generates
   data with `gen_data.sql` (parameterised by the scale's row counts).
2. Clones the template into one database per arm:
   - **Arm A** — `CREATE EXTENSION jsonb_delta; CREATE EXTENSION pg_tviews;`
   - **Arm B** — `CREATE EXTENSION pg_tviews;`
   - **Arm C** — no extensions.
3. Arms A/B create the tview with
   `SELECT pg_tviews_create('tv_product', <product_select.sql>)`; arm C creates a
   `MATERIALIZED VIEW mv_product` with a unique index on `pk_product`.
4. Runs the same sequence of `tb_product` mutations under `psql \timing`:
   `update_single`, `update_batch` (1% of rows), `insert_single`,
   `delete_single`, plus the one-time `build`. For arm C each timed statement is
   the `REFRESH MATERIALIZED VIEW` that the change forces.
5. Checks the correctness gate. Arms A/B emit `RB_OK divergence=0` on success; a
   non-zero divergence emits `RB_DIVERGENCE <n>` and **fails the run**.
6. Parses each `\timing` log, appending `scale · arm · op · ms` rows to
   `results/raw.tsv`, and drops the scratch databases.

After all scales, `aggregate.py` prints a per-operation median table and writes
`results/summary.tsv`.

Timing is `psql \timing` on autocommit statements, so every figure is the
end-to-end, client-observed cost **including** the post-statement refresh flush
— not an internal micro-timing.

---

## Reading the results

Everything lands in `test/sql/real_benchmark/results/`:

| File | Contents |
|------|----------|
| `raw.tsv` | One row per measured statement: `scale⇥arm⇥op⇥ms`. |
| `summary.tsv` | Per `(scale, arm, op)` stats: `n`, `min_ms`, `median_ms`, `mean_ms`. Arms are named `pg_tviews+jsonb_delta`, `pg_tviews+native`, `full_refresh_matview`. |
| `<scale>_<arm>.log` | The raw `psql \timing` transcript for that arm (`small_a.log`, `large_c.log`, …). Grep these for `RB_OK` / `RB_DIVERGENCE`. |

`aggregate.py` also prints a comparison table to stdout, e.g.:

```
── small ────────────────────────────────────────
  op               A median   B median   C median    A vs C   A vs B
  build              59.800     60.200     33.800      0.6x    1.01x
  update_single       1.770      1.670     15.000      8.5x    0.94x
  update_batch        5.120      4.970     15.200      3.0x    0.97x
  insert_single       1.830      1.670     14.900      8.2x    0.91x
  delete_single       1.150      1.140     15.200     13.3x    0.99x
```

- **`A vs C`** = `full_refresh ÷ pg_tviews+jsonb_delta` — the incremental speedup.
- **`A vs B`** = `pg_tviews+native ÷ pg_tviews+jsonb_delta` — jsonb_delta's effect
  (≈1.0 means parity; see [jsonb-ivm-integration.md](jsonb-ivm-integration.md)).

For how to interpret these — flat vs linear, point vs batch, the one-time build
cost — see [results-interpretation.md](results-interpretation.md). The published
figures are in [results.md](results.md).

---

## Reproducing on a clean box

`test/sql/real_benchmark/provision-ubuntu.sh` provisions a fresh Ubuntu 24.04
host (run as root) with PostgreSQL 18 (PGDG), pgrx 0.17.0, pg_tviews, and
jsonb_delta 0.3.0 — the exact stack behind [results.md](results.md). Sync this
working tree to `/root/pg_tviews`, then:

```bash
sudo test/sql/real_benchmark/provision-ubuntu.sh
sudo -u postgres bash -c 'cd /root/pg_tviews/test/sql/real_benchmark && \
  PGUSER=postgres ./run.sh --scales "small medium large"'
```

---

## Troubleshooting

**`CREATE EXTENSION pg_tviews` fails / GUCs missing.** `pg_tviews` must be in
`shared_preload_libraries` and the cluster restarted. Verify with
`SHOW shared_preload_libraries;`.

**Arm A can't find jsonb_delta.** Install jsonb_delta 0.3.0 into the same
cluster (`cargo pgrx install --pg-config "$PGC"` in the jsonb_delta checkout) and
confirm `SELECT default_version FROM pg_available_extensions WHERE name = 'jsonb_delta';`
returns `0.3.0`. Arms B and C do not need it.

**`permission denied to create database`.** The connecting role must be a
superuser or hold `CREATEDB`; the harness creates and drops scratch databases.

**`RB_DIVERGENCE <n>` in a log.** The tview did not match its backing view — a
correctness failure, not a timing issue. Inspect the offending arm's
`<scale>_<arm>.log` and the divergent rows:

```sql
SELECT pk_product, t.data, v.data
FROM tv_product t FULL JOIN v_product v USING (pk_product)
WHERE t.data IS DISTINCT FROM v.data;
```

**`aggregate.py` not found / python missing.** Install `python3`; the raw
timings are still in `results/raw.tsv` even if aggregation fails.

---

## See also

- [Benchmark Overview](overview.md) — schema, operations, and methodology
- [Results](results.md) — published figures from a measured run
- [Results Interpretation](results-interpretation.md) — how to read the numbers
- [jsonb_delta Integration](jsonb-ivm-integration.md) — jsonb_delta's role and the parity finding
- [`test/sql/real_benchmark/README.md`](../../test/sql/real_benchmark/README.md) — harness details
