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
parent, first row per root), whole-row reads, `xmin`, `LATERAL … LIMIT 1` picks,
`FILTER` and `string_agg`, `EXISTS`, `OFFSET 0` fences, `WITH ORDINALITY`,
`ROW_NUMBER() … rn = 1`, `CURRENT_DATE` under `time_refresh`, soft-delete filters; and
definitions outside the dialect (an unordered aggregate or pick, a volatile function,
a session reader, `ctid`, `CURRENT_USER`), which must be refused with `0A000`. Each
kind names the productions of `docs/reference/dialect.toml` it exercises:
`gen/coverage.py` checks every production names a kind or why none draws it, and,
given the `REACHED` lines a run appends to `GEN_COVERAGE`, that each was reached. Then random writes: multi-row
inserts, updates, deletes, rows moving between parents, key changes, transactions,
and mutation functions (one `SELECT fn()` writing in a few statements, then reading
a TVIEW back: the read must see fresh rows).

- `gen/run.sh [SHAPES] [SEEDS] [STATEMENTS]` (defaults 20, "1 2 3", 40): each case in
  its own database; a definition is created or refused with a documented SQLSTATE
  (the list, with a comment per code, is in `gen/lib.sql`; refusals are printed);
  any other error, a divergence after a top-level statement or a stale read inside a
  mutation fails the case, printing its seed and keeping its SQL to replay. `MODES`
  as above.
- **Roles.** Every TVIEW is created by a role that is no superuser and owns no base
  table (granted `SELECT` and `TRIGGER`), every write is made by a third role (from
  which every other table hides some rows by row security the owner bypasses), and
  the oracle reads as the owner, in the frame of `docs/reference/owner-frame.md`.
  `run.sh` creates the two roles for the run and drops them.
- **Visibility** (`visibility.sql`). Every table carries a `tenant_id`, and every row
  references parents of its own tenant only: inserts and moves pick parents of the
  row's tenant, and a flip moves a row nothing references, with its parents, to the
  other tenant (a write to the column the policies filter on). Every TVIEW projects
  its root's tenant. The pattern of `docs/reference/ddl.md` ("Row-level security on
  `tv_*`") is applied: the base tables and every `tv_*` carry the same tenant policy
  for the readers, and a reader is granted a `tv_*` when it may evaluate its
  definition. Four readers (`run.sh` creates them): A and B of tenants 1 and 2, C of
  tenant 1 who may not select one column of one table, N whom no policy lets read a
  row. After every check outside a mutation function, `gen_visible()` evaluates each
  definition as each reader (the backing view's query run as a query, so the base
  tables' policies apply to the reader, as `security_invoker` would) and fails
  (`LEAKED`) when a row the reader reads from `tv_*` is not, binary-wise, the row of
  its evaluation with that key. A privilege error evaluating it means the reader
  reads none of its rows. A TVIEW reading rows of every tenant (a lookup, and a TVIEW
  embedding one) is compared on its keys. Functions a definition calls keep their own
  rights. The tenant choices draw from their own random stream: a seed's shapes and
  writes are those it drew before.
- `GEN_SELFTEST=1` writes with `session_replication_role = replica`, so no pg_tviews
  trigger fires: every case must then diverge. It proves the oracle sees a refresh
  that does not happen. `GEN_SELFTEST=2` lets the oracle read in the session's
  settings (another time zone) instead of the frame's: a case rendering a
  timestamptz must then diverge, and one must. It proves the oracle reads in the
  frame. `GEN_SELFTEST=3` runs every case under each wrong row security pattern: a
  `tv_*` without row security, a child table reader A reads less of than its parent
  (a restrictive policy on it), a `tv_*` granted to reader C, who may not select a
  column it copies. For each, some case must leak. It proves the visibility oracle
  sees a reader reading more than its definition returns to it.
- `GEN_FEATURES` turns on value forms left out of the default run while their defect
  is open (`OPT_IN` in `gen.py`): `exact` (numerics with 30 digits or trailing
  zeros). It also takes the name of any shape kind, which is then half the draws
  (`readset-checks.sh` weights `wholerow,syscol`). The kinds `wholerow`, `syscol` and
  `unordered` started as opt-in and are now in the default grammar.
  The generator found #288 (an `UPDATE` of a
  referenced `pk_<entity>` left its children's fan-out unwritten) and #289 (a fan-out
  into the children of a parent a LEFT JOIN's `ON` filters out), both now in the
  default grammar and in `regress/cascade/regress_fanout_patch_soundness.sql`.

**Skip mutants** (`mutants/`): eleven skip decisions of pg_tviews (each `S<nn>.mutant`
file says what its decision skips and how its mutant breaks it), switched one at a
time to decide wrongly in a build with the `test-mutants` feature
(`pg_tviews.test_mutant`, `src/mutant.rs`): a column-set decision misses the last
column of its set, a predicate decision skips one class of inputs it must not. The
generator must see each one; `mutants/run.sh` fails on a survivor, or on a case
failing otherwise than by a divergence. The generator's `derived` kind and its
write-back triggers exist to kill S41 and S24.

CI's pg18 job runs 5 shapes on seed 1; the nightly run 20 shapes on three new seeds
(the date and 1 to 3), in every mode, and the mutants on seeds 1 to 3.

## What this net does not cover

Lock order and deadlock classes (the isolation specs, `test/isolation/`), SQLSTATEs
(the regression suite), concurrency (the fuzzer, `test/concurrency/fuzz/`), timing and
statement counts (the benchmarks, `test/bench/`). A change to any of these brings its
own spec or test.
