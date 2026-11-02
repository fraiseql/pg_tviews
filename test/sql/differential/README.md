# Differential suite

`schema.sql` creates every TVIEW shape pg_tviews maintains (plain, mapped joins,
DISTINCT ON, embedded TVIEWs, UNION, aggregates, unnest/LATERAL/ANY paths, a
recursive view, window functions, function reads, a materialized view) and the
statement generator the harnesses share.

- `run.sh [N] [SEEDS]`: per seed, N random statements (single- and multi-row writes,
  key changes, rows moving between parents, MERGE, ON CONFLICT, COPY), some in
  transactions with savepoints; after each, every TVIEW must equal its view.
- `golden.sh [--bless]`: what the oracle cannot see, against `expected/`: each shape's
  stored plan (`plan.json`, relation OIDs and column numbers written as names), the
  pg_tviews triggers carrying its entity (`triggers.txt`: table, name, function, type,
  enabled state: topology and firing order), the triggers carrying none
  (`_extension/triggers.txt`) and the feed a fixed script publishes
  (`_feed/feed.txt`, ids masked). Explain every golden diff in the commit that blesses
  it.
- `MODES` (`test/lib/modes.sh`) runs `run.sh` once per refresh mode: the patch path
  off, the catalog caches off. `MODES="$MODES_ALL"` runs all three; CI's pg18 job and
  the nightly run do.

## What this net does not cover

Lock order and deadlock classes (the isolation specs, `test/isolation/`), SQLSTATEs
(the regression suite), concurrency (the fuzzer, `test/concurrency/fuzz/`), timing and
statement counts (the benchmarks, `test/bench/`). A change to any of these brings its
own spec or test.
