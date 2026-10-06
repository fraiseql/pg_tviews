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
check of `tv_product` against its backing view (`tviews.public__tv_product`); a non-zero
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
medium = 10K / 50K, large = 100K / 500K. Each run writes into its own
`results/physical/<timestamp>/` (override with `RUN_DIR`): `raw.tsv`,
`summary.tsv`, per-arm `\timing` logs, and the physical files below.
`aggregate.py timing` prints the comparison table and writes `summary.tsv`.
Arms A/B run once per TVIEW persistence mode (`--modes "unlogged logged"`).

`docs/benchmarks/results.md` is generated from a run of this harness (the
committed `results/raw.tsv` / `summary.tsv` back it).

## Physical cost

Latency hides most of what a refresh costs: new tuple versions, index
insertions, WAL, dead tuples and visibility-map churn. Every scenario therefore
also records physical counters per step and per relation.

```bash
./run_physical.sh --scales "small medium large"   # everything, one run dir
./scenarios/skewed_fanout.sh --dry-run            # any scenario: row-count sanity check
./selftest.sh                                     # harness self-checks
```

| Scenario | What it isolates |
|---|---|
| `run.sh` (product) | the arms above, per op group |
| `scenarios/payload_sweep.sh` | one-field change on a 100 B .. 64 KB `data` document (TOAST) |
| `scenarios/skewed_fanout.sh` | two-hop user -> post -> comment cascade with celebrity users; 1M rows at `SCALE=1`, 10M at `SCALE=10` |
| `scenarios/noop.sh` | refreshes whose output is unchanged, including one key reached via three paths |

Every scenario runs in both `unlogged` (the shipped `pg_tviews.unlogged_by_default`)
and `logged` mode (`MODES`), gates on a row-for-row `tv_*` vs backing-view
divergence check, and appends to the run directory:

| File | Content |
|---|---|
| `physical.csv` | `bench.physical` rows: one per (scenario, mode, step, relation) |
| `fanout.csv` | dependents per parent key (p50/p95/p99/max) per cascade edge |
| `explain/<scenario>_<mode>/summary.tsv` | flush statements of one representative refresh |
| `env.tsv` | machine, server settings, extension versions, `pg_tviews.*` GUCs, commit |
| `report.md` | all of the above as markdown (`aggregate.py physical RUN_DIR`) |

### What the physical columns mean

Snapshots come from `lib/stats.sql` (`bench.snapshot`, `bench.delta`,
`bench.physical`); a step is the interval between two snapshots.

| Column | Source | Meaning |
|---|---|---|
| `n_tup_upd`, `n_tup_hot_upd`, `hot_pct` | `pg_stat_all_tables` | tuple updates, and the share that were HOT (no index insertions, prunable in-page) |
| `n_tup_newpage_upd` | `pg_stat_all_tables` | updates whose new version landed on another page (no free space / fillfactor) |
| `n_dead_tup_after` | `pg_stat_all_tables` | dead tuples left for VACUUM after the step (autovacuum is off during steps) |
| `seq_scan`, `idx_scan` | `pg_stat_all_tables` | scans of the relation during the step; propagation seq-scans show up here |
| `heap/idx/toast_blks_hit/read` | `pg_statio_all_tables` | buffer hits and reads per relation |
| `heap/index/toast_bytes_before/after` | `pg_relation_size`, `pg_indexes_size`, TOAST relation | bloat and TOAST growth |
| `all_visible_frac_after` | `pg_visibility_map_summary` | share of heap pages still all-visible (index-only scans, cheap VACUUM); NULL without `pg_visibility` |
| `wal_records`, `wal_fpi`, `wal_bytes` | `pg_stat_wal` | **cluster-wide** WAL for the step (base-table writes and background activity included) |
| `io_hits/reads/writes/extends` | `pg_stat_io` (client backends) | cluster-wide relation IO |
| `ops`, `wal_bytes_per_op` | step definition | operations in the step (e.g. refreshes) and WAL per operation |
| `bytes_per_row` | derived | heap + index + TOAST bytes per live tuple |

Method notes:

- Each scenario resets counters (`bench.reset()`), disables autovacuum on its
  tables, runs `VACUUM (ANALYZE)` and a `CHECKPOINT` before its first step, so
  dead-tuple and full-page-image counts start from a known state.
- PG15+ publishes table stats lazily: `pg_stat_force_next_flush()` is issued as
  its own statement before every snapshot.
- `explain/` comes from a **separate** pass with `auto_explain` (ANALYZE, BUFFERS,
  WAL, nested statements) reported as client NOTICEs (`lib/explain_on.sql`).
  auto_explain perturbs timings, so it never runs inside a timed step.
  `shared dirtied/written` are only available there, per statement.
- `auto_explain` and `pg_visibility` are contrib modules; pgrx-managed servers
  don't build contrib, so run `make -C contrib/auto_explain install` (same for
  `pg_visibility`) in the `~/.pgrx/<version>/` source tree once.
