# jsonb_delta Integration

How pg_tviews uses the [jsonb_delta](https://github.com/evoludigit/jsonb_delta)
extension, and what the benchmark says its value is. (The extension was formerly
called `jsonb_ivm`; this page keeps the historical filename.)

**pg_tviews**: 0.1.0 • **jsonb_delta**: 0.3.0 • **PostgreSQL**: 18.4 •
**Last updated**: 2026-07-22

## What jsonb_delta is

jsonb_delta is a separate Rust/pgrx extension providing surgical JSONB patch
functions — it rewrites only the changed path of a JSONB document instead of
replacing the whole value. pg_tviews uses it as an **optional** accelerator on
the refresh hot path. It is not required: without it, pg_tviews falls back to a
native path built on standard PostgreSQL JSONB functions.

## How pg_tviews uses it

At refresh time pg_tviews computes each affected tview row's desired JSON and
writes the difference into the stored document. When jsonb_delta is installed,
that write goes through its presence-detected scalar patch function
(`jsonb_smart_patch_scalar`); otherwise the native fallback does the same job
with in-core JSONB operations.

The runtime coupling is deliberately narrow: **the only jsonb_delta function
pg_tviews calls on the hot path is the scalar patch**. That keeps the dependency
surface small and the fallback faithful — arm A and arm B produce byte-identical
tview rows (both are gated on the same row-for-row divergence check against the
backing view).

## What the benchmark shows: parity

The [real benchmark](running-benchmarks.md) runs two pg_tviews arms:

- **Arm A** — pg_tviews **with** jsonb_delta (surgical scalar patch)
- **Arm B** — pg_tviews **without** it (native fallback)

For the product-catalogue workload, the two arms are **at parity**
(A-vs-B ≈ 0.92–1.05× across all scales and operations — see
[results.md](results.md)). For these moderately-sized JSON documents, the
surgical scalar patch and the native fallback are statistically
indistinguishable.

This is an honest, measured result, not a disappointment: jsonb_delta's win
shows up on **large nested / array-valued documents**, which this single-JSONB-per-product
catalogue does not stress. Its value here is having a stable, presence-detected
patch path — not a headline speed-up on this workload. Install it, but do not
expect it to move these numbers.

## Where jsonb_delta *would* help: arrays

The one place a surgical path could beat full recompute is a tview row carrying a
large child **array** (e.g. a `comments: [...]` array rebuilt from child rows on
every cascade). That was measured directly in
[array-fastpath-go-no-go.md](array-fastpath-go-no-go.md):

- At the **function level**, surgical array patching is 3–5× faster than
  rebuilding the array.
- **End-to-end**, the O(document) physical write (detoast + rewrite + WAL, which
  the surgical path also pays) erodes that to a ~2× constant factor for arrays of
  ≥ ~1000 elements — and to a **regression** below ~100 elements.

The conclusion there is **NO-GO for now**: the payoff only materialises for
array-heavy tview rows that no profiled pg_tviews workload currently exhibits, so
the refresh path keeps full-array recompute as its default. See that page for the
GO trigger that would justify the queue rework.

## Installing jsonb_delta

Install version **0.3.0** into the same cluster as pg_tviews (always pass
`--pg-config`):

```bash
git clone --depth 1 --branch v0.3.0 https://github.com/evoludigit/jsonb_delta.git
cd jsonb_delta
cargo pgrx install --release --pg-config "$(which pg_config)"
```

Then, in the target database:

```sql
CREATE EXTENSION jsonb_delta;   -- before CREATE EXTENSION pg_tviews
SELECT default_version FROM pg_available_extensions WHERE name = 'jsonb_delta';
-- expect: 0.3.0
```

pg_tviews detects jsonb_delta at refresh time; no configuration is needed. If it
is absent, the native fallback is used automatically.

## See also

- [Running Benchmarks](running-benchmarks.md) — how to run the A/B/C comparison
- [Results](results.md) — the measured parity figures
- [Array fast-path GO/NO-GO](array-fastpath-go-no-go.md) — the array-patching analysis
- [`test/sql/real_benchmark/README.md`](../../test/sql/real_benchmark/README.md) — harness details
