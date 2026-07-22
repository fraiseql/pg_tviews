# Array fast-path: GO/NO-GO benchmark

**Decision: NO-GO for now (defer).** Keep full-array recompute as the default
refresh path. Do **not** start the refresh-queue rework yet. A concrete GO
trigger is defined at the end.

## Context

Joint-design thread [jsonb_delta #25](https://github.com/evoludigit/jsonb_delta/issues/25)
resolved that the only real array speedup available to pg_tviews is a
**pg_tviews-side** change: instead of rebuilding a tview row's whole `comments`
array from the child rows on every cascade (`apply_full_replacement`), track the
single changed element and call jsonb_delta's surgical binary functions
(`jsonb_array_update_where` / `jsonb_array_delete_where`, pointer-passthrough).

Two costs are irreducibly O(array size) and no jsonb_delta function removes them:

1. producing `desired` — our `jsonb_agg` over N child rows;
2. the physical whole-tuple + TOAST rewrite — WAL/storage track document size,
   not delta.

The open question the benchmark answers: **does the surgical function's CPU win
survive once the O(doc) physical write is added?**

## Method

`test/sql/benchmark_array_fastpath.sql`, against **jsonb_delta 0.3.0** (the
pointer-passthrough is a 0.3.0 property). One `tv_post` row whose `data` holds a
`comments` array of N elements matching the issue-#50 shape
(`{"id": <pk>, "body": <~128 incompressible hex chars>}`).

- **full_recompute** — rebuild the whole array from the child rows via
  `jsonb_agg` (what `apply_full_replacement` re-runs today).
- **surgical** — patch the one changed element on the existing doc via
  `jsonb_array_update_where` / `jsonb_array_delete_where`.

Measured at two levels, for both `update` and `delete`, at N ∈ {10, 100, 1000, 5000}:

- **function** — pure expression evaluation, no table write (isolates transform CPU).
- **e2e** — the full `UPDATE` statement, including tuple/TOAST/WAL rewrite.

Bodies are md5-hex (near-incompressible) on purpose: compressible natural-language
text would only shrink the physical write and *flatter* the surgical path, so this
is a conservative stress test. `jit = off` for amortised per-op timing (its compile
cost would hit both methods equally). Timings below are representative of two runs
that agreed within ~5% (PG 18.1, pgrx dev cluster, shared workstation).

## Results (µs per op; `speedup = full ÷ surgical`, >1 means surgical wins)

| N | doc size | op | function (full → surg) | **fn speedup** | e2e (full → surg) | **e2e speedup** |
|---:|---:|:--|---:|:--:|---:|:--:|
| 10 | 1.7 KB | update | 53 → 14 | **3.7×** | 132 → 217 | **0.61×** |
| 100 | 4.5 KB | update | 355 → 119 | **3.0×** | 1051 → 837 | **1.26×** |
| 1000 | 43 KB | update | 3457 → 913 | **3.8×** | 4994 → 2669 | **1.87×** |
| 5000 | 214 KB | update | 17571 → 4341 | **4.0×** | 22743 → 10363 | **2.19×** |
| 10 | 1.7 KB | delete | 53 → 11 | **5.0×** | 519 → 780 | **0.67×** |
| 100 | 4.5 KB | delete | 355 → 115 | **3.1×** | 1148 → 958 | **1.20×** |
| 1000 | 43 KB | delete | 3457 → 909 | **3.8×** | 5012 → 2623 | **1.91×** |
| 5000 | 214 KB | delete | 17571 → 4376 | **4.0×** | 23092 → 10674 | **2.16×** |

## Reading the numbers

- **Function level: surgical is 3–5× faster, everywhere.** This validates (and
  slightly exceeds) jsonb_delta's stated 1.6–3.9×. The CPU cost of rebuilding and
  reserialising the whole array genuinely dominates when you only touch one element.
- **The physical write erodes that win to a ~2× constant factor** — exactly the
  thread's prediction. At N=5000 the 4.0× function win becomes 2.2× e2e; the O(doc)
  write (which surgical *also* pays — it must detoast the old doc, patch, and
  rewrite O(doc)) is added to both methods and compresses the ratio.
- **There is a crossover around N≈100 (docs ≳ 4 KB).** Below it, surgical is
  break-even-to-*worse*: at N=10 it is a **0.6× regression**, because the transform
  CPU is negligible at that size and surgical's fixed per-statement cost (parse
  path, detoast the existing doc, `to_jsonb` the match key) is not amortised by any
  saving. full_recompute at small N never reads the old doc at all.
- **It stays a constant factor, not a complexity-class change.** Both methods are
  O(doc) end-to-end; even at N=5000 the absolute op cost is ~10 ms (surgical) vs
  ~23 ms (full). Nothing here turns an intractable refresh tractable.

## Decision: NO-GO (defer), with a GO trigger

The proposed queue rework is substantial and permanent hot-path complexity
(memory: enrich `RefreshKey` beyond `(entity, pk)` with `(child_key, op)`; read
**both** transition tables on UPDATE to handle re-parenting — today the cascade
uses `new.or_else(old)` at `src/trigger.rs:353`; a size-gated element-op apply
path; and a fallback for aggregated elements that aren't pure child-column
projections). It also carries correctness risk (re-parenting, fallback detection).

Against that cost, the payoff is a **2× constant factor that only materialises for
tview rows carrying arrays of ~1000+ elements**, and is a **regression below
~100**. There is no profiled pg_tviews workload today with that array shape and
element-churn as a measured bottleneck. So the honest call — consistent with the
thread's own "defer until an array-heavy workload justifies it" — is **NO-GO now**.

**GO trigger (revisit when ALL hold):**

1. A real workload has tview rows whose arrays routinely hold **≥ ~1000 elements**, and
2. per-element array churn (not whole-row rebuilds) is a **profiled** refresh
   bottleneck for that workload, and
3. the apply path can be **size-gated** so small arrays keep using full recompute
   (protects the sub-100 case from the measured regression).

When that happens the seam is already there and cheap to exploit: the per-element
delta exists at cascade time (`find_affected_pks_batch` builds child_pk→parent_pk
at `src/propagate.rs:156`; the op is knowable from which transition table is
populated at `src/trigger.rs:353`) — today we discard child_pk and enqueue only
`(entity, parent_pk)`. Expect ~1.8–2.2× on the large-array refresh, no more.

## Reproduce

```bash
# jsonb_delta 0.3.0 must be available to the cluster
createdb bench_array_fastpath
psql -d bench_array_fastpath -f test/sql/benchmark_array_fastpath.sql
```
