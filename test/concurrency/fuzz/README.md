# Concurrent differential fuzzer

`run.sh [SECONDS] [SEEDS] [CLIENTS] [MODE]` loads the differential suite's schema
(`test/sql/differential/schema.sql`, every TVIEW shape pg_tviews maintains) and
`fuzz.sql` (crossed fan-outs: users who comment on each other's posts, read through
joins and through embedded TVIEWs) into a fresh database per seed, then runs pgbench
clients against it while a checker compares every TVIEW with its backing view.

- **`mixed`**: each transaction runs 1 to 4 random statements inside one outer statement
  (`fuzz_txn`, as a mutation function runs them: refreshed once at its end), each in a
  subtransaction that one time in five is rolled back on purpose. A constraint error
  (rows another client changed meanwhile) only undoes its statement; `40001` and
  `40P01` fail the transaction, which pgbench counts; any other error aborts the client
  and fails the run. One transaction in ten runs at `REPEATABLE READ` or `SERIALIZABLE`
  (`fuzz_refused`): a write pg_tviews maintains must be refused with `0A000`. `MIX` is
  the share of statements drawn from the differential suite (the rest from `fuzz.sql`).
  One statement in four is followed by a read of a random TVIEW, which must equal its
  view inside the transaction (`fuzz_read_back`: a read refreshes what was queued).
- **`values`**: autocommit single-row writes of columns no TVIEW joins, filters or
  groups by, at `READ COMMITTED`. None may fail: such writes never wait for each other
  in a cycle (#263).

The checker runs `fuzz_check(true)` in a `REPEATABLE READ` transaction in a loop: every
committed snapshot must be fresh, since every transaction maintains its TVIEWs before
it commits. TVIEWs over a materialized view are left to the final check: `REFRESH
MATERIALIZED VIEW` writes its rows frozen, so an older snapshot sees them next to the
TVIEW rows from before it. A stale TVIEW prints its first differing row, the table's
version and the view's.

To chase a failure: `KEEP=1` keeps the database; narrow it with `MIX`;
then write the race as an isolation spec (`test/isolation/specs/`). A statement trigger
named `trg_tview_e_*` fires between pg_tviews' delta triggers and its flush, and can
wait on an advisory lock another session holds, to interleave two statements at that
point (see `crossed-value-writes.spec`).

`MODES` (`test/lib/modes.sh`) runs every seed once per refresh mode: e.g.
`MODES="patch:off,cache:on"` recomputes every row from its view instead of patching
it, `cache:off` reads the catalog at every flush. `MODES="$MODES_ALL"` (after sourcing
`test/lib/modes.sh`) runs the three modes; CI's pg18 job and the nightly run do.
