# ADR 0077: Refresh strategy selection

- Status: Accepted. Since measured: `pg_tviews_suspend_triggers()` suspends refresh (fixed), and
  `pg_tviews_refresh(entity)` requires owning the TVIEW (0.1.0-beta.27); the recipe below stands.
- Issue: #77
- Harness: `test/sql/real_benchmark/scenarios/refresh_strategy.sh`

## Context

A flush brings every changed key of an entity up to date with one statement shape:
`INSERT … SELECT … FROM v_<entity> WHERE pk = ANY($1) ON CONFLICT … DO UPDATE`, chunked
by `pg_tviews.batch_size` (1000). #77 asked whether another strategy should be chosen
per flush, from the key count and the number of heap blocks the keys touch:

| id | strategy |
|---|---|
| S1 | today: `= ANY($1)` upsert, batches of 100 / 1000 / 10 000 / all |
| S2 | the same upsert with `JOIN unnest($1)` instead of `= ANY($1)` |
| S3 | UNLOGGED staging table, then `MERGE` |
| S4 | full rebuild: `TRUNCATE` + `INSERT … SELECT` from the view |

## Method

`refresh_strategy.sh` builds a TVIEW-shaped table on plain SQL: 200 000 rows of about
1 KB (`jsonb` with an md5 payload), fillfactor 85, the #72 `IS DISTINCT FROM` guard. Each
trial first changes the N keys in the base table, then times one strategy; the median of
3 trials is reported. Keys are clustered (contiguous pks) or scattered (random pks);
`blocks` is the number of distinct heap blocks the keys occupy. PostgreSQL 18.1,
`shared_buffers` 128 MB but a 94 GB machine, so the whole table stayed in the OS cache:
**these are in-memory numbers**. Load average 0.8 → 2.5 during the run (other sessions),
so differences under ~20% are noise.

## Results (ms)

UNLOGGED:

| layout | N | blocks | S1/b100 | S1/b1000 | S1/b10k | S1/all | S2 | S3 | S4 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| clustered | 10 | 3 | 0.7 | 0.5 | 0.7 | 0.7 | 0.6 | 1.0 | 754 |
| clustered | 1 000 | 169 | 19.9 | 22.6 | 22.6 | 22.4 | 22.1 | 12.3 | 755 |
| clustered | 10 000 | 1 669 | 196 | 225 | 226 | 227 | 237 | 207 | 766 |
| clustered | 100 000 | 16 669 | 2 416 | 2 563 | 2 593 | 2 509 | 2 604 | 1 758 | **912** |
| scattered | 10 | 10 | 0.3 | 0.3 | 0.3 | 0.3 | 0.3 | 0.6 | 934 |
| scattered | 1 000 | 994 | 20.6 | 20.4 | 20.1 | 20.3 | 20.5 | 69.1 | 921 |
| scattered | 10 000 | 8 836 | 285 | 272 | 254 | 254 | 265 | 277 | 949 |
| scattered | 100 000 | 32 825 | 3 474 | 3 317 | 2 951 | 2 320 | 3 101 | 2 140 | **1 023** |

LOGGED follows the same shape, 5–20% slower (S4: 949–1 162 ms; S1 at 100 000 keys:
2 664–3 720 ms). Raw TSV: `test/sql/real_benchmark/results/refresh_strategy/`.

## Findings

1. **Per-key refresh is linear, about 0.02–0.035 ms per key**, whatever the layout.
   **A full rebuild is flat, about 0.75–1.1 s** for 200 000 rows. The crossover is near
   **30 000–35 000 keys, 15–18% of the table**.
2. **Block spread barely matters in memory.** Scattered keys touch 5× more blocks than
   clustered ones at 10 000 keys and cost 15–25% more. `pg_stats.correlation` is therefore
   not worth a predictor today. On a table larger than memory the spread would matter
   more; that was not measured.
3. **Batch size is not a performance knob below 10 000 keys.** At 100 000 scattered keys,
   one statement beats batches of 100 (2.3 s vs 3.5 s): fewer statements, same work.
   `batch_size` stays what it is, a bound on statement size.
4. **`JOIN unnest` (S2) is never better than `= ANY` (S1).**
5. **Staging + MERGE (S3)** wins only at 100 000 keys, where the full rebuild wins by
   more, and it is erratic mid-range (69–122 ms vs 20 ms at 1 000 scattered keys).

## Decision

**No automatic strategy switch.** The only strategy that beats per-key refresh is the
full rebuild, and its advantage comes from `TRUNCATE`: an ACCESS EXCLUSIVE lock that blocks
every reader of the TVIEW until the transaction ends. A flush cannot take that lock on its
own initiative. A non-blocking full recompute (upsert of every row) costs what per-key
refresh costs at N = all rows, so it gains nothing.

S2 and S3 are rejected: no consistent gain. `batch_size` is unchanged.

Instead, the choice goes to the caller, who knows whether the lock is acceptable. For a
bulk change touching more than about 15% of a TVIEW's rows (a migration, a backfill, a
celebrity parent with most of the table below it), suspend refresh and rebuild once:

```sql
BEGIN;
SET LOCAL pg_tviews.suspend_triggers = on;
-- bulk UPDATE / INSERT / DELETE on the base tables
SELECT pg_tviews_refresh('post');   -- TRUNCATE + reload
COMMIT;
```

Measured on a 20 000-row TVIEW, updating 10 000 rows: 1 518 ms with per-row refresh,
123 ms suspended + rebuilt (63 ms update, 60 ms rebuild), with the TVIEW identical to its
view afterwards. `pg_tviews_suspend_triggers()` did not suspend anything when this was
measured (1 493 ms); it is fixed separately.

This guidance goes into the performance guide. `pg_tviews_profile()` (#74) already warns
about high fan-out.

## Consequences / open items

- Re-run on a table larger than memory before revisiting finding 2.
- The propagation lookup (question 4 of the plan) is covered by the #71 propagation
  indexes and was not re-measured here.
