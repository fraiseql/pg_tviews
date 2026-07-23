# Issue #56 — `scalar_cascade_fanout` results

Single-field author update (`UPDATE tb_user SET bio = …`) with a wide nested-parent
fan-out (1 author → N posts, each embedding `author` as a nested object). The
direct-patch fast path OFF (recompute baseline) vs ON.

Harness: `test/sql/real_benchmark/scalar_cascade_fanout.sh` — a server-side plpgsql
loop (one statement per iteration, so the per-statement flush still fires),
counters read in the same session.

Machine: local dev, PostgreSQL 18.1 (pgrx cluster, port 28818), debug build.

| fan-out | iters | config | writes/s | view recomputes | patches applied |
|---:|---:|---|---:|---:|---:|
| 60  | 2000 | GUC off | 35 | 122,000 | 0 |
| 60  | 2000 | GUC on  | 48 | **0** | 122,000 |
| 300 | 800  | GUC off | 16 | 240,800 | 0 |
| 300 | 800  | GUC on  | 21 | **0** | 240,800 |

**Speedup (on/off): ~1.3–1.4×.** The decisive result is the recompute count:
`122,000 → 0` (and `240,800 → 0`) — the fast path issues **zero backing-view
queries**, counter-proven, while the baseline recomputes every fan-out row.

## Why the local wall-clock ratio is modest (not the issue's ~76×)

The issue's ~89 → thousands figure was measured against a **per-row** recompute
baseline. Since then, issue #48 gave the recompute path a **bulk** upsert
(`refresh_bulk`: one `INSERT … SELECT … ON CONFLICT` for the whole fan-out), so
today's baseline is already efficient. The direct patch replaces that one bulk
`INSERT…SELECT` (JOIN + `jsonb_build_object` per row) with one grouped
`UPDATE … jsonb_smart_patch_nested` (path-merge per row) — both O(fan-out), so the
gain is the per-row constant (JOIN + full-document rebuild eliminated), plus the
fixed per-update flush overhead (trigger, topological sort, parent discovery) that
both configs pay equally and which dilutes the ratio at this scale.

The wall-clock win grows with **document/JOIN complexity** (heavier
`jsonb_build_object` + more joined dimensions ⇒ recompute gets more expensive while
the patch stays a path-merge) and under write contention (fewer/no reads of the
backing relations). No performance bug was found: the fast path does one grouped
UPDATE per entity, loads metadata once per entity (cached), and adds no per-key SPI.
