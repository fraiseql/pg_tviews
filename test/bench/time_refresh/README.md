# The time refresh under writers

What `pg_tviews_refresh_time_dependent()` costs the writers running while it runs.

- `schema.sql`: seven TVIEWs that read the current time, about 28,100 rows at
  `scale=1` (the row counts of a pilot-sized deployment), and the four saves of the mix
  as mutation functions (one write, then the document read back).
- `save_*.sql`: the pgbench scripts of the 40/20/20/20 mix (placement, asset, site, add-on).
- `run.sh <label>`: loads the database afresh, runs 8 writers for `DURATION` seconds
  (600), calls the time refresh once `AT` seconds in (180), as one call (`FORM=one`) or
  one transaction per TVIEW (`FORM=per_tview`), or `CALL tviews.pg_tviews_time_boundary()`
  (`FORM=call`, the same in one procedure), and keeps per-transaction latencies,
  lock waits sampled every 100 ms, failed saves and refreshes, and `tviews.stats`.
- `summarize.py OUT_DIR`: per run and save, p50/p95/p99 of the saves that overlapped the
  refresh against the others, the failures, the lock waits by tag (`0x5476` value,
  `0x5477` relation), and the full refreshes.

```sh
for run in 1 2 3; do
  for form in one per_tview; do FORM=$form OUT=out ./run.sh "$form-$run"; done
done
python3 -I summarize.py out
```

Latency percentiles need a quiet machine: on a shared host they measure the host.
