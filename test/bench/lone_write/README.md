# Lone writes: the statements one write runs

What one write costs when nothing else runs, counted statement by statement. It
complements `test/concurrency/run.sh` (throughput and deadlocks under concurrency) and
`test/bench/write_path/` (latency per write shape).

- `setup.sql`: the `embed` workload's schema (`test/concurrency/embed.sql`). Each of 200
  items shows the counts of current items in its place and its group, so a soft-delete or a
  start-date move changes a link and rewrites a place's and a group's items. It adds
  `tv_group`, `tv_card`, and `tv_owner`, which `tv_lot`, `tv_tag` and `tv_ledger` embed.
- `wide.sql`: one `tv_item` whose read set joins nine tables, at 2,000 items.
- `partitioned.sql`: the embed workload with `tb_item` in 200 hash partitions, 2,000 items.
- `embedded.sql`: the embed workload alone. `composed.sql`: its data with the counts held
  by `tv_place` and `tv_group` and composed at read time, not embedded in every item.
- `pss.sh <label> [package root]`: per write kind, the SPI statements pg_tviews runs, how
  many were planned, and their times, from `pg_stat_statements`. One summary line per kind
  goes to `out/summary.txt` and one statement table to `out/<label>-<schema>-<kind>.txt`.
- `swap.sh <package root>`: installs a packaged build and restarts the dev server.

## Measuring

Load `pg_stat_statements` on the dev server, and undo it afterwards:

```sql
ALTER SYSTEM SET shared_preload_libraries = 'pg_tviews', 'pg_stat_statements';  -- two literals
ALTER SYSTEM SET pg_stat_statements.track = 'all';
ALTER SYSTEM SET pg_stat_statements.track_planning = on;
-- restart; when done: ALTER SYSTEM RESET each of the three, and restart
```

Written as one literal (`'pg_tviews,pg_stat_statements'`), the setting stops the server
from starting.

Package each build into its own directory (`cargo pgrx package --pg-config
~/.pgrx/18.1/pgrx-install/bin/pg_config --out-dir <dir>`, one `CARGO_TARGET_DIR` per
build, md5 of each `.so` checked), then interleave them:

```sh
for round in 1 2; do
  for build in base head; do SCHEMA=wide ./pss.sh "$build" "/tmp/pkg-$build"; done
done
```

Compare:

- **counts** (statements and plans per write): exact, the same at every run;
- **`per_compute`** (the last column of the summary): the nested statements' planning
  and execution time divided by the compute statement's, the recompute upsert, which does
  the same work in every build. Raw milliseconds move with the host's load. Backend CPU
  time is useless on a shared host.

Summary columns: label, schema, kind, statements, plans, plan ms, execution ms, compute
ms, per_compute.
