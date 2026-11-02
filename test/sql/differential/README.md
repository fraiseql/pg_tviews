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

## Generated shapes (`gen/`)

`schema.sql` holds the shapes someone thought of; `gen/` draws new ones. From a seed,
`gen/gen.py` (standard library only) writes cases: each a schema of 2 to 4 tables in
the project's convention (`tb_<entity>`, `pk_<entity>` int or bigint, `id uuid`,
value columns of six types, `fk_<entity>` with or without a foreign key) and 3 to 6
TVIEWs drawn from a grammar: plain projections, inner and LEFT joins up to three
levels, embedded TVIEWs, scalar subquery aggregates, `jsonb_agg` arrays of children,
`UNION ALL`, CTEs, `GROUP BY` keyed by the group, `DISTINCT ON` (last child per
parent, first row per root), soft-delete filters. Then random writes: multi-row
inserts, updates, deletes, rows moving between parents, key changes, transactions,
and mutation functions (one `SELECT fn()` writing in a few statements, then reading
a TVIEW back: the read must see fresh rows).

- `gen/run.sh [SHAPES] [SEEDS] [STATEMENTS]` (defaults 20, "1 2 3", 40): each case in
  its own database; a definition is created or refused with a documented SQLSTATE
  (the list, with a comment per code, is in `gen/lib.sql`; refusals are printed);
  any other error, a divergence after a top-level statement or a stale read inside a
  mutation fails the case, printing its seed and keeping its SQL to replay. `MODES`
  as above.
- `GEN_SELFTEST=1` writes with `session_replication_role = replica`, so no pg_tviews
  trigger fires: every case must then diverge. It proves the oracle sees a refresh
  that does not happen.
- `GEN_FEATURES` turns on writes left out of the default run while their defect is
  open (`OPT_IN` in `gen.py`); none is. The generator found #288 (an `UPDATE` of a
  referenced `pk_<entity>` left its children's fan-out unwritten) and #289 (a fan-out
  into the children of a parent a LEFT JOIN's `ON` filters out), both now in the
  default grammar and in `regress/cascade/regress_fanout_patch_soundness.sql`.

CI's pg18 job runs 5 shapes on seed 1; the nightly run 20 shapes on three new seeds
(the date and 1 to 3), in every mode.

## What this net does not cover

Lock order and deadlock classes (the isolation specs, `test/isolation/`), SQLSTATEs
(the regression suite), concurrency (the fuzzer, `test/concurrency/fuzz/`), timing and
statement counts (the benchmarks, `test/bench/`). A change to any of these brings its
own spec or test.
