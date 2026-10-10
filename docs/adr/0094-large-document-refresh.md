# ADR 0094: Large-document refresh (TOAST/WAL cost of one-field changes)

- Status: Accepted (no code change; guidance only). The `pg_tviews_profile()` follow-up shipped: it
  warns when TOAST holds over 30% of a TVIEW and points here.
- Issue: #94
- Harness: `test/sql/real_benchmark/scenarios/toast_knobs.sh` (built on #83)
- Data: `test/sql/real_benchmark/results/toast/{random,text,unlogged}/physical.csv`

## Context

When one small field of a large `data` document changes, the refresh writes a whole new
`data` value. Above the ~2 KB TOAST threshold that is a new TOAST value (new chunks, plus
their index entries and WAL), even though the change is a few bytes. The #69 baseline
(`payload_sweep.sh`, LOGGED) measured, per 200 one-integer refreshes:

| `data` size | TOAST growth | WAL per refresh |
|---:|---:|---:|
| 4 KB | 1.05 MB | 17.3 kB |
| 16 KB | 3.26 MB | 46.6 kB |
| 64 KB | 13.03 MB | 148.6 kB |

Question: is there a storage knob, a schema pattern, or code in pg_tviews / jsonb_delta that
reduces this, and is it worth shipping?

## Method

`toast_knobs.sh`: 2000 rows, `tv_doc.data = {counter, payload}`, 200 single-row autocommit
updates of `counter` on the base table, `VACUUM FULL tv_doc` after the knob is applied.
Counters come from `bench.physical` (load-independent). PostgreSQL 18.1 (pgrx build),
LOGGED unless noted, pg_tviews built from `main` + local branches, so **HOT defaults
(#70/#73, PR #87) are not in these numbers** (`n_tup_hot_upd` = 0 everywhere).

Two payloads: `random` (md5 hex, incompressible: worst case) and `text` (repetitive JSON-ish
text, compressible: closer to real documents). Knobs: default, `SET STORAGE MAIN`,
`toast_tuple_target = 8160`.

`default_toast_compression = lz4` / `SET COMPRESSION lz4` could not be measured: the pgrx
PostgreSQL build has no lz4 (`compression method lz4 not supported`). Read cost was not
measured. Both are open items below.

## Results (`tv_doc`, 200 refreshes, LOGGED)

WAL is per refresh, for the whole transaction (base-table update + refresh).

| payload | size | knob | TOAST growth | WAL / refresh |
|---|---:|---|---:|---:|
| random | 4 KB | default | +1.05 MB | 17.1 kB |
| random | 4 KB | MAIN | 0 (inline) | 11.1 kB |
| random | 4 KB | tuple_target 8160 | 0 (inline) | 11.2 kB |
| random | 16 KB | default / MAIN / tuple_target | +3.2 MB | 49.5 kB (all three) |
| random | 64 KB | default / MAIN / tuple_target | +13.1 MB | 148.6 kB (all three) |
| text | 4 / 16 / 64 KB | default | 0 (compressed inline) | 10.3 / 15.2 / 19.8 kB |
| text | 4 KB | tuple_target 8160 | 0 | 13.7 kB |
| text | 16 / 64 KB | MAIN | 0 | 15.2 / 21.0 kB |
| text, UNLOGGED (shipped default) | 4 / 16 / 64 KB | default | 0 | 4.7 / 7.2 / 9.7 kB |

Findings:

1. **Knobs only help while the stored value fits in a page.** `MAIN` and a raised
   `toast_tuple_target` keep a ~4 KB value inline and cut WAL by about a third (17.1 → 11.1
   kB) and remove TOAST growth, at the price of a much larger heap (17.6 MB vs 0.2 MB heap +
   11 MB TOAST for 2000 rows, so no net space win). At 16 KB and 64 KB they change nothing:
   PostgreSQL cannot inline them, and cost keeps scaling linearly with document size.
2. **Compressible documents are mostly not affected.** pglz compresses realistic JSON below
   the threshold; none of the `text` runs touched TOAST. The cost is a worst-case,
   incompressible-payload problem (embedded base64, hashes, already-compressed blobs).
3. **UNLOGGED (the shipped default) removes the WAL half.** The remaining cost there is
   TOAST bloat and vacuum work, not WAL.
4. **Every refresh rewrites the value regardless of knob.** No knob makes the update
   proportional to the change.

## Decision

**No-go on code.** No change to pg_tviews or jsonb_delta will make the TOAST cost of a
one-field change proportional to the change size:

- PostgreSQL has no partial update of a TOASTed `jsonb`. TOAST values are immutable; a
  changed datum is written as a new value. Slice fetch exists for reads of some external
  types, not for updates. Delivering this needs a core PostgreSQL change (an in-place or
  delta-chunk TOAST update), which is out of scope for the extension.
- The one mechanism that avoids the rewrite is not modifying the column at all: an
  unchanged TOAST pointer is reused. That is a schema decision, not a refresh-engine one.

### Guidance to document (user-facing, follow-up docs change)

1. **Split rarely-changing large sub-documents** (a post's `content`, a product's
   `description`) out of the hot TVIEW: give them their own TVIEW (1:1 on the same pk) and
   reference/compose them at read time. The hot TVIEW's `data` then stays small and inline.
2. **Leave storage knobs alone** unless documents are consistently 2–8 KB *and*
   incompressible *and* the TVIEW is LOGGED: then `ALTER COLUMN data SET STORAGE MAIN` cuts
   WAL by about a third. Beyond one page it does nothing.
3. **Keep TVIEWs UNLOGGED** (the default) when WAL volume matters.

## Consequences / follow-ups (not filed, need maintainer go)

- `pg_tviews_profile()` (#74): flag "large `data` (> 2 KB stored), small hot field" from the
  direct-patch capture stats and point at guidance 1. Cheap; the stats exist.
- Docs: add the guidance above to the performance guide.
- Re-measure with lz4 (needs a PG build with lz4) and measure read cost of `MAIN`.
- Re-run with #87's HOT defaults merged: HOT removes the heap-side cost only; the TOAST
  value is new either way, so the conclusion should not change.
- Interaction with #78 (field dependency classes): "hot" vs "cold" fields is the same
  classification; a per-field split recommendation could ride on that ADR.
